import { globMatch, memoryOf, sizeOf, zsetOrder, type Entry, type Server } from "./data";

/**
 * The commands the mock's server answers, on one database, with Redis's replies (RESP2 shapes):
 * the console sends them, and the grid's edits, the value editor and New key go through them
 * too, so every path changes the same data the way Redis would. A command it does not know is
 * Redis's own "unknown command" error.
 */

export type Reply =
  | { t: "nil" }
  | { t: "int"; v: number }
  | { t: "bulk"; v: string }
  | { t: "status"; v: string }
  | { t: "err"; v: string }
  | { t: "arr"; v: Reply[] };

export const nil: Reply = { t: "nil" };
export const OK: Reply = { t: "status", v: "OK" };
export const int = (v: number): Reply => ({ t: "int", v });
export const bulk = (v: string): Reply => ({ t: "bulk", v });
export const arr = (v: Reply[]): Reply => ({ t: "arr", v });
export const err = (v: string): Reply => ({ t: "err", v });

const WRONGTYPE = err("WRONGTYPE Operation against a key holding the wrong kind of value");
const SYNTAX = err("ERR syntax error");
const NOT_INT = err("ERR value is not an integer or out of range");

const arity = (name: string) =>
  err(`ERR wrong number of arguments for '${name.toLowerCase()}' command`);

/** Redis's `LRANGE` / `GETRANGE` index rules: negative from the end, clamped. */
function range(len: number, start: number, stop: number): [number, number] {
  let a = start < 0 ? Math.max(len + start, 0) : start;
  let b = stop < 0 ? len + stop : Math.min(stop, len - 1);
  if (a > b || a >= len) return [0, -1];
  a = Math.max(a, 0);
  b = Math.max(b, 0);
  return [a, b];
}

const isInt = (s: string | undefined) => s !== undefined && /^-?\d+$/.test(s);

/** A score as `ZADD` takes it. */
function score(s: string): number | null {
  const t = s.trim().toLowerCase();
  if (t === "inf" || t === "+inf") return Infinity;
  if (t === "-inf") return -Infinity;
  return /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/.test(t) ? Number(t) : null;
}

const scoreText = (n: number) => (n === Infinity ? "inf" : n === -Infinity ? "-inf" : String(n));

/** The next stream id after the last one (`*`), or the given one when it is newer. */
function streamId(entries: { id: string }[], given: string): string | null {
  const last = entries.at(-1)?.id ?? "0-0";
  const [lms, lseq] = last.split("-").map(Number) as [number, number];
  if (given === "*") {
    const now = Math.max(Date.now(), lms);
    return now === lms ? `${lms}-${lseq + 1}` : `${now}-0`;
  }
  const m = /^(\d+)(?:-(\d+|\*))?$/.exec(given);
  if (!m) return null;
  const ms = Number(m[1]);
  const seq = m[2] === "*" ? (ms === lms ? lseq + 1 : 0) : Number(m[2] ?? 0);
  return ms > lms || (ms === lms && seq > lseq) ? `${ms}-${seq}` : null;
}

/** The INFO text of the server's sections (`# Name` then `field:value` lines). */
export function infoText(s: Server, section?: string): string {
  const keyspace = [...s.dbs]
    .filter(([, db]) => db.size > 0)
    .map(([i, db]) => {
      const expires = [...db.values()].filter((e) => e.pttl !== null).length;
      return `db${i}:keys=${db.size},expires=${expires},avg_ttl=0`;
    });
  const sections: [string, string[]][] = [
    [
      "Server",
      [
        `redis_version:${s.version}`,
        `redis_mode:${s.mode}`,
        "os:Linux 6.8.0 x86_64",
        "tcp_port:6379",
        "uptime_in_seconds:86400",
      ],
    ],
    ["Clients", [`connected_clients:${s.clients.length + 1}`, "blocked_clients:1"]],
    ["Memory", ["used_memory:1048576", "used_memory_human:1.00M", "maxmemory:0"]],
    ["Stats", ["total_connections_received:42", "total_commands_processed:4821"]],
    ["Replication", ["role:master", "connected_slaves:0"]],
    ["Keyspace", keyspace],
  ];
  const want = section?.toLowerCase();
  return sections
    .filter(
      ([name]) => !want || want === "all" || want === "everything" || name.toLowerCase() === want,
    )
    .map(([name, lines]) => [`# ${name}`, ...lines].join("\r\n"))
    .join("\r\n\r\n");
}

