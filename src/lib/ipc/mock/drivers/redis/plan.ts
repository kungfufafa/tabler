import { argvText } from "@/features/drivers/redis/cli";
import type { RedisEdit } from "../../../bindings/RedisEdit";
import type { RedisNewValue } from "../../../bindings/RedisNewValue";
import type { RowKey } from "../../../bindings/RowKey";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { Value } from "../../../bindings/Value";
import { mockError } from "../../transport";
import type { Server } from "./data";
import { exec, type Reply } from "./engine";

/**
 * The mock's edits as the backend plans and applies them (`sqlgen::native::redis` and
 * `drivers/redis/apply.rs`): each change is commands with the reply each must get, behind a guard
 * proved first (the key's type, its absence, or reads of its own); the preview prints `WATCH`,
 * then `MULTI` … `EXEC`; the apply runs a change's commands even when one fails (Redis has no
 * rollback) and reports the change's first failure. A change that cannot be planned fails the
 * whole request, as the backend's plan does.
 */

type Expect =
  | { kind: "any" }
  | { kind: "ok"; msg: string }
  | { kind: "int"; n: number; msg: string }
  | { kind: "positive"; msg: string }
  | { kind: "bytes"; v: string; msg: string };

export interface Cmd {
  argv: string[];
  expect: Expect;
}

export type Guard =
  | { kind: "none" }
  | { kind: "type"; key: string; ty: string }
  | { kind: "absent"; key: string }
  | { kind: "checks"; key: string; checks: Cmd[] };

export interface KeyEdit {
  change: number;
  guard: Guard;
  commands: Cmd[];
}

const GONE = "the key no longer exists (changed by someone else)";
const EXISTS = "a key with this name already exists";
export const CHANGED = "the key was changed by someone else";
const FIELD_CHANGED = "the field changed or no longer exists (changed by someone else)";

const TYPE_STRING = "string";
const TYPE_JSON = "ReJSON-RL";

const any: Expect = { kind: "any" };
const cmd = (argv: string[], expect: Expect = any): Cmd => ({ argv, expect });
const NONE: Guard = { kind: "none" };

const invalid = (message: string): never => mockError("INVALID_INPUT", message);

/** The bytes (as text) a cell is sent as (`value_arg`). */
export function valueArg(v: Value, what: string): string {
  switch (v.t) {
    case "null":
      return invalid(`Redis has no NULL ${what}: set an empty string, or delete the key`);
    case "default":
      return invalid(`Redis has no DEFAULT ${what}`);
    case "bytes":
      if (v.v.truncated)
        return invalid(
          `only a preview of the ${what} is loaded: load the whole value before editing it`,
        );
      return new TextDecoder("utf-8", { ignoreBOM: true }).decode(
        Uint8Array.from(atob(v.v.base64), (c) => c.charCodeAt(0)),
      );
    case "array":
      return invalid(`an array is not a Redis ${what}: edit it as text`);
    case "bool":
      return v.v ? "true" : "false";
    case "int":
    case "float":
      return String(v.v);
    case "other":
      return v.v.text;
    default:
      return v.v;
  }
}

/** A key name from a cell (`key_arg`). */
export const keyArg = (v: Value): string =>
  v.t === "null" ? invalid("a key name is required") : valueArg(v, "key");

/** A TTL cell in seconds as milliseconds; null for no expiry (`ttl_ms`). */
export function ttlMs(v: Value): number | null {
  const bad = () => invalid("a TTL is a number of seconds (empty or -1 for no expiry)");
  let secs: number;
  if (v.t === "null") return null;
  if (v.t === "int" || v.t === "float") secs = v.v;
  else if (v.t === "bigInt" || v.t === "decimal" || v.t === "text") {
    const s = v.v.trim();
    if (s === "") return null;
    secs = /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(s) ? Number(s) : bad();
  } else return bad();
  if (!Number.isFinite(secs)) return bad();
  if (secs === -1) return null;
  const ms = Math.round(secs * 1000);
  if (ms < 1)
    return invalid(
      "a TTL must be positive (empty or -1 for no expiry); delete the key to remove it",
    );
  return ms;
}

