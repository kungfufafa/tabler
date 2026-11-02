import { databaseIndex, redisForm } from "@/features/drivers/redis/params";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { RedisCopyArgs } from "../../../bindings/RedisCopyArgs";
import type { RedisDbArgs } from "../../../bindings/RedisDbArgs";
import type { RedisEditArgs } from "../../../bindings/RedisEditArgs";
import type { RedisEditResult } from "../../../bindings/RedisEditResult";
import type { RedisElement } from "../../../bindings/RedisElement";
import type { RedisKeyPage } from "../../../bindings/RedisKeyPage";
import type { RedisNewKeyArgs } from "../../../bindings/RedisNewKeyArgs";
import type { RedisPageArgs } from "../../../bindings/RedisPageArgs";
import type { RedisServerInfo } from "../../../bindings/RedisServerInfo";
import type { RedisSlowEntry } from "../../../bindings/RedisSlowEntry";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { READ_ONLY, runConsole, type ConsoleState } from "./console";
import { globMatch, memoryOf, server, sizeOf, zsetOrder, type Entry, type Server } from "./data";
import { infoText } from "./engine";
import {
  applyPlan,
  CHANGED,
  createKey,
  keyArg,
  keyEdits,
  planChange,
  preview,
  rowKey,
  ttlMs,
  type KeyEdit,
} from "./plan";

/**
 * A Redis connection of the mock (`drivers/redis/session.rs`): every database a table of keys
 * keyed by `(key, type)` with the read-only `type`, `size` and `memory` columns, browsed by SCAN
 * cursor with the filter bar as globs, an exact key or a type; edits planned and applied as the
 * backend does (a result per change, no rollback), the console, the value editor's calls and the
 * server's clients. A read-only connection refuses every write path with the backend's message.
 */

/** The backend's caps (`RedisSession::capabilities`). */
export const REDIS_CAPS: SessionCaps = {
  driver: "redis",
  flavor: null,
  dialect: null,
  language: "redisCli",
  features: [
    "editor",
    "editRows",
    "insertRows",
    "deleteRows",
    "exactCount",
    "structureView",
    "viewDefinition",
    "processList",
    "killProcess",
    "startupCommands",
  ],
  namespaces: {
    databases: false,
    schemas: "single",
    defaultSchema: null,
    labels: { database: "Database", schema: "Schema", table: "Database", tables: "Databases" },
  },
  data: {
    paging: "cursor",
    count: "estimate",
    sort: false,
    filterOps: [
      "eq",
      "like",
      "ilike",
      "contains",
      "containsCase",
      "hasPrefix",
      "hasPrefixCase",
      "hasSuffix",
      "hasSuffixCase",
    ],
    rawFilter: null,
    transactions: "batch",
    affected: "exact",
    defaultKeyword: false,
    upsert: false,
    serverExprs: [],
    maxParams: 0,
  },
  safety: { readOnly: "classifierOnly", hardened: false, confirm: "password" },
  session: { keepaliveSecs: 30, pingBilled: false },
};

const KNOWN_TYPES = ["string", "list", "set", "zset", "hash", "stream", "ReJSON-RL"];

function column(
  name: string,
  ordinal: number,
  dataType: string,
  kind: ColumnMeta["kind"],
  extra: Partial<ColumnMeta> = {},
): ColumnMeta {
  return {
    name,
    ordinal,
    dataType,
    kind,
    nullable: true,
    defaultExpr: null,
    isPk: false,
    pkOrdinal: null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra: null,
    ...extra,
  };
}

/** The columns of a `dbN` table (`keys::columns`). */
export const COLUMNS: ColumnMeta[] = [
  column("key", 1, "key", "text", {
    nullable: false,
    isPk: true,
    pkOrdinal: 1,
    comment: "text, or bytes when the name is not UTF-8",
  }),
  column("type", 2, "type", "enum", {
    nullable: false,
    isPk: true,
    pkOrdinal: 2,
    isGenerated: true,
    enumValues: KNOWN_TYPES,
  }),
  column("ttl", 3, "ttl (seconds)", "int", {
    comment: "seconds until the key expires; empty for no expiry",
  }),
  column("size", 4, "size", "int", {
    isGenerated: true,
    comment: "bytes of a string, elements of a container",
  }),
  column("memory", 5, "memory (bytes)", "int", { isGenerated: true }),
  column("value", 6, "value", "text", {
    comment: "a string's value; a preview of a container's elements",
  }),
];

