import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { BrowseRequest } from "../../../bindings/BrowseRequest";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterNode } from "../../../bindings/FilterNode";
import type { FilterOp } from "../../../bindings/FilterOp";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { runConsole } from "./console";
import { MISSING, N, tables, typeOf, type Item, type KeySchema, type Table } from "./data";
import {
  attrJson,
  compare,
  describeTable,
  itemJson,
  keyNames,
  keyOf,
  keyText,
  mergeColumns,
  requestText,
  same,
  tableMeta,
} from "./model";

/**
 * A mock DynamoDB session, mirroring the backend's (`drivers/dynamodb/session.rs`): the same caps
 * (cursor pages, a costly count, sort only by the sort key of a Query, the 17 filter operators,
 * changes as one conditional request or one transaction, the console), the region as the one
 * database and schema, Scan or Query pages with the last key as the cursor, conditional edits
 * whose failed condition reads like the backend's, the `tableInfo` and `browseIndex` calls, and
 * every refusal of a read-only session.
 */

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** The backend's `errors::CONDITION_FAILED`. */
export const CONDITION_FAILED =
  "expected to affect 1 row(s) but matched 0; the item may have been changed or deleted by someone else";

/** The 17 filter operators of the backend's `FILTER_OPS`. */
export const FILTER_OPS: FilterOp[] = [
  "eq",
  "ne",
  "gt",
  "lt",
  "ge",
  "le",
  "like",
  "notLike",
  "in",
  "notIn",
  "isNull",
  "isNotNull",
  "between",
  "notBetween",
  "contains",
  "notContains",
  "hasPrefix",
];

/** `session::capabilities(region)`. */
export function dynamoCaps(region: string): SessionCaps {
  return {
    driver: "dynamodb",
    flavor: null,
    dialect: null,
    language: "partiql",
    // In `Feature`'s order, as the backend sorts them.
    features: [
      "editor",
      "editRows",
      "insertRows",
      "deleteRows",
      "exactCount",
      "structureView",
      "viewDefinition",
    ],
    namespaces: {
      databases: false,
      schemas: "single",
      defaultSchema: region,
      labels: { database: "Region", schema: "Region", table: "Table", tables: "Tables" },
    },
    data: {
      paging: "cursor",
      count: "costly",
      sort: true,
      filterOps: FILTER_OPS,
      rawFilter: null,
      transactions: "batch",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: [],
      maxParams: 0,
    },
    safety: { readOnly: "classifierOnly", hardened: false, confirm: "passcode" },
    session: { keepaliveSecs: null, pingBilled: false },
  };
}

export const failure = (
  code: string,
  message: string,
  detail: string | null = null,
): ErrorPayload => ({
  code,
  message,
  detail,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: null,
  data: null,
});

/** The mock's view of a profile's params. */
export interface MockDynamoOptions {
  region: string;
  readOnly: boolean;
  sampleSize: number;
  /** The endpoint the server info names, when the profile has one of its own. */
  endpoint: string | null;
}

export function optionsOf(p: ConnectionProfile): MockDynamoOptions {
  const params = p.options.driverParams ?? {};
  const sample = Number(params.sampleSize);
  const endpoint =
    typeof params.endpoint === "string" && params.endpoint.trim() !== ""
      ? params.endpoint.trim()
      : null;
  return {
    region: p.database?.trim() || "us-east-1",
    readOnly: p.options.readOnly,
    sampleSize: Number.isInteger(sample) && sample > 0 ? sample : 100,
    endpoint: endpoint ?? (params.auth === "local" ? "http://localhost:8000/" : null),
  };
}

export function tableOf(name: string): Table {
  return (
    tables().get(name) ??
    mockError("DB_ERROR", `Requested resource not found: Table: ${name} not found`)
  );
}

// --- reading ----------------------------------------------------------------------------------

const unsupported = (what: string): never =>
  mockError("UNSUPPORTED", `unsupported: DynamoDB filters cannot ${what}`);

const textOf = (v: Value | undefined): string | null =>
  v?.t === "text"
    ? v.v
    : v?.t === "int" || v?.t === "float" || v?.t === "bigInt" || v?.t === "decimal"
      ? String(v.v)
      : null;

