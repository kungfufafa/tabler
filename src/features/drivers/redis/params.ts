import type { ConnectionProfile } from "@/lib/ipc";
import type { RedisParams } from "@/lib/ipc/bindings/RedisParams";
import type { RedisProtocol } from "@/lib/ipc/bindings/RedisProtocol";

/**
 * `RedisParams` (`drivers/redis/params.rs`) read leniently from a profile or the form, and
 * written back with only what differs from the backend's defaults: a value of the wrong type
 * counts as unset, as the backend's `#[serde(default)]` would refuse it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

export const PROTOCOLS: readonly (readonly [RedisProtocol, string])[] = [
  ["auto", "Automatic (RESP3, or RESP2 before Redis 6)"],
  ["resp3", "RESP3"],
  ["resp2", "RESP2"],
];

/** The backend's bounds (`params.rs`): `MAX_SCAN_COUNT`, `MAX_PREVIEW_BYTES`, `MAX_PREVIEW_ELEMENTS`. */
export const MAX_SCAN_COUNT = 100_000;
export const MAX_PREVIEW_BYTES = 16 * 1024 * 1024;
export const MAX_PREVIEW_ELEMENTS = 10_000;
/** The backend's defaults (`keys::Limits`) and the key tree's separator. */
export const DEFAULT_PREVIEW_BYTES = 16 * 1024;
export const DEFAULT_PREVIEW_ELEMENTS = 50;
export const DEFAULT_SEPARATOR = ":";
/** `info::MAX_DATABASES`: the highest DB index is one less. */
export const MAX_DATABASES = 4096;

/** The form's view of the params: text fields as text ("" when unset). */
export interface RedisForm {
  scanCount: string;
  previewBytes: string;
  previewElements: string;
  memoryColumn: boolean;
  keySeparator: string;
  protocol: RedisProtocol;
}

const num = (p: Params, key: string): string => {
  const v = p?.[key];
  return typeof v === "number" ? String(v) : typeof v === "string" ? v.trim() : "";
};

export function redisFormOf(p: Params): RedisForm {
  const protocol = PROTOCOLS.find(([id]) => id === p?.protocol)?.[0] ?? "auto";
  const sep = p?.keySeparator;
  return {
    scanCount: num(p, "scanCount"),
    previewBytes: num(p, "previewBytes"),
    previewElements: num(p, "previewElements"),
    memoryColumn: p?.memoryColumn !== false,
    // Not trimmed: a space may separate the levels.
    keySeparator: typeof sep === "string" ? sep : "",
    protocol,
  };
}

export const redisForm = (p: ConnectionProfile): RedisForm => redisFormOf(p.options.driverParams);

/** A whole number of the form ("" unset; NaN when it is not one). */
export function wholeNumber(text: string): number | null {
  if (text === "") return null;
  return /^\d+$/.test(text) ? Number(text) : NaN;
}

/** The params a profile keeps: only what differs from the defaults (null: none). */
export function toRedisParams(f: RedisForm): Partial<RedisParams> | null {
  const out: Partial<RedisParams> = {};
  const n = (text: string) => {
    const v = wholeNumber(text);
    return v === null || Number.isNaN(v) ? null : v;
  };
  const scan = n(f.scanCount);
  if (scan !== null) out.scanCount = scan;
  const bytes = n(f.previewBytes);
  if (bytes !== null) out.previewBytes = bytes;
  const elements = n(f.previewElements);
  if (elements !== null) out.previewElements = elements;
  if (!f.memoryColumn) out.memoryColumn = false;
  if (f.keySeparator !== "" && f.keySeparator !== DEFAULT_SEPARATOR)
    out.keySeparator = f.keySeparator;
  if (f.protocol !== "auto") out.protocol = f.protocol;
  return Object.keys(out).length > 0 ? out : null;
}

/** The form's params as `ConnectionFormValues.driverParams` holds them. */
export const formParams = (f: RedisForm): Record<string, unknown> => ({ ...f });

/** The key tree's separator of a profile. */
export const keySeparator = (p: ConnectionProfile): string =>
  redisForm(p).keySeparator || DEFAULT_SEPARATOR;

/**
 * The DB index of the Database field (`params::database_index`): `3` or the sidebar's `db3`,
 * 0 when empty; null when it is not one.
 */
export function databaseIndex(text: string | null | undefined): number | null {
  const t = (text ?? "").trim();
  if (t === "") return 0;
  const digits = t.startsWith("db") ? t.slice(2) : t;
  // Rust's `u32` parse takes a leading `+`.
  if (!/^\+?\d+$/.test(digits)) return null;
  const n = Number(digits);
  return n < MAX_DATABASES ? n : null;
}