const setString = (key: string, value: string) =>
  cmd(["SET", key, value, "XX", "KEEPTTL"], { kind: "ok", msg: GONE });
const setTtl = (key: string, ms: number | null) =>
  ms === null
    ? cmd(["PERSIST", key])
    : cmd(["PEXPIRE", key, String(ms)], { kind: "int", n: 1, msg: GONE });

function jsonSet(key: string, path: string, doc: string): Cmd {
  try {
    JSON.parse(doc);
  } catch {
    invalid("the value is not valid JSON");
  }
  return cmd(["JSON.SET", key, path, doc], { kind: "ok", msg: GONE });
}

function scoreArg(score: string): string {
  const s = score.trim();
  const ok =
    ["inf", "+inf", "-inf"].includes(s.toLowerCase()) ||
    (/^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(s) && Number.isFinite(Number(s)));
  return ok ? s : invalid(`“${score}” is not a score (a number, inf or -inf)`);
}

function streamIdArg(id: string): string {
  const t = id.trim();
  if (t === "*") return t;
  const digits = (s: string) => /^\d{1,20}$/.test(s);
  const [ms, seq] = t.split("-", 2) as [string, string?];
  const ok = t.includes("-") ? digits(ms) && (digits(seq ?? "") || seq === "*") : digits(t);
  return ok ? t : invalid(`“${t}” is not a stream entry id (milliseconds-sequence)`);
}

function streamAdd(key: string, id: string | null, fields: [string, string][]): Cmd {
  if (fields.length === 0) invalid("a stream entry needs at least one field");
  return cmd(["XADD", key, streamIdArg(id ?? "*"), ...fields.flat()]);
}

/** The commands that create `key` (`create_key`). */
export function createKey(
  key: string,
  value: RedisNewValue,
  ttl: number | null,
): Omit<KeyEdit, "change"> {
  const empty = (what: string): never =>
    invalid(`a new ${what} needs at least one element (Redis deletes empty ones)`);
  const commands: Cmd[] = [];
  switch (value.type) {
    case "string":
      return {
        guard: NONE,
        commands: [
          cmd(
            [
              "SET",
              key,
              valueArg(value.value, "value"),
              "NX",
              ...(ttl !== null ? ["PX", String(ttl)] : []),
            ],
            {
              kind: "ok",
              msg: EXISTS,
            },
          ),
        ],
      };
    case "hash":
      if (value.fields.length === 0) empty("hash");
      commands.push(
        cmd([
          "HSET",
          key,
          ...value.fields.flatMap((f) => [valueArg(f.name, "field"), valueArg(f.value, "value")]),
        ]),
      );
      break;
    case "list":
      if (value.values.length === 0) empty("list");
      commands.push(cmd(["RPUSH", key, ...value.values.map((v) => valueArg(v, "element"))]));
      break;
    case "set":
      if (value.members.length === 0) empty("set");
      commands.push(cmd(["SADD", key, ...value.members.map((v) => valueArg(v, "member"))]));
      break;
    case "zset":
      if (value.members.length === 0) empty("sorted set");
      commands.push(
        cmd([
          "ZADD",
          key,
          ...value.members.flatMap((m) => [scoreArg(m.score), valueArg(m.member, "member")]),
        ]),
      );
      break;
    case "stream":
      commands.push(
        streamAdd(
          key,
          value.id,
          value.fields.map((f) => [valueArg(f.name, "field"), valueArg(f.value, "value")]),
        ),
      );
      break;
    case "json":
      commands.push({
        ...jsonSet(key, "$", value.doc),
        argv: ["JSON.SET", key, "$", value.doc, "NX"],
        expect: { kind: "ok", msg: EXISTS },
      });
      break;
  }
  if (ttl !== null) commands.push(setTtl(key, ttl));
  const refusesItself = value.type === "json" && commands.length === 1;
  return { guard: refusesItself ? NONE : { kind: "absent", key }, commands };
}

/** The key and type of a row (`row_key`). */
export function rowKey(key: RowKey): [string, string | null] {
  if (key.kind !== "pk") return invalid("Redis rows are addressed by their key");
  const k = key.value.key ?? invalid("the row has no key");
  const t = key.value.type;
  return [keyArg(k), t && (t.t === "text" || t.t === "enum") ? t.v : null];
}