/** Whether `item` passes one condition of the filter bar (as the backend's expressions test it). */
function passes(item: Item, column: string, op: FilterOp, raw: Value[]): boolean {
  const v = item[column];
  // The filter bar's text means a number in a number attribute (the backend's `candidates`).
  const values = raw.map((x) =>
    v && typeOf(v) === "N" && x.t === "text" && /^-?\d+(\.\d+)?$/.test(x.v.trim())
      ? N(x.v.trim())
      : x,
  );
  const one = values[0];
  const eq = (x: Value | undefined) =>
    v !== undefined &&
    x !== undefined &&
    (same(v, x) || (typeOf(v) === "N" && compare(v, x) === 0));
  switch (op) {
    case "eq":
      return eq(one);
    case "ne":
      return !eq(one);
    case "gt":
      return v !== undefined && compare(v, one) > 0;
    case "lt":
      return v !== undefined && compare(v, one) < 0;
    case "ge":
      return v !== undefined && compare(v, one) >= 0;
    case "le":
      return v !== undefined && compare(v, one) <= 0;
    case "in":
      return values.some(eq);
    case "notIn":
      return !values.some(eq);
    case "isNull":
      return v === undefined || v.t === "null";
    case "isNotNull":
      return v !== undefined && v.t !== "null";
    case "between":
    case "notBetween": {
      const inside = v !== undefined && compare(v, values[0]) >= 0 && compare(v, values[1]) <= 0;
      return op === "between" ? inside : !inside;
    }
    case "contains":
    case "notContains": {
      const needle = textOf(one) ?? "";
      const has =
        v?.t === "text"
          ? v.v.includes(needle)
          : v?.t === "array"
            ? v.v.some((x) => textOf(x) === needle)
            : false;
      return op === "contains" ? has : !has;
    }
    case "hasPrefix":
      return v?.t === "text" && v.v.startsWith(textOf(one) ?? "");
    case "like":
    case "notLike": {
      const pattern = textOf(one) ?? "";
      const body = pattern.endsWith("%") ? pattern.slice(0, -1) : pattern;
      if (/[%_]/.test(body))
        unsupported("match LIKE patterns other than a prefix (abc%) or an exact value");
      const hit = v?.t === "text" && (pattern.endsWith("%") ? v.v.startsWith(body) : v.v === body);
      return op === "like" ? hit : !hit;
    }
    default:
      return unsupported(`use ${op}`);
  }
}

function matches(item: Item, g: FilterGroup): boolean {
  const test = (n: FilterNode): boolean => {
    switch (n.kind) {
      case "condition":
        return passes(item, n.column, n.op, n.values);
      case "group":
        return matches(item, n);
      case "anyColumn":
        return Object.keys(item).some((c) => passes(item, c, n.op, n.values));
      case "raw":
        return mockError("UNSUPPORTED", "unsupported: DynamoDB has no raw filter");
    }
  };
  if (g.items.length === 0) return true;
  return g.conjunction === "and" ? g.items.every(test) : g.items.some(test);
}

/** What a page reads: a Query when the filter fixes the partition key with `=`, else a Scan. */
interface Read {
  action: "Scan" | "Query";
  hash: Value | null;
}

function readOf(keys: KeySchema, filter: FilterGroup | null): Read {
  const top =
    filter?.conjunction === "and" || filter?.items.length === 1 ? (filter?.items ?? []) : [];
  const hash = top.find(
    (n) =>
      n.kind === "condition" &&
      n.column === keys.hash.name &&
      n.op === "eq" &&
      n.values.length === 1,
  );
  return hash?.kind === "condition"
    ? { action: "Query", hash: hash.values[0] ?? null }
    : { action: "Scan", hash: null };
}

/** The items a read returns, in the order DynamoDB returns them. */
function ordered(items: readonly Item[], keys: KeySchema, read: Read, descending: boolean): Item[] {
  if (read.action === "Scan") return [...items];
  const range = keys.range?.name;
  const out = items.filter((i) => same(i[keys.hash.name], read.hash ?? undefined));
  if (range) out.sort((a, b) => compare(a[range], b[range]) * (descending ? -1 : 1));
  return out;
}

