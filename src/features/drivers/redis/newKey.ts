import type { Value } from "@/lib/ipc";
import type { RedisNewValue } from "@/lib/ipc/bindings/RedisNewValue";

/**
 * The New key dialog's first value, from its text as the type takes it: a string or a JSON
 * document whole; one element or member per line for a list or set; `field value` per line for
 * a hash or a stream entry; `score member` per line for a sorted set (ZADD's order). The backend
 * checks scores, JSON and emptiness again (`sqlgen::native::redis::create_key`).
 */

export const NEW_KEY_TYPES: readonly (readonly [string, string])[] = [
  ["string", "String"],
  ["hash", "Hash"],
  ["list", "List"],
  ["set", "Set"],
  ["zset", "Sorted set"],
  ["stream", "Stream"],
  ["json", "RedisJSON document"],
];

/** What the value box asks for, per type. */
export const VALUE_HINT: Record<string, string> = {
  string: "The value",
  hash: "One field per line: field value",
  list: "One element per line",
  set: "One member per line",
  zset: "One member per line: score member",
  stream: "The entry's fields, one per line: field value",
  json: '{"name": "Ada"}',
};

const text = (v: string): Value => ({ t: "text", v });

/** `first rest…` at the first space or tab; the rest may hold spaces. */
function pair(line: string): [string, string] | null {
  const m = /^(\S+)[ \t]+(.*)$/.exec(line);
  return m ? [m[1]!, m[2]!] : null;
}

export function newValueOf(
  type: string,
  input: string,
): { ok: true; value: RedisNewValue } | { ok: false; error: string } {
  const lines = input.split(/\r?\n/).filter((l) => l.trim() !== "");
  const pairs = () => {
    const out: [string, string][] = [];
    for (const l of lines) {
      const p = pair(l.trim());
      if (!p) return null;
      out.push(p);
    }
    return out;
  };
  const needsOne = (what: string) => ({
    ok: false as const,
    error: `A new ${what} needs at least one element (Redis deletes empty ones)`,
  });
  switch (type) {
    case "string":
      return { ok: true, value: { type: "string", value: text(input) } };
    case "json":
      try {
        JSON.parse(input);
      } catch (e) {
        return { ok: false, error: `Not JSON: ${(e as Error).message}` };
      }
      return { ok: true, value: { type: "json", doc: input } };
    case "list":
    case "set": {
      if (lines.length === 0) return needsOne(type);
      const items = lines.map(text);
      return {
        ok: true,
        value: type === "list" ? { type, values: items } : { type, members: items },
      };
    }
    case "hash":
    case "stream": {
      const p = pairs();
      if (!p) return { ok: false, error: "Write each field as: field value" };
      if (p.length === 0) return needsOne(type);
      const fields = p.map(([name, value]) => ({ name: text(name), value: text(value) }));
      return {
        ok: true,
        value: type === "hash" ? { type, fields } : { type, id: null, fields },
      };
    }
    case "zset": {
      const p = pairs();
      if (!p) return { ok: false, error: "Write each member as: score member" };
      if (p.length === 0) return needsOne("sorted set");
      const bad = p.find(
        ([s]) => !/^(?:[+-]?inf|[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)$/i.test(s),
      );
      if (bad) return { ok: false, error: `“${bad[0]}” is not a score (a number, inf or -inf)` };
      return {
        ok: true,
        value: { type, members: p.map(([score, member]) => ({ member: text(member), score })) },
      };
    }
    default:
      return { ok: false, error: `Unknown type ${type}` };
  }
}