const missing = (v: Value) => v.t === "default" || (v.t === "other" && v.v.ty === "missing");

function refuseExprs(exprs: Record<string, unknown> | undefined) {
  if (exprs && Object.keys(exprs).length > 0)
    invalid("Redis has no server expressions (NOW(), DEFAULT)");
}

/** One staged change of a key table (`plan_change`). */
export function planChange(index: number, c: StagedChange): KeyEdit {
  if (c.op === "delete") {
    const [k, ty] = rowKey(c.key);
    return {
      change: index,
      guard: ty !== null ? { kind: "type", key: k, ty } : NONE,
      commands: [cmd(["UNLINK", k], { kind: "int", n: 1, msg: GONE })],
    };
  }
  if (c.op === "update") {
    refuseExprs(c.exprs);
    const [k, ty] = rowKey(c.key);
    let value: Value | undefined;
    let ttl: number | null | undefined;
    let rename: string | undefined;
    for (const [column, v] of Object.entries(c.changes)) {
      if (column === "value") value = v;
      else if (column === "ttl") ttl = ttlMs(v);
      else if (column === "key") rename = keyArg(v);
      else if (column === "type" || column === "size" || column === "memory")
        invalid(`the ${column} column is read-only`);
      else invalid(`a Redis key has no column “${column}”`);
    }
    const commands: Cmd[] = [];
    if (value !== undefined) {
      if (ty === TYPE_STRING) commands.push(setString(k, valueArg(value, "value")));
      else if (ty === TYPE_JSON)
        commands.push({
          ...jsonSet(k, "$", valueArg(value, "value")),
          argv: ["JSON.SET", k, "$", valueArg(value, "value"), "XX"],
        });
      else if (ty !== null) invalid(`a ${ty} is edited element by element in the value editor`);
      else invalid("the key's type is not known: reload the page before editing its value");
    }
    if (ttl !== undefined) commands.push(setTtl(k, ttl));
    if (rename !== undefined && rename !== k)
      commands.push(cmd(["RENAMENX", k, rename], { kind: "int", n: 1, msg: EXISTS }));
    return { change: index, guard: ty !== null ? { kind: "type", key: k, ty } : NONE, commands };
  }
  refuseExprs(c.exprs);
  let key: string | null = null;
  let ty: string | null = null;
  let value: Value | undefined;
  let ttl: number | null = null;
  for (const [column, v] of Object.entries(c.values)) {
    if (missing(v)) continue;
    if (column === "key") key = v.t === "null" ? null : keyArg(v);
    else if (column === "type") {
      if (v.t === "text" || v.t === "enum") ty = v.v;
      else if (v.t !== "null") invalid("the type is a name");
    } else if (column === "value") value = v;
    else if (column === "ttl") ttl = ttlMs(v);
    else if (column !== "size" && column !== "memory")
      invalid(`a Redis key has no column “${column}”`);
  }
  if (!key) return invalid("a new key needs a name");
  const type = ty ?? TYPE_STRING;
  let created: Omit<KeyEdit, "change">;
  if (type === TYPE_STRING)
    created = createKey(
      key,
      { type: "string", value: value && value.t !== "null" ? value : { t: "text", v: "" } },
      ttl,
    );
  else if (type === TYPE_JSON)
    created = createKey(
      key,
      { type: "json", doc: value && value.t !== "null" ? valueArg(value, "value") : "{}" },
      ttl,
    );
  else return invalid(`a new ${type} is created with New key, which asks for its first elements`);
  return { change: index, ...created };
}