/**
 * Where a page ends: the table's key and, for an index, the index's key attributes too (an
 * index's `LastEvaluatedKey`), as DynamoDB JSON with its names in order.
 */
function cursorText(item: Item, t: Table, keys: KeySchema): string {
  const names = [...new Set([...keyNames(t.keys), ...keyNames(keys)])].sort();
  return JSON.stringify(itemJson(Object.fromEntries(names.map((n) => [n, item[n] ?? MISSING]))));
}

/** A page's end as the grid's cursor (base64url, as `encode_cursor`). */
export const encodeCursor = (text: string): string =>
  btoa(unescape(encodeURIComponent(text)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");

export function decodeCursor(cursor: string): string {
  try {
    const b64 = cursor.replace(/-/g, "+").replace(/_/g, "/");
    return JSON.stringify(JSON.parse(decodeURIComponent(escape(atob(b64)))));
  } catch {
    return mockError("INVALID_INPUT", "the page cursor is not one DynamoDB gave");
  }
}

/** One grid page of `items` (a table's, or an index's with `keys` its keys). */
export function page(
  t: Table,
  items: readonly Item[],
  keys: KeySchema,
  req: BrowseRequest,
  region: string,
  sampleSize: number,
  index: string | null,
): BrowsePage {
  const started = performance.now();
  const sort = req.sort;
  const range = keys.range?.name;
  if (sort.length > 0 && (sort.length > 1 || sort[0]!.column !== range))
    mockError(
      "UNSUPPORTED",
      "unsupported: DynamoDB sorts only by the sort key, in a Query (filter the partition key with =)",
    );
  const read = readOf(keys, req.filter);
  if (sort.length > 0 && read.action !== "Query")
    mockError(
      "UNSUPPORTED",
      "unsupported: DynamoDB sorts only a Query: filter the partition key with =, the sort key with at most one condition, and the other columns without Any column",
    );
  const all = ordered(items, keys, read, sort[0]?.dir === "desc").filter(
    (i) => !req.filter || matches(i, req.filter),
  );
  // The table's key orders a page's items (an index's items are told apart by it too).
  let start = 0;
  if (req.cursor) {
    const after = decodeCursor(req.cursor);
    const at = all.findIndex((i) => cursorText(i, t, keys) === after);
    start = at + 1;
  }
  const rows = all.slice(start, start + req.limit);
  const last = rows[rows.length - 1];
  const more = start + req.limit < all.length;
  const meta = tableMeta(t, region, sampleSize);
  let cols = mergeColumns(meta.columns, rows);
  if (req.columns) cols = req.columns.flatMap((w) => cols.filter((c) => c.name === w));
  const body: Record<string, unknown> = { TableName: t.name };
  if (index) body.IndexName = index;
  if (read.action === "Query") {
    body.KeyConditionExpression = "#n0 = :v0";
    body.ExpressionAttributeNames = { "#n0": keys.hash.name };
    body.ExpressionAttributeValues = { ":v0": read.hash ? attrJson(read.hash) : null };
    if (sort[0]?.dir === "desc") body.ScanIndexForward = false;
  }
  body.Limit = req.limit;
  return {
    columns: cols,
    rows: rows.map((item) => cols.map((c) => item[c.name] ?? MISSING)),
    pkColumns: keyNames(t.keys),
    identity: "pk",
    rowIds: null,
    offset: req.offset,
    limit: req.limit,
    sql: `${read.action} ${JSON.stringify(body)}`,
    durationMs: Math.max(1, Math.round(performance.now() - started)),
    nextCursor: more && last ? encodeCursor(cursorText(last, t, keys)) : null,
  };
}

// --- edits ------------------------------------------------------------------------------------

/** The key of a row key, typed by the table's key schema. */
function keyItem(key: RowKey, keys: KeySchema): Item {
  if (key.kind !== "pk") mockError("INVALID_INPUT", "DynamoDB items are edited by their key");
  const out: Item = {};
  for (const n of keyNames(keys)) {
    const v = key.value[n];
    if (v === undefined) mockError("INVALID_INPUT", `the row's key lacks ${n}`);
    out[n] = v;
  }
  return out;
}

const isMissing = (v: Value) => v.t === "other" && v.v.ty === "missing";

/** One write of a plan: what it does to the table, its condition and its text. */
interface Write {
  change: number;
  kind: "Put" | "Update" | "Delete";
  key: Item;
  /** Put: the whole item. Update: the attributes set (missing: removed). */
  values: Item;
  text: string;
}

function checkValues(values: Record<string, Value>): void {
  for (const v of Object.values(values))
    if (v.t === "default") mockError("INVALID_INPUT", "DynamoDB has no default values");
}

/** The writes of the staged changes (`sqlgen::native::dynamo::plan_changes`), with their text. */
function plan(t: Table, changes: readonly StagedChange[]): Write[] {
  const keys = t.keys;
  const out: Write[] = [];
  const text = (action: string, body: Record<string, unknown>) =>
    requestText(action, { TableName: t.name, ...body });
  changes.forEach((ch, i) => {
    if (ch.op === "insert") {
      checkValues(ch.values);
      const item: Item = Object.fromEntries(
        Object.entries(ch.values).filter(([, v]) => !isMissing(v)),
      );
      for (const k of [keys.hash, keys.range])
        if (k && typeOf(item[k.name] ?? MISSING) !== k.type)
          mockError("INVALID_INPUT", `the new item needs its key attribute ${k.name} (${k.type})`);
      out.push({
        change: i,
        kind: "Put",
        key: keyOf(item, keys)!,
        values: item,
        text: text("PutItem", {
          Item: itemJson(item),
          ConditionExpression: "attribute_not_exists(#n0)",
          ExpressionAttributeNames: { "#n0": keys.hash.name },
        }),
      });
      return;
    }
    const key = keyItem(ch.key, keys);
    if (ch.op === "delete") {
      out.push({
        change: i,
        kind: "Delete",
        key,
        values: {},
        text: text("DeleteItem", {
          Key: itemJson(key),
          ConditionExpression: "attribute_exists(#n0)",
          ExpressionAttributeNames: { "#n0": keys.hash.name },
        }),
      });
      return;
    }
    checkValues(ch.changes);
    const newKey = { ...key };
    const sets: Item = {};
    for (const [name, v] of Object.entries(ch.changes)) {
      const keyAttr = [keys.hash, keys.range].find((k) => k?.name === name);
      if (keyAttr) {
        if (typeOf(v) !== keyAttr.type)
          mockError("INVALID_INPUT", `the key attribute ${name} must keep a ${keyAttr.type} value`);
        newKey[name] = v;
      } else sets[name] = v;
    }
    if (keyText(newKey, keys) === keyText(key, keys)) {
      if (Object.keys(sets).length === 0)
        mockError("INVALID_INPUT", "the change sets no attribute");
      const names = Object.keys(sets);
      const set = names.filter((n) => !isMissing(sets[n]!));
      const remove = names.filter((n) => isMissing(sets[n]!));
      const expr = [
        set.length > 0
          ? `SET ${set.map((n) => `#n${names.indexOf(n)} = :v${names.indexOf(n)}`).join(", ")}`
          : "",
        remove.length > 0 ? `REMOVE ${remove.map((n) => `#n${names.indexOf(n)}`).join(", ")}` : "",
      ].filter(Boolean);
      out.push({
        change: i,
        kind: "Update",
        key,
        values: sets,
        text: text("UpdateItem", {
          Key: itemJson(key),
          UpdateExpression: expr.join(" "),
          ConditionExpression: `attribute_exists(#n${names.length})`,
          ExpressionAttributeNames: Object.fromEntries(
            [...names, keys.hash.name].map((n, j) => [`#n${j}`, n]),
          ),
          ExpressionAttributeValues: Object.fromEntries(
            set.map((n) => [`:v${names.indexOf(n)}`, attrJson(sets[n]!)]),
          ),
        }),
      });
      return;
    }
    // A new key: the item is deleted and put again under it, in one transaction.
    const old = t.items.find((x) => keyText(x, keys) === keyText(key, keys));
    if (!old)
      mockError(
        "INVALID_INPUT",
        "a key attribute can only change on an item loaded in the grid: reload the page",
      );
    const item: Item = { ...old, ...newKey };
    for (const [n, v] of Object.entries(sets)) {
      if (isMissing(v)) delete item[n];
      else item[n] = v;
    }
    const body = {
      TransactItems: [
        {
          Delete: {
            TableName: t.name,
            Key: itemJson(key),
            ConditionExpression: "attribute_exists(#n0)",
          },
        },
        {
          Put: {
            TableName: t.name,
            Item: itemJson(item),
            ConditionExpression: "attribute_not_exists(#n0)",
          },
        },
      ],
    };
    const tx = requestText("TransactWriteItems", body);
    out.push({ change: i, kind: "Delete", key, values: {}, text: tx });
    out.push({ change: i, kind: "Put", key: newKey, values: item, text: tx });
  });
  return out;
}

/** Whether a write's condition holds on `items` (the item is there, or not for a put). */
const holds = (items: readonly Item[], w: Write, keys: KeySchema): boolean => {
  const there = items.some((x) => keyText(x, keys) === keyText(w.key, keys));
  return w.kind === "Put" ? !there : there;
};

/** Applies a write to `items` (its condition held). */
function run(items: Item[], w: Write, keys: KeySchema): void {
  const at = items.findIndex((x) => keyText(x, keys) === keyText(w.key, keys));
  if (w.kind === "Delete") items.splice(at, 1);
  else if (w.kind === "Put") items.push({ ...w.values });
  else {
    const next = { ...items[at]! };
    for (const [n, v] of Object.entries(w.values)) {
      if (isMissing(v)) delete next[n];
      else next[n] = v;
    }
    items[at] = next;
  }
}

/** The text Preview shows, once per change (a changed key's transaction once). */
function previews(writes: readonly Write[]): string[] {
  const out: string[] = [];
  let last = -1;
  for (const w of writes)
    if (w.change !== last) {
      out.push(w.text);
      last = w.change;
    }
  return out;
}

// --- the session ------------------------------------------------------------------------------

export function dynamoSession(profile: ConnectionProfile): MockSession {
  const o = optionsOf(profile);
  const region = o.region;

  const refuse = () => mockError("INVALID_INPUT", READ_ONLY_MESSAGE);

  const apply = (req: ApplyRequest): ApplyResult => {
    const started = performance.now();
    const t = tableOf(req.table.name);
    const result = (results: RowResult[], sql: string[]): ApplyResult => {
      const error = results.find((r) => r.error)?.error ?? null;
      return {
        success: error === null,
        results,
        sql,
        error,
        durationMs: Math.max(1, Math.round(performance.now() - started)),
      };
    };
    const row = (i: number, error: ErrorPayload | null): RowResult => {
      const ch = req.changes[i]!;
      return { rowId: ch.rowId, op: ch.op, affected: error ? 0 : 1, returned: null, error };
    };
    if (o.readOnly)
      return result(
        req.changes.map((_, i) => row(i, failure("INVALID_INPUT", READ_ONLY_MESSAGE))),
        [],
      );
    const writes = plan(t, req.changes);
    const sql = previews(writes);
    const keys = t.keys;
    // One change is one request (a changed key's transaction); more are one transaction of at
    // most 100 items, all or nothing, with a reason per change.
    if (req.changes.length === 1) {
      const items = [...t.items];
      const ok = writes.every((w) => {
        if (!holds(items, w, keys)) return false;
        run(items, w, keys);
        return true;
      });
      if (ok) t.items = items;
      return result(
        [
          row(
            0,
            ok ? null : failure("DB_ERROR", CONDITION_FAILED, "ConditionalCheckFailedException"),
          ),
        ],
        sql,
      );
    }
    const seen = new Set<string>();
    for (const w of writes) {
      const k = keyText(w.key, keys);
      if (seen.has(k))
        return result(
          req.changes.map((_, i) =>
            row(
              i,
              failure(
                "INVALID_INPUT",
                "two changes of the same item cannot be applied in one transaction; commit them one at a time",
              ),
            ),
          ),
          sql,
        );
      seen.add(k);
    }
    const items = [...t.items];
    const failedAt = writes.findIndex((w) => !holds(t.items, w, keys));
    if (failedAt < 0) {
      for (const w of writes) run(items, w, keys);
      t.items = items;
      return result(
        req.changes.map((_, i) => row(i, null)),
        sql,
      );
    }
    const failedChanges = new Set(
      writes.filter((w) => !holds(t.items, w, keys)).map((w) => w.change),
    );
    return result(
      req.changes.map((_, i) =>
        row(
          i,
          failedChanges.has(i)
            ? failure("DB_ERROR", CONDITION_FAILED)
            : failure(
                "DB_ERROR",
                `not applied: the transaction was cancelled because item ${failedAt + 1} failed`,
              ),
        ),
      ),
      sql,
    );
  };

  const browseIndex = (args: unknown): BrowsePage => {
    const a = (args ?? {}) as { table?: unknown; index?: unknown; request?: unknown };
    const table = typeof a.table === "string" ? a.table : "";
    const index = typeof a.index === "string" ? a.index : "";
    const req = a.request as BrowseRequest | undefined;
    if (!req || typeof req !== "object")
      mockError("INVALID_INPUT", "browseIndex: missing field `request`");
    const t = tableOf(table);
    const found = [...t.global, ...t.local].find((i) => i.name === index);
    if (!found) mockError("NOT_FOUND", `not found: ${table} has no index “${index}”`);
    // An index holds the items that have its key attributes.
    const items = t.items.filter((i) => keyOf(i, found.keys) !== null);
    return page(t, items, found.keys, req, region, o.sampleSize, index);
  };

  return {
    caps: dynamoCaps(region),
    listDatabases: () => [{ name: region, owner: null, encoding: null, isCurrent: true }],
    listSchemas: () => [{ name: region, isDefault: true }],
    listTables: () =>
      [...tables().keys()].sort().map((name) => ({
        name,
        schema: region,
        kind: "table",
        rowEstimate: null,
        sizeBytes: null,
        comment: null,
      })),
    tableMeta: (table: TableRef) => tableMeta(tableOf(table.name), region, o.sampleSize),
    browse: (req) => {
      const t = tableOf(req.table.name);
      return page(t, t.items, t.keys, req, region, o.sampleSize, null);
    },
    count: (table, filter) => {
      const t = tableOf(table.name);
      if (!filter || filter.items.length === 0) return { count: t.itemCount, exact: false };
      return { count: t.items.filter((i) => matches(i, filter)).length, exact: true };
    },
    fetchCell: (table, key, column) => {
      const t = tableOf(table.name);
      const k = keyText(keyItem(key, t.keys), t.keys);
      const item = t.items.find((i) => keyText(i, t.keys) === k);
      if (!item) mockError("NOT_FOUND", "not found: no item with this key");
      return item[column] ?? MISSING;
    },
    preview: (req) => {
      if (o.readOnly) refuse();
      return previews(plan(tableOf(req.table.name), req.changes));
    },
    apply,
    run: (statement, emit) => runConsole({ readOnly: o.readOnly, region }, statement, emit),
    call: (method, args) => {
      const a = (args ?? {}) as { table?: unknown };
      switch (method) {
        case "tableInfo": {
          const t = tableOf(typeof a.table === "string" ? a.table : "");
          return {
            table: describeTable(t, region),
            timeToLive: t.ttl
              ? { TimeToLiveStatus: "ENABLED", AttributeName: t.ttl }
              : { TimeToLiveStatus: "DISABLED" },
            continuousBackups: {
              ContinuousBackupsStatus: "ENABLED",
              PointInTimeRecoveryDescription: {
                PointInTimeRecoveryStatus: t.pitr ? "ENABLED" : "DISABLED",
              },
            },
          };
        }
        case "browseIndex":
          return browseIndex(args);
        default:
          // Anything else may write (`call_kind`): a read-only session refuses it first.
          if (o.readOnly) refuse();
          return mockError("UNSUPPORTED", `unsupported: DynamoDB: unknown call “${method}”`);
      }
    },
  };
}