const PK = ["key", "type"];
const text = (v: string): Value => ({ t: "text", v });
const int = (v: number): Value => ({ t: "int", v });
const NULL: Value = { t: "null" };
const json = (v: unknown): Value => ({ t: "json", v: JSON.stringify(v) });

/** The TTL cell, rounded the way Redis's `TTL` rounds (`ttl_cell`). */
const ttlCell = (e: Entry): Value =>
  e.pttl === null ? NULL : int(Math.floor((e.pttl + 500) / 1000));

const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

interface Limits {
  bytes: number;
  elements: number;
  memory: boolean;
}

/** The grid's value preview (`keys::preview_cell`): ranged and capped, as the backend reads it. */
function previewCell(e: Entry, l: Limits): Value {
  switch (e.type) {
    case "string": {
      const bytes = new TextEncoder().encode(e.value);
      if (bytes.length <= l.bytes) return text(e.value);
      return {
        t: "bytes",
        v: { base64: b64(bytes.subarray(0, l.bytes)), len: bytes.length, truncated: true },
      };
    }
    case "list":
      return json(e.value.slice(0, l.elements));
    case "set":
      return json([...e.value].slice(0, l.elements));
    case "hash":
      return json(Object.fromEntries([...e.value].slice(0, l.elements)));
    case "zset":
      return json(Object.fromEntries(zsetOrder(e.value).slice(0, l.elements)));
    case "stream":
      return json(
        [...e.value]
          .reverse()
          .slice(0, l.elements)
          .map((x) => ({ id: x.id, fields: Object.fromEntries(x.fields) })),
      );
    case "ReJSON-RL":
      return { t: "json", v: e.value };
    default:
      return NULL;
  }
}

/** A key's whole value (`fetch_cell`'s `value`). */
function wholeValue(e: Entry): Value {
  return e.type === "string"
    ? text(e.value)
    : previewCell(e, { bytes: Infinity, elements: 100_000, memory: true });
}

function row(key: string, e: Entry, l: Limits): Value[] {
  const size = sizeOf(e);
  return [
    text(key),
    text(e.type),
    ttlCell(e),
    size === null ? NULL : int(size),
    l.memory ? int(memoryOf(key, e)) : NULL,
    previewCell(e, l),
  ];
}

const unsupportedFilter = (): never =>
  mockError(
    "UNSUPPORTED",
    "unsupported: Redis filters keys by one name pattern (equals, contains, starts / ends with, like, or a raw glob) and by type",
  );

/** Escapes a glob's characters; with `fold` ASCII letters match either case (`glob_literal`). */
function globLiteral(s: string, fold: boolean): string {
  let out = "";
  for (const c of s) {
    if (fold && /[A-Za-z]/.test(c)) out += `[${c.toLowerCase()}${c.toUpperCase()}]`;
    else out += /[*?[\]\\]/.test(c) ? `\\${c}` : c;
  }
  return out;
}

/** A LIKE pattern as a glob (`like_glob`). */
function likeGlob(p: string, fold: boolean): string {
  let out = "";
  const chars = Array.from(p);
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i]!;
    if (c === "%") out += "*";
    else if (c === "_") out += "?";
    else if (c === "\\" && i + 1 < chars.length) out += globLiteral(chars[++i]!, fold);
    else out += globLiteral(c, fold);
  }
  return out;
}

interface KeyFilter {
  exact: string | null;
  pattern: string | null;
  type: string | null;
}