/** The value editor's edits of one key (`calls::key_edits`). */
export function keyEdits(key: string, ty: string, edits: readonly RedisEdit[]): KeyEdit[] {
  const typed = (commands: Cmd[]): Omit<KeyEdit, "change"> => ({
    guard: { kind: "type", key, ty },
    commands,
  });
  const v = valueArg;
  const fits = (want: string, op: string) => {
    if (want !== ty) invalid(`${op} edits a ${want}, not a ${ty}`);
  };
  const holds = (index: number, old: string): Guard => ({
    kind: "checks",
    key,
    checks: [
      cmd(["LINDEX", key, String(index)], {
        kind: "bytes",
        v: old,
        msg: "the element changed or moved (changed by someone else)",
      }),
    ],
  });
  return edits.map((e, i): KeyEdit => {
    let g: Omit<KeyEdit, "change">;
    switch (e.op) {
      case "stringSet":
        fits(TYPE_STRING, "a string value");
        g = typed([setString(key, v(e.value, "value"))]);
        break;
      case "hashSet":
        fits("hash", "a field");
        g = typed([cmd(["HSET", key, v(e.field, "field"), v(e.value, "value")])]);
        break;
      case "hashAdd":
        fits("hash", "a field");
        g = typed([
          cmd(["HSETNX", key, v(e.field, "field"), v(e.value, "value")], {
            kind: "int",
            n: 1,
            msg: "the field already exists",
          }),
        ]);
        break;
      case "hashDelete":
        fits("hash", "a field");
        g = typed([
          cmd(["HDEL", key, v(e.field, "field")], {
            kind: "int",
            n: 1,
            msg: "the field no longer exists",
          }),
        ]);
        break;
      case "hashRename": {
        fits("hash", "a field");
        const [from, to, value] = [v(e.from, "field"), v(e.to, "field"), v(e.value, "value")];
        if (from === to) invalid("the field already has this name");
        g = {
          guard: {
            kind: "checks",
            key,
            checks: [
              cmd(["HGET", key, from], { kind: "bytes", v: value, msg: FIELD_CHANGED }),
              cmd(["HEXISTS", key, to], { kind: "int", n: 0, msg: "the field already exists" }),
            ],
          },
          commands: [
            cmd(["HSETNX", key, to, value], { kind: "int", n: 1, msg: "the field already exists" }),
            cmd(["HDEL", key, from], { kind: "int", n: 1, msg: "the field no longer exists" }),
          ],
        };
        break;
      }
      case "listSet":
        fits("list", "an element");
        g = {
          guard: holds(e.index, v(e.old, "element")),
          commands: [
            cmd(["LSET", key, String(e.index), v(e.value, "element")], {
              kind: "ok",
              msg: "the element no longer exists",
            }),
          ],
        };
        break;
      case "listPush":
        fits("list", "an element");
        if (e.values.length === 0) invalid("nothing to push");
        g = typed([
          cmd([e.head ? "LPUSH" : "RPUSH", key, ...e.values.map((x) => v(x, "element"))]),
        ]);
        break;
      case "listInsert":
        fits("list", "an element");
        g = typed([
          cmd(
            [
              "LINSERT",
              key,
              e.before ? "BEFORE" : "AFTER",
              v(e.pivot, "element"),
              v(e.value, "element"),
            ],
            {
              kind: "positive",
              msg: "the pivot element no longer exists",
            },
          ),
        ]);
        break;
      case "listDelete": {
        fits("list", "an element");
        const marker = `__tabler_deleted_${crypto.randomUUID()}`;
        g = {
          guard: holds(e.index, v(e.old, "element")),
          commands: [
            cmd(["LSET", key, String(e.index), marker], {
              kind: "ok",
              msg: "the element no longer exists",
            }),
            cmd(["LREM", key, "1", marker], {
              kind: "int",
              n: 1,
              msg: "the element no longer exists",
            }),
          ],
        };
        break;
      }
      case "setAdd":
        fits("set", "a member");
        g = typed([
          cmd(["SADD", key, v(e.member, "member")], {
            kind: "int",
            n: 1,
            msg: "the member already exists",
          }),
        ]);
        break;
      case "setRemove":
        fits("set", "a member");
        g = typed([
          cmd(["SREM", key, v(e.member, "member")], {
            kind: "int",
            n: 1,
            msg: "the member no longer exists",
          }),
        ]);
        break;
      case "setRename": {
        fits("set", "a member");
        const [from, to] = [v(e.from, "member"), v(e.to, "member")];
        if (from === to) invalid("the member already has this name");
        g = {
          guard: {
            kind: "checks",
            key,
            checks: [
              cmd(["SISMEMBER", key, from], {
                kind: "int",
                n: 1,
                msg: "the member no longer exists",
              }),
              cmd(["SISMEMBER", key, to], { kind: "int", n: 0, msg: "the member already exists" }),
            ],
          },
          commands: [
            cmd(["SADD", key, to], { kind: "int", n: 1, msg: "the member already exists" }),
            cmd(["SREM", key, from], { kind: "int", n: 1, msg: "the member no longer exists" }),
          ],
        };
        break;
      }
      case "zsetAdd":
        fits("zset", "a member");
        g = typed([
          cmd(["ZADD", key, "NX", scoreArg(e.score), v(e.member, "member")], {
            kind: "int",
            n: 1,
            msg: "the member already exists",
          }),
        ]);
        break;
      case "zsetScore":
        fits("zset", "a member");
        g = typed([cmd(["ZADD", key, "XX", scoreArg(e.score), v(e.member, "member")])]);
        break;
      case "zsetRemove":
        fits("zset", "a member");
        g = typed([
          cmd(["ZREM", key, v(e.member, "member")], {
            kind: "int",
            n: 1,
            msg: "the member no longer exists",
          }),
        ]);
        break;
      case "streamAdd":
        fits("stream", "an entry");
        g = typed([
          streamAdd(
            key,
            e.id,
            e.fields.map((f) => [v(f.name, "field"), v(f.value, "value")]),
          ),
        ]);
        break;
      case "streamDelete":
        fits("stream", "an entry");
        g = typed([
          cmd(["XDEL", key, streamIdArg(e.id)], {
            kind: "int",
            n: 1,
            msg: "the entry no longer exists",
          }),
        ]);
        break;
      case "jsonSet":
        fits(TYPE_JSON, "a JSON path");
        g = typed([jsonSet(key, e.path, e.doc)]);
        break;
      case "jsonDelete":
        fits(TYPE_JSON, "a JSON path");
        g = typed([cmd(["JSON.DEL", key, e.path])]);
        break;
    }
    return { change: i, ...g };
  });
}