/** Runs `argv` on database `db` of `s` (SELECT, MULTI and the like are the caller's). */
export function exec(s: Server, db: number, argv: string[]): Reply {
  const keys = s.dbs.get(db) ?? new Map<string, Entry>();
  const name = (argv[0] ?? "").toUpperCase();
  const a = argv.slice(1);
  const k = a[0] ?? "";
  const get = (key: string) => keys.get(key);
  const typed = <T extends Entry["type"]>(key: string, type: T) => {
    const e = get(key);
    if (e && e.type !== type) return "wrong" as const;
    return e as Extract<Entry, { type: T }> | undefined;
  };
  const need = (n: number) => a.length < n;

  switch (name) {
    case "PING":
      return a.length > 0 ? bulk(a[0]!) : { t: "status", v: "PONG" };
    case "ECHO":
      return need(1) ? arity(name) : bulk(a[0]!);
    case "TIME": {
      const ms = Date.now();
      return arr([bulk(String(Math.floor(ms / 1000))), bulk(String((ms % 1000) * 1000))]);
    }
    case "DBSIZE":
      return int(keys.size);
    case "INFO":
      return bulk(infoText(s, a[0]));
    case "TYPE":
      return need(1) ? arity(name) : { t: "status", v: get(k)?.type ?? "none" };
    case "EXISTS":
      return need(1) ? arity(name) : int(a.filter((x) => keys.has(x)).length);
    case "TTL":
    case "PTTL": {
      if (need(1)) return arity(name);
      const e = get(k);
      if (!e) return int(-2);
      if (e.pttl === null) return int(-1);
      return int(name === "PTTL" ? e.pttl : Math.floor((e.pttl + 500) / 1000));
    }
    case "EXPIRE":
    case "PEXPIRE": {
      if (need(2) || !isInt(a[1])) return need(2) ? arity(name) : NOT_INT;
      const e = get(k);
      if (!e) return int(0);
      const n = Number(a[1]);
      const ms = name === "EXPIRE" ? n * 1000 : n;
      if (ms <= 0) keys.delete(k);
      else e.pttl = ms;
      return int(1);
    }
    case "PERSIST": {
      const e = get(k);
      if (!e || e.pttl === null) return int(0);
      e.pttl = null;
      return int(1);
    }
    case "DEL":
    case "UNLINK":
      return int(a.filter((x) => keys.delete(x)).length);
    case "RENAME":
    case "RENAMENX": {
      if (need(2)) return arity(name);
      const e = get(k);
      if (!e) return err("ERR no such key");
      if (name === "RENAMENX" && keys.has(a[1]!)) return int(0);
      keys.delete(k);
      keys.set(a[1]!, e);
      return name === "RENAME" ? OK : int(1);
    }
    case "COPY": {
      if (need(2)) return arity(name);
      const e = get(k);
      if (!e || keys.has(a[1]!)) return int(0);
      keys.set(a[1]!, structuredClone(e));
      return int(1);
    }
    case "FLUSHDB":
      keys.clear();
      return OK;
    case "FLUSHALL":
      for (const d of s.dbs.values()) d.clear();
      return OK;
    case "KEYS":
      return need(1) ? arity(name) : arr([...keys.keys()].filter((x) => globMatch(k, x)).map(bulk));
    case "RANDOMKEY": {
      const first = keys.keys().next();
      return first.done ? nil : bulk(first.value);
    }
    case "SCAN": {
      if (!isInt(a[0])) return err("ERR invalid cursor");
      const all = [...keys.keys()];
      let at = Number(a[0]);
      let count = 10;
      let match: string | null = null;
      let type: string | null = null;
      for (let i = 1; i < a.length; i += 2) {
        const opt = a[i]!.toUpperCase();
        const v = a[i + 1];
        if (v === undefined) return SYNTAX;
        if (opt === "MATCH") match = v;
        else if (opt === "COUNT" && isInt(v)) count = Math.max(1, Number(v));
        else if (opt === "TYPE") type = v;
        else return SYNTAX;
      }
      const out: Reply[] = [];
      const end = Math.min(all.length, at + count);
      for (; at < end; at++) {
        const key = all[at]!;
        if (match !== null && !globMatch(match, key)) continue;
        if (type !== null && keys.get(key)?.type !== type) continue;
        out.push(bulk(key));
      }
      return arr([bulk(at >= all.length ? "0" : String(at)), arr(out)]);
    }
    case "MEMORY": {
      if ((a[0] ?? "").toUpperCase() !== "USAGE") return err("ERR unknown subcommand");
      const e = get(a[1] ?? "");
      return e ? int(memoryOf(a[1]!, e)) : nil;
    }
    case "OBJECT": {
      const e = get(a[1] ?? "");
      if (!e) return nil;
      const enc: Record<string, string> = {
        string: "embstr",
        hash: "listpack",
        list: "quicklist",
        set: "listpack",
        zset: "listpack",
        stream: "stream",
      };
      return bulk(enc[e.type] ?? "raw");
    }
    case "CONFIG":
      if ((a[0] ?? "").toUpperCase() === "GET" && globMatch(a[1] ?? "", "databases"))
        return arr([bulk("databases"), bulk(String(s.databases))]);
      return (a[0] ?? "").toUpperCase() === "GET" ? arr([]) : OK;
    case "CLIENT": {
      const sub = (a[0] ?? "").toUpperCase();
      if (sub === "LIST")
        return bulk(
          s.clients
            .map(
              (c) =>
                `id=${c.id} addr=${c.addr} name=${c.name} age=60 idle=${c.idle} flags=${c.flags} db=${c.db} cmd=${c.cmd} user=${c.user}`,
            )
            .join("\n"),
        );
      if (sub === "ID") return int(1);
      if (sub === "SETNAME" || sub === "SETINFO") return OK;
      return err(`ERR unknown subcommand '${a[0] ?? ""}'`);
    }
    case "SLOWLOG": {
      const sub = (a[0] ?? "").toUpperCase();
      if (sub === "LEN") return int(s.slowlog.length);
      if (sub === "RESET") {
        s.slowlog = [];
        return OK;
      }
      return arr(
        s.slowlog.map((e) =>
          arr([
            int(e.id),
            int(e.at),
            int(e.us),
            arr(e.argv.map(bulk)),
            bulk(e.client),
            bulk(e.name),
          ]),
        ),
      );
    }
    case "COMMAND":
      return int(240);

    // Strings.
    case "GET": {
      const e = typed(k, "string");
      return e === "wrong" ? WRONGTYPE : e ? bulk(e.value) : nil;
    }
    case "MGET":
      return arr(
        a.map((x) => (get(x)?.type === "string" ? bulk((get(x) as { value: string }).value) : nil)),
      );
    case "SET": {
      if (need(2)) return arity(name);
      let nx = false;
      let xx = false;
      let keep = false;
      let ttl: number | null = null;
      for (let i = 2; i < a.length; i++) {
        const opt = a[i]!.toUpperCase();
        if (opt === "NX") nx = true;
        else if (opt === "XX") xx = true;
        else if (opt === "KEEPTTL") keep = true;
        else if ((opt === "EX" || opt === "PX") && isInt(a[i + 1])) {
          ttl = Number(a[++i]) * (opt === "EX" ? 1000 : 1);
        } else return SYNTAX;
      }
      const e = get(k);
      if ((nx && e) || (xx && !e)) return nil;
      keys.set(k, { type: "string", value: a[1]!, pttl: ttl ?? (keep && e ? e.pttl : null) });
      return OK;
    }
    case "STRLEN": {
      const e = typed(k, "string");
      return e === "wrong" ? WRONGTYPE : int(e ? sizeOf(e)! : 0);
    }
    case "GETRANGE": {
      const e = typed(k, "string");
      if (e === "wrong") return WRONGTYPE;
      if (!isInt(a[1]) || !isInt(a[2])) return NOT_INT;
      const bytes = new TextEncoder().encode(e?.value ?? "");
      const [from, to] = range(bytes.length, Number(a[1]), Number(a[2]));
      return bulk(
        new TextDecoder("utf-8", { ignoreBOM: true }).decode(bytes.subarray(from, to + 1)),
      );
    }
    case "APPEND": {
      const e = typed(k, "string");
      if (e === "wrong") return WRONGTYPE;
      const value = (e?.value ?? "") + (a[1] ?? "");
      keys.set(k, { type: "string", value, pttl: e?.pttl ?? null });
      return int(new TextEncoder().encode(value).length);
    }
    case "INCR":
    case "DECR":
    case "INCRBY":
    case "DECRBY": {
      const e = typed(k, "string");
      if (e === "wrong") return WRONGTYPE;
      const by = name.endsWith("BY") ? a[1] : "1";
      if (!isInt(by) || (e && !isInt(e.value))) return NOT_INT;
      const n = Number(e?.value ?? 0) + (name.startsWith("DECR") ? -1 : 1) * Number(by);
      keys.set(k, { type: "string", value: String(n), pttl: e?.pttl ?? null });
      return int(n);
    }

    // Hashes.
    case "HGET":
    case "HEXISTS":
    case "HSTRLEN": {
      const e = typed(k, "hash");
      if (e === "wrong") return WRONGTYPE;
      const v = e?.value.get(a[1] ?? "");
      if (name === "HEXISTS") return int(v === undefined ? 0 : 1);
      if (name === "HSTRLEN") return int(v === undefined ? 0 : new TextEncoder().encode(v).length);
      return v === undefined ? nil : bulk(v);
    }
    case "HSET":
    case "HSETNX":
    case "HMSET": {
      if (a.length < 3 || (name !== "HSETNX" && a.length % 2 === 0)) return arity(name);
      const e = typed(k, "hash");
      if (e === "wrong") return WRONGTYPE;
      const h = e ?? { type: "hash" as const, value: new Map<string, string>(), pttl: null };
      if (name === "HSETNX" && h.value.has(a[1]!)) return int(0);
      let added = 0;
      for (let i = 1; i + 1 < a.length; i += 2) {
        if (!h.value.has(a[i]!)) added++;
        h.value.set(a[i]!, a[i + 1]!);
      }
      keys.set(k, h);
      return name === "HMSET" ? OK : int(added);
    }
    case "HDEL": {
      const e = typed(k, "hash");
      if (e === "wrong") return WRONGTYPE;
      const n = a.slice(1).filter((f) => e?.value.delete(f)).length;
      if (e && e.value.size === 0) keys.delete(k);
      return int(n);
    }
    case "HGETALL":
    case "HKEYS":
    case "HVALS":
    case "HLEN": {
      const e = typed(k, "hash");
      if (e === "wrong") return WRONGTYPE;
      const pairs = [...(e?.value ?? new Map<string, string>())];
      if (name === "HLEN") return int(pairs.length);
      if (name === "HKEYS") return arr(pairs.map(([f]) => bulk(f)));
      if (name === "HVALS") return arr(pairs.map(([, v]) => bulk(v)));
      return arr(pairs.flatMap(([f, v]) => [bulk(f), bulk(v)]));
    }
    case "HINCRBY": {
      const e = typed(k, "hash");
      if (e === "wrong") return WRONGTYPE;
      const h = e ?? { type: "hash" as const, value: new Map<string, string>(), pttl: null };
      const old = h.value.get(a[1] ?? "") ?? "0";
      if (!isInt(old) || !isInt(a[2])) return NOT_INT;
      const n = Number(old) + Number(a[2]);
      h.value.set(a[1]!, String(n));
      keys.set(k, h);
      return int(n);
    }

    // Lists.
    case "LPUSH":
    case "RPUSH": {
      if (need(2)) return arity(name);
      const e = typed(k, "list");
      if (e === "wrong") return WRONGTYPE;
      const l = e ?? { type: "list" as const, value: [] as string[], pttl: null };
      for (const v of a.slice(1)) {
        if (name === "LPUSH") l.value.unshift(v);
        else l.value.push(v);
      }
      keys.set(k, l);
      return int(l.value.length);
    }
    case "LPOP":
    case "RPOP": {
      const e = typed(k, "list");
      if (e === "wrong") return WRONGTYPE;
      if (!e) return nil;
      const v = name === "LPOP" ? e.value.shift() : e.value.pop();
      if (e.value.length === 0) keys.delete(k);
      return v === undefined ? nil : bulk(v);
    }
    case "LLEN":
    case "LRANGE":
    case "LINDEX": {
      const e = typed(k, "list");
      if (e === "wrong") return WRONGTYPE;
      const l = e?.value ?? [];
      if (name === "LLEN") return int(l.length);
      if (name === "LINDEX") {
        if (!isInt(a[1])) return NOT_INT;
        const i = Number(a[1]);
        const v = l[i < 0 ? l.length + i : i];
        return v === undefined ? nil : bulk(v);
      }
      if (!isInt(a[1]) || !isInt(a[2])) return NOT_INT;
      const [from, to] = range(l.length, Number(a[1]), Number(a[2]));
      return arr(l.slice(from, to + 1).map(bulk));
    }
    case "LSET": {
      const e = typed(k, "list");
      if (e === "wrong") return WRONGTYPE;
      if (!e) return err("ERR no such key");
      if (!isInt(a[1])) return NOT_INT;
      const i = Number(a[1]);
      const at = i < 0 ? e.value.length + i : i;
      if (at < 0 || at >= e.value.length) return err("ERR index out of range");
      e.value[at] = a[2] ?? "";
      return OK;
    }
    case "LREM": {
      const e = typed(k, "list");
      if (e === "wrong") return WRONGTYPE;
      if (!e) return int(0);
      if (!isInt(a[1])) return NOT_INT;
      let left = Number(a[1]) === 0 ? Infinity : Math.abs(Number(a[1]));
      const fromTail = Number(a[1]) < 0;
      const order = fromTail ? [...e.value.keys()].reverse() : [...e.value.keys()];
      const drop = new Set<number>();
      for (const i of order) {
        if (left === 0) break;
        if (e.value[i] === a[2]) {
          drop.add(i);
          left--;
        }
      }
      e.value = e.value.filter((_, i) => !drop.has(i));
      if (e.value.length === 0) keys.delete(k);
      return int(drop.size);
    }
    case "LINSERT": {
      const e = typed(k, "list");
      if (e === "wrong") return WRONGTYPE;
      if (!e) return int(0);
      const at = e.value.indexOf(a[2] ?? "");
      if (at < 0) return int(-1);
      e.value.splice((a[1] ?? "").toUpperCase() === "BEFORE" ? at : at + 1, 0, a[3] ?? "");
      return int(e.value.length);
    }

    // Sets.
    case "SADD": {
      if (need(2)) return arity(name);
      const e = typed(k, "set");
      if (e === "wrong") return WRONGTYPE;
      const set = e ?? { type: "set" as const, value: new Set<string>(), pttl: null };
      const before = set.value.size;
      for (const m of a.slice(1)) set.value.add(m);
      keys.set(k, set);
      return int(set.value.size - before);
    }
    case "SREM": {
      const e = typed(k, "set");
      if (e === "wrong") return WRONGTYPE;
      const n = a.slice(1).filter((m) => e?.value.delete(m)).length;
      if (e && e.value.size === 0) keys.delete(k);
      return int(n);
    }
    case "SMEMBERS":
    case "SCARD":
    case "SISMEMBER": {
      const e = typed(k, "set");
      if (e === "wrong") return WRONGTYPE;
      const set = e?.value ?? new Set<string>();
      if (name === "SCARD") return int(set.size);
      if (name === "SISMEMBER") return int(set.has(a[1] ?? "") ? 1 : 0);
      return arr([...set].map(bulk));
    }
    case "SSCAN":
    case "HSCAN": {
      const e = typed(k, name === "SSCAN" ? "set" : "hash");
      if (e === "wrong") return WRONGTYPE;
      if (!isInt(a[1])) return err("ERR invalid cursor");
      const count = a.findIndex((x) => x.toUpperCase() === "COUNT");
      const n = count >= 0 && isInt(a[count + 1]) ? Number(a[count + 1]) : 10;
      const items: string[][] =
        e === undefined
          ? []
          : e.type === "set"
            ? [...e.value].map((m) => [m])
            : [...e.value].map(([f, v]) =>
                a.some((x) => x.toUpperCase() === "NOVALUES") ? [f] : [f, v],
              );
      const from = Number(a[1]);
      const page = items.slice(from, from + n);
      const next = from + n >= items.length ? "0" : String(from + n);
      return arr([bulk(next), arr(page.flat().map(bulk))]);
    }

    // Sorted sets.
    case "ZADD": {
      const e = typed(k, "zset");
      if (e === "wrong") return WRONGTYPE;
      let i = 1;
      let nx = false;
      let xx = false;
      for (; i < a.length; i++) {
        const opt = a[i]!.toUpperCase();
        if (opt === "NX") nx = true;
        else if (opt === "XX") xx = true;
        else break;
      }
      const rest = a.slice(i);
      if (rest.length === 0 || rest.length % 2 !== 0) return SYNTAX;
      const z = e ?? { type: "zset" as const, value: new Map<string, number>(), pttl: null };
      let added = 0;
      for (let j = 0; j < rest.length; j += 2) {
        const sc = score(rest[j]!);
        if (sc === null) return err("ERR value is not a valid float");
        const m = rest[j + 1]!;
        const has = z.value.has(m);
        if ((nx && has) || (xx && !has)) continue;
        if (!has) added++;
        z.value.set(m, sc);
      }
      if (z.value.size > 0) keys.set(k, z);
      return int(added);
    }
    case "ZREM": {
      const e = typed(k, "zset");
      if (e === "wrong") return WRONGTYPE;
      const n = a.slice(1).filter((m) => e?.value.delete(m)).length;
      if (e && e.value.size === 0) keys.delete(k);
      return int(n);
    }
    case "ZSCORE": {
      const e = typed(k, "zset");
      if (e === "wrong") return WRONGTYPE;
      const v = e?.value.get(a[1] ?? "");
      return v === undefined ? nil : bulk(scoreText(v));
    }
    case "ZCARD": {
      const e = typed(k, "zset");
      return e === "wrong" ? WRONGTYPE : int(e?.value.size ?? 0);
    }
    case "ZRANGE":
    case "ZREVRANGE": {
      const e = typed(k, "zset");
      if (e === "wrong") return WRONGTYPE;
      if (!isInt(a[1]) || !isInt(a[2])) return NOT_INT;
      let all = zsetOrder(e?.value ?? new Map());
      if (name === "ZREVRANGE" || a.some((x) => x.toUpperCase() === "REV")) all = all.reverse();
      const [from, to] = range(all.length, Number(a[1]), Number(a[2]));
      const page = all.slice(from, to + 1);
      return a.some((x) => x.toUpperCase() === "WITHSCORES")
        ? arr(page.flatMap(([m, sc]) => [bulk(m), bulk(scoreText(sc))]))
        : arr(page.map(([m]) => bulk(m)));
    }

    // Streams.
    case "XADD": {
      if (a.length < 4 || a.length % 2 !== 0) return arity(name);
      const e = typed(k, "stream");
      if (e === "wrong") return WRONGTYPE;
      const st = e ?? {
        type: "stream" as const,
        value: [] as { id: string; fields: [string, string][] }[],
        pttl: null,
      };
      const id = streamId(st.value, a[1]!);
      if (id === null)
        return err(
          "ERR The ID specified in XADD is equal or smaller than the target stream top item",
        );
      const fields: [string, string][] = [];
      for (let i = 2; i < a.length; i += 2) fields.push([a[i]!, a[i + 1]!]);
      st.value.push({ id, fields });
      keys.set(k, st);
      return bulk(id);
    }
    case "XDEL": {
      const e = typed(k, "stream");
      if (e === "wrong") return WRONGTYPE;
      if (!e) return int(0);
      const before = e.value.length;
      e.value = e.value.filter((x) => !a.slice(1).includes(x.id));
      return int(before - e.value.length);
    }
    case "XLEN": {
      const e = typed(k, "stream");
      return e === "wrong" ? WRONGTYPE : int(e?.value.length ?? 0);
    }
    case "XRANGE":
    case "XREVRANGE": {
      const e = typed(k, "stream");
      if (e === "wrong") return WRONGTYPE;
      const rev = name === "XREVRANGE";
      const [lo, hi] = rev ? [a[2], a[1]] : [a[1], a[2]];
      const cmp = (x: string, y: string) => {
        const [xa, xb] = x.split("-").map(Number) as [number, number];
        const [ya, yb = 0] = y.split("-").map(Number) as [number, number?];
        return xa - ya || xb - yb;
      };
      const ci = a.findIndex((x) => x.toUpperCase() === "COUNT");
      const count = ci >= 0 && isInt(a[ci + 1]) ? Number(a[ci + 1]) : Infinity;
      let items = (e?.value ?? []).filter(
        (x) =>
          (lo === "-" || cmp(x.id, lo ?? "0") >= 0) && (hi === "+" || cmp(x.id, hi ?? "0") <= 0),
      );
      if (rev) items = items.reverse();
      return arr(
        items
          .slice(0, count)
          .map((x) => arr([bulk(x.id), arr(x.fields.flatMap(([f, v]) => [bulk(f), bulk(v)]))])),
      );
    }

    // RedisJSON (whole documents and `$` only).
    case "JSON.GET": {
      const e = typed(k, "ReJSON-RL");
      return e === "wrong" ? WRONGTYPE : e ? bulk(e.value) : nil;
    }
    case "JSON.TYPE": {
      const e = typed(k, "ReJSON-RL");
      if (e === "wrong") return WRONGTYPE;
      if (!e) return nil;
      const v: unknown = JSON.parse(e.value);
      return bulk(Array.isArray(v) ? "array" : v === null ? "null" : typeof v);
    }
    case "JSON.SET": {
      if (a.length < 3) return arity(name);
      if (a[1] !== "$" && a[1] !== ".") return err("ERR the mock sets whole documents only ($)");
      const e = typed(k, "ReJSON-RL");
      if (e === "wrong") return WRONGTYPE;
      try {
        JSON.parse(a[2]!);
      } catch {
        return err("ERR expected value");
      }
      const opt = (a[3] ?? "").toUpperCase();
      if ((opt === "NX" && e) || (opt === "XX" && !e)) return nil;
      keys.set(k, { type: "ReJSON-RL", value: a[2]!, pttl: e?.pttl ?? null });
      return OK;
    }
    case "JSON.DEL": {
      const e = typed(k, "ReJSON-RL");
      if (e === "wrong") return WRONGTYPE;
      if (!e || (a[1] !== undefined && a[1] !== "$")) return int(0);
      keys.delete(k);
      return int(1);
    }
    default:
      return err(
        `ERR unknown command '${argv[0] ?? ""}', with args beginning with: ${a
          .slice(0, 3)
          .map((x) => `'${x}' `)
          .join("")}`,
      );
  }
}