/** The filter bar as a key match and a type (`scan::key_filter`). */
export function keyFilter(filter: FilterGroup | null): KeyFilter {
  const out: KeyFilter = { exact: null, pattern: null, type: null };
  const setKeys = (exact: string | null, pattern: string | null) => {
    if (out.exact !== null || out.pattern !== null) unsupportedFilter();
    out.exact = exact;
    out.pattern = pattern;
  };
  const keyMatch = (op: string, values: Value[]) => {
    if (values.length !== 1) unsupportedFilter();
    const v = keyArg(values[0]!);
    const wrap = (pre: boolean, post: boolean, fold: boolean) =>
      setKeys(null, `${pre ? "*" : ""}${globLiteral(v, fold)}${post ? "*" : ""}`);
    switch (op) {
      case "eq":
        return setKeys(v, null);
      case "like":
        return setKeys(null, likeGlob(v, false));
      case "ilike":
        return setKeys(null, likeGlob(v, true));
      case "contains":
        return wrap(true, true, false);
      case "containsCase":
        return wrap(true, true, true);
      case "hasPrefix":
        return wrap(false, true, false);
      case "hasPrefixCase":
        return wrap(false, true, true);
      case "hasSuffix":
        return wrap(true, false, false);
      case "hasSuffixCase":
        return wrap(true, false, true);
      default:
        return unsupportedFilter();
    }
  };
  const add = (g: FilterGroup) => {
    if (g.items.length > 1 && g.conjunction === "or") unsupportedFilter();
    for (const item of g.items) {
      if (item.kind === "group") add(item);
      else if (item.kind === "raw") {
        const glob = item.sql.trim();
        if (glob !== "") setKeys(null, glob);
      } else if (item.kind === "anyColumn") keyMatch(item.op, item.values);
      else if (item.column === "type") {
        const v = item.values[0];
        if (
          item.op !== "eq" ||
          item.values.length !== 1 ||
          !v ||
          (v.t !== "text" && v.t !== "enum")
        )
          unsupportedFilter();
        if (out.type !== null) unsupportedFilter();
        out.type = (v as { v: string }).v;
      } else if (item.column === "key") keyMatch(item.op, item.values);
      else unsupportedFilter();
    }
  };
  if (filter) add(filter);
  return out;
}

