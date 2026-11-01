import type { ConnectionProfile } from "@/lib/ipc";
import type { CassandraCompression } from "@/lib/ipc/bindings/CassandraCompression";
import type { CassandraConsistency } from "@/lib/ipc/bindings/CassandraConsistency";
import type { CassandraParams } from "@/lib/ipc/bindings/CassandraParams";
import type { CassandraSerialConsistency } from "@/lib/ipc/bindings/CassandraSerialConsistency";

/**
 * The lane's driver params (`CassandraParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form: anything of the wrong type counts as unset, since the backend's
 * `#[serde(default)]` would refuse it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

/** The consistency levels in cqlsh's order, the default (`LOCAL_ONE`) first. */
export const CONSISTENCIES: readonly CassandraConsistency[] = [
  "LOCAL_ONE",
  "ONE",
  "TWO",
  "THREE",
  "QUORUM",
  "LOCAL_QUORUM",
  "EACH_QUORUM",
  "ALL",
  "ANY",
];

export const SERIAL_CONSISTENCIES: readonly CassandraSerialConsistency[] = [
  "LOCAL_SERIAL",
  "SERIAL",
];

export const COMPRESSIONS: readonly (readonly [CassandraCompression, string])[] = [
  ["none", "None"],
  ["lz4", "LZ4"],
  ["snappy", "Snappy"],
];

/** `params.rs`: the request timeout's and the editor's page size's ranges and defaults. */
export const REQUEST_TIMEOUT = { min: 1, max: 3600, default: 30 } as const;
export const PAGE_SIZE = { min: 1, max: 10_000, default: 100 } as const;

/** The form's view of the params: strings for the text fields ("" when unset). */
export interface CassandraForm {
  consistency: CassandraConsistency;
  serialConsistency: CassandraSerialConsistency;
  localDatacenter: string;
  /** Digits as typed; "" for 30 s. */
  requestTimeoutSecs: string;
  /** Digits as typed; "" for 100 rows. */
  pageSize: string;
  compression: CassandraCompression;
  singleNode: boolean;
}

const pick = <T extends string>(all: readonly T[], v: unknown, fallback: T): T =>
  typeof v === "string" && (all as readonly string[]).includes(v) ? (v as T) : fallback;

const digits = (v: unknown): string =>
  typeof v === "number" && Number.isInteger(v) ? String(v) : typeof v === "string" ? v.trim() : "";

export function cassandraParamsOf(params: Params): CassandraForm {
  const dc = params?.localDatacenter;
  return {
    consistency: pick(CONSISTENCIES, params?.consistency, "LOCAL_ONE"),
    serialConsistency: pick(SERIAL_CONSISTENCIES, params?.serialConsistency, "LOCAL_SERIAL"),
    localDatacenter: typeof dc === "string" ? dc.trim() : "",
    requestTimeoutSecs: digits(params?.requestTimeoutSecs),
    pageSize: digits(params?.pageSize),
    compression: pick(
      COMPRESSIONS.map(([c]) => c),
      params?.compression,
      "none",
    ),
    singleNode: params?.singleNode === true,
  };
}

export const cassandraParams = (p: ConnectionProfile): CassandraForm =>
  cassandraParamsOf(p.options.driverParams);

/** A whole number within `range`, or null. */
export function inRange(text: string, range: { min: number; max: number }): number | null {
  if (!/^\d+$/.test(text)) return null;
  const n = Number(text);
  return n >= range.min && n <= range.max ? n : null;
}

/**
 * The params a profile keeps: only what differs from the backend's defaults, so a profile with
 * none stores none. Numbers are numbers (the form checks their digits first).
 */
export function toCassandraParams(f: CassandraForm): Partial<CassandraParams> | null {
  const out: Partial<CassandraParams> = {};
  if (f.consistency !== "LOCAL_ONE") out.consistency = f.consistency;
  if (f.serialConsistency !== "LOCAL_SERIAL") out.serialConsistency = f.serialConsistency;
  if (f.localDatacenter !== "") out.localDatacenter = f.localDatacenter;
  const timeout = inRange(f.requestTimeoutSecs, REQUEST_TIMEOUT);
  if (timeout !== null) out.requestTimeoutSecs = timeout;
  const size = inRange(f.pageSize, PAGE_SIZE);
  if (size !== null) out.pageSize = size;
  if (f.compression !== "none") out.compression = f.compression;
  if (f.singleNode) out.singleNode = true;
  return Object.keys(out).length > 0 ? out : null;
}