/** The commands a guard reads first, with the reply each must get. */
function guardChecks(g: Guard): Cmd[] {
  switch (g.kind) {
    case "none":
      return [];
    case "type":
      return [cmd(["TYPE", g.key], { kind: "bytes", v: g.ty, msg: CHANGED })];
    case "absent":
      return [cmd(["EXISTS", g.key], { kind: "int", n: 0, msg: EXISTS })];
    case "checks":
      return g.checks;
  }
}

/** The preview lines of a plan, as the apply sends them (`preview`). */
export function preview(plan: readonly KeyEdit[]): string[] {
  const out: string[] = [];
  for (const e of plan) {
    if (e.guard.kind !== "none") out.push(argvText(["WATCH", e.guard.key]));
    const wrap = e.commands.length > 1 || e.guard.kind !== "none";
    if (wrap) out.push("MULTI");
    for (const c of e.commands) out.push(argvText(c.argv));
    if (wrap) out.push("EXEC");
  }
  return out;
}

/** Why a reply is not the one a command expects; null when it is. */
function mismatch(r: Reply, e: Expect): string | null {
  if (r.t === "err") return r.v;
  switch (e.kind) {
    case "any":
      return null;
    case "ok":
      return r.t === "status" && r.v === "OK" ? null : e.msg;
    case "int":
      return r.t === "int" && r.v === e.n ? null : e.msg;
    case "positive":
      return r.t === "int" && r.v > 0 ? null : e.msg;
    case "bytes":
      return (r.t === "bulk" || r.t === "status") && r.v === e.v ? null : e.msg;
  }
}

/**
 * Applies a plan on database `db`: per change its guard's reads (the first that answers
 * otherwise refuses the change, none of it sent), then its commands, all of them (no rollback);
 * the change's error is its first command that failed. Null for a change that applied.
 */
export function applyPlan(s: Server, db: number, plan: readonly KeyEdit[]): (string | null)[] {
  return plan.map((e) => {
    for (const c of guardChecks(e.guard)) {
      const why = mismatch(exec(s, db, c.argv), c.expect);
      if (why) return why;
    }
    let first: string | null = null;
    for (const c of e.commands) {
      const why = mismatch(exec(s, db, c.argv), c.expect);
      first ??= why;
    }
    return first;
  });
}