const quoteGlob = (s: string) => (/^[^\s"';]+$/.test(s) ? s : JSON.stringify(s));

export function redisSession(profile: ConnectionProfile): MockSession {
  const s: Server = server();
  const readOnly = profile.options.readOnly;
  const defaultDb = databaseIndex(profile.database);
  if (defaultDb === null || defaultDb >= s.databases)
    mockError("INVALID_INPUT", `the DB index is a number from 0 to ${s.databases - 1}`);
  const f = redisForm(profile);
  const limits: Limits = {
    bytes: Number(f.previewBytes) || 16 * 1024,
    elements: Number(f.previewElements) || 50,
    memory: f.memoryColumn,
  };
  const consoleState: ConsoleState = { server: s, readOnly, db: defaultDb, queue: null };
  const clientId = s.nextClient++;
  s.clients.push({
    id: clientId,
    addr: "127.0.0.1:50001",
    name: "",
    user: profile.user || "default",
    db: defaultDb,
    cmd: "client|list",
    idle: 0,
    flags: "N",
  });

  const refuseWrite = () => {
    if (readOnly) mockError("INVALID_INPUT", READ_ONLY);
  };
  const dbOf = (table: TableRef): number => {
    const m = /^db(\d{1,9})$/.exec(table.name);
    const n = m ? Number(m[1]) : NaN;
    return Number.isInteger(n) && n < s.databases
      ? n
      : mockError("NOT_FOUND", `not found: Redis has no database ${table.name}`);
  };
  const keysOf = (db: number) => s.dbs.get(db)!;
  const plan = (req: ApplyRequest): KeyEdit[] => req.changes.map((c, i) => planChange(i, c));

  const editResult = (db: number, edits: KeyEdit[]): RedisEditResult => {
    if (db >= s.databases) mockError("NOT_FOUND", `not found: Redis has no database db${db}`);
    return {
      results: applyPlan(s, db, edits).map((e) => ({ ok: e === null, error: e })),
      preview: preview(edits),
    };
  };

  const keyPage = (a: RedisPageArgs): RedisKeyPage => {
    if (a.db >= s.databases) mockError("NOT_FOUND", `not found: Redis has no database db${a.db}`);
    const key = keyArg(a.key);
    const e = keysOf(a.db).get(key);
    if (!e) mockError("NOT_FOUND", "not found: the key no longer exists (changed by someone else)");
    if (e.type !== a.type) mockError("INVALID_INPUT", CHANGED);
    const count = Math.min(Math.max(a.count ?? 100, 1), 10_000);
    const size = sizeOf(e);
    const page: RedisKeyPage = {
      key: text(key),
      type: e.type,
      ttl: ttlCell(e),
      size: size === null ? NULL : int(size),
      value: null,
      elements: [],
      nextCursor: null,
    };
    const from = a.cursor ? Number(a.cursor) : 0;
    if (!Number.isInteger(from) || from < 0)
      mockError("INVALID_INPUT", `“${a.cursor ?? ""}” is not a cursor`);
    const window = <T>(items: T[], map: (x: T, i: number) => RedisElement) => {
      page.elements = items.slice(from, from + count).map((x, i) => map(x, from + i));
      page.nextCursor = from + count < items.length ? String(from + count) : null;
    };
    switch (e.type) {
      case "string":
        page.value = previewCell(e, limits);
        break;
      case "ReJSON-RL":
        page.value = { t: "json", v: e.value };
        break;
      case "hash":
        window([...e.value], ([k, v]) => ({ name: text(k), value: text(v) }));
        break;
      case "set":
        window([...e.value], (m) => ({ name: text(m), value: null }));
        break;
      case "list":
        window(e.value, (v, i) => ({ name: int(i), value: text(v) }));
        break;
      case "zset":
        window(zsetOrder(e.value), ([m, sc]) => ({ name: text(m), value: { t: "float", v: sc } }));
        break;
      case "stream":
        // Newest first; the mock's cursor is a position.
        window([...e.value].reverse(), (x) => ({
          name: text(x.id),
          value: json(Object.fromEntries(x.fields)),
        }));
        break;
    }
    return page;
  };

  return {
    caps: REDIS_CAPS,
    currentDatabase: `db${defaultDb}`,
    listDatabases: () =>
      Array.from({ length: s.databases }, (_, i) => ({
        name: `db${i}`,
        owner: null,
        encoding: null,
        isCurrent: i === defaultDb,
      })),
    listSchemas: () => [],
    listTables: () =>
      Array.from({ length: s.databases }, (_, i) => {
        const keys = keysOf(i);
        const expires = [...keys.values()].filter((e) => e.pttl !== null).length;
        return {
          name: `db${i}`,
          schema: null,
          kind: "table" as const,
          rowEstimate: keys.size,
          sizeBytes: null,
          comment: expires > 0 ? `${expires} keys with a TTL` : null,
        };
      }),
    tableMeta: (table): TableMeta => ({
      table,
      kind: "table",
      columns: COLUMNS,
      primaryKey: PK,
      indexes: [],
      foreignKeys: [],
      triggers: [],
      comment: null,
      rowEstimate: keysOf(dbOf(table)).size,
      ddl: null,
      checks: [],
      options: null,
    }),
    viewDefinition: (table) => {
      const db = dbOf(table);
      const keys = keysOf(db);
      return JSON.stringify(
        {
          database: db,
          keys: keys.size,
          keysWithTtl: [...keys.values()].filter((e) => e.pttl !== null).length,
          averageTtlMs: 0,
          server: `${s.product} ${s.version}`,
          mode: s.mode,
          protocol: "RESP3",
          primaryKey: PK,
        },
        null,
        2,
      );
    },
    browse: (req): BrowsePage => {
      const db = dbOf(req.table);
      const keys = keysOf(db);
      const f = keyFilter(req.filter);
      const page = (rows: Value[][], sql: string, next: string | null): BrowsePage => ({
        columns: COLUMNS,
        rows,
        pkColumns: PK,
        identity: "pk",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql,
        durationMs: 1,
        nextCursor: next,
      });
      if (f.exact !== null) {
        const e = keys.get(f.exact);
        const rows = e && (f.type === null || e.type === f.type) ? [row(f.exact, e, limits)] : [];
        return page(rows, `TYPE ${quoteGlob(f.exact)}`, null);
      }
      // SCAN steps of COUNT keys from the cursor until a page is full or the keyspace ends.
      const all = [...keys.keys()];
      const count = Number(redisForm(profile).scanCount) || req.limit;
      let at = req.cursor ? Number(req.cursor) : 0;
      if (!Number.isInteger(at) || at < 0)
        mockError("INVALID_INPUT", `“${req.cursor ?? ""}” is not a SCAN cursor`);
      const scans: string[] = [];
      const rows: Value[][] = [];
      while (at < all.length && rows.length < req.limit) {
        scans.push(
          [
            "SCAN",
            String(at),
            ...(f.pattern ? ["MATCH", quoteGlob(f.pattern)] : []),
            "COUNT",
            String(count),
            ...(f.type ? ["TYPE", f.type] : []),
          ].join(" "),
        );
        const end = Math.min(all.length, at + count);
        for (; at < end; at++) {
          const key = all[at]!;
          const e = keys.get(key)!;
          if (f.pattern !== null && !globMatch(f.pattern, key)) continue;
          if (f.type !== null && e.type !== f.type) continue;
          rows.push(row(key, e, limits));
        }
      }
      if (scans.length === 0) scans.push(`SCAN ${at} COUNT ${count}`);
      return page(rows, scans.join("\n"), at < all.length ? String(at) : null);
    },
    count: (table, filter) => {
      const keys = keysOf(dbOf(table));
      const f = keyFilter(filter);
      if (f.exact !== null) {
        const e = keys.get(f.exact);
        return { count: e && (f.type === null || e.type === f.type) ? 1 : 0, exact: true };
      }
      if (f.pattern === null && f.type === null) return { count: keys.size, exact: true };
      // A small keyspace is scanned whole within the backend's time, so the count is exact.
      let n = 0;
      for (const [key, e] of keys)
        if (
          (f.pattern === null || globMatch(f.pattern, key)) &&
          (f.type === null || e.type === f.type)
        )
          n++;
      return { count: n, exact: true };
    },
    fetchCell: (table, key, columnName) => {
      const [k, loaded] = rowKey(key);
      const e = keysOf(dbOf(table)).get(k);
      if (!e)
        mockError("NOT_FOUND", "not found: the key no longer exists (changed by someone else)");
      if (loaded !== null && loaded !== e.type) mockError("INVALID_INPUT", CHANGED);
      switch (columnName) {
        case "key":
          return text(k);
        case "type":
          return text(e.type);
        case "ttl":
          return ttlCell(e);
        case "size": {
          const n = sizeOf(e);
          return n === null ? NULL : int(n);
        }
        case "memory":
          return int(memoryOf(k, e));
        case "value":
          return wholeValue(e);
        default:
          return mockError("INVALID_INPUT", `a Redis key has no column “${columnName}”`);
      }
    },
    preview: (req) => {
      refuseWrite();
      dbOf(req.table);
      return preview(plan(req));
    },
    apply: (req): ApplyResult => {
      const db = dbOf(req.table);
      const edits = (() => {
        refuseWrite();
        return plan(req);
      })();
      const outcomes = applyPlan(s, db, edits);
      const results: RowResult[] = req.changes.map((c, i) => {
        const e = outcomes[i] ?? null;
        return {
          rowId: c.rowId,
          op: c.op,
          affected: e === null ? 1 : 0,
          returned: null,
          error:
            e === null
              ? null
              : {
                  code: "DB_ERROR",
                  message: e,
                  detail: null,
                  hint: null,
                  sqlstate: null,
                  vendorCode: null,
                  position: null,
                  statementIndex: null,
                  data: null,
                },
        };
      });
      const error = results.find((r) => r.error)?.error ?? null;
      return { success: error === null, results, sql: preview(edits), error, durationMs: 1 };
    },
    run: (statement, emit) => runConsole(consoleState, statement, emit),
    call: (method, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      // Default-deny like calls.rs call_kind: every call but the reads counts as a write.
      const reads = ["serverInfo", "keyPage", "slowLog", "previewEdits"];
      if (!reads.includes(method)) refuseWrite();
      switch (method) {
        case "serverInfo": {
          const info = infoText(s, "everything");
          const sections = info.split(/\r\n\r\n/).map((block) => {
            const [head = "", ...lines] = block.split(/\r\n/);
            return {
              name: head.replace(/^# /, ""),
              fields: lines.map((l): [string, string] => [
                l.slice(0, l.indexOf(":")),
                l.slice(l.indexOf(":") + 1),
              ]),
            };
          });
          return {
            product: s.product,
            version: s.version,
            mode: s.mode,
            protocol: 3,
            databases: s.databases,
            readOnly,
            sections,
          } satisfies RedisServerInfo;
        }
        case "keyPage":
          return keyPage(a as unknown as RedisPageArgs);
        case "slowLog": {
          const n = Math.min(Math.max(Number(a.count ?? 128) || 128, 1), 10_000);
          return s.slowlog.slice(0, n).map((e): RedisSlowEntry => ({
            id: int(e.id),
            startedAt: int(e.at),
            durationUs: int(e.us),
            command: e.argv.join(" "),
            client: e.client || null,
            clientName: e.name || null,
          }));
        }
        case "previewEdits": {
          const e = a as unknown as RedisEditArgs;
          return preview(keyEdits(keyArg(e.key), e.type, e.edits));
        }
        case "editKey": {
          const e = a as unknown as RedisEditArgs;
          return editResult(e.db, keyEdits(keyArg(e.key), e.type, e.edits));
        }
        case "newKey": {
          const n = a as unknown as RedisNewKeyArgs;
          const ttl = n.ttl ? ttlMs(n.ttl) : null;
          return editResult(n.db, [{ change: 0, ...createKey(keyArg(n.key), n.value, ttl) }]);
        }
        case "copyKey": {
          const c = a as unknown as RedisCopyArgs;
          return editResult(c.db, [
            {
              change: 0,
              guard: { kind: "none" },
              commands: [
                {
                  argv: ["COPY", keyArg(c.from), keyArg(c.to)],
                  expect: { kind: "int", n: 1, msg: "a key with this name already exists" },
                },
              ],
            },
          ]);
        }
        case "flushDb": {
          const d = a as unknown as RedisDbArgs;
          if (d.db >= s.databases)
            mockError("NOT_FOUND", `not found: Redis has no database db${d.db}`);
          return editResult(d.db, [
            {
              change: 0,
              guard: { kind: "none" },
              commands: [{ argv: ["FLUSHDB", "ASYNC"], expect: { kind: "any" } }],
            },
          ]);
        }
        default:
          return mockError("UNSUPPORTED", `unsupported: Redis: unknown call “${method}”`);
      }
    },
    listProcesses: () =>
      s.clients.map((c) => ({
        id: String(c.id),
        user: c.user,
        database: `db${c.db}`,
        application: c.name || null,
        client: c.addr,
        state: c.flags,
        command: c.cmd,
        startedAt: null,
        durationMs: c.idle * 1000,
        waitEvent: null,
        query: c.cmd,
        current: c.id === clientId,
        own: c.id === clientId,
      })),
    killProcesses: (ids) =>
      ids.map((id) => {
        if (readOnly) return { id, ok: false, message: READ_ONLY };
        const n = Number(id.trim());
        if (!Number.isInteger(n)) return { id, ok: false, message: `invalid client id ${id}` };
        const at = s.clients.findIndex((c) => c.id === n);
        if (at < 0) return { id, ok: false, message: "no such client (it may have disconnected)" };
        s.clients.splice(at, 1);
        return { id, ok: true, message: null };
      }),
  };
}
