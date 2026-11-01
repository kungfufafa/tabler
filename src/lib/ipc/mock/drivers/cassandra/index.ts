import { splitCql } from "@/features/drivers/cassandra/dialect";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { type Editor, runStatement } from "./cql";
import { clusterKeyspaces } from "./data";
import { cassandraSession, writes } from "./session";
import { ServerFailure } from "./values";

/**
 * The mock driver of the cassandra lane (Cassandra and ScyllaDB), served in `vite --mode mock`
 * with `?mockDrivers=cassandra`: a Cassandra 4.1 node with the `shop` and `iot` keyspaces, and a
 * ScyllaDB node opened read-only at LOCAL_QUORUM. Connecting checks what the backend's
 * `validate` and `open` check, in the same words; there are no calls of the form (`util`) and no
 * session calls.
 */

const CONSISTENCIES = [
  "ANY",
  "ONE",
  "TWO",
  "THREE",
  "QUORUM",
  "ALL",
  "LOCAL_QUORUM",
  "EACH_QUORUM",
  "LOCAL_ONE",
];

/** `CassandraParams::from_driver_params` and `check`, with their words. */
function params(p: ConnectionProfile): Editor {
  const raw = p.options.driverParams ?? {};
  const bad = (why: string): never => mockError("INVALID_INPUT", `Cassandra settings: ${why}`);
  const variant = (key: string, all: string[], fallback: string): string => {
    const v = raw[key];
    if (v === undefined || v === null) return fallback;
    if (typeof v !== "string" || !all.includes(v)) return bad(`unknown variant \`${String(v)}\``);
    return v;
  };
  const number = (key: string): number | null => {
    const v = raw[key];
    if (v === undefined || v === null) return null;
    if (typeof v !== "number" || !Number.isInteger(v) || v < 0)
      return bad(`invalid type for ${key}`);
    return v;
  };
  const consistency = variant("consistency", CONSISTENCIES, "LOCAL_ONE");
  const serial = variant("serialConsistency", ["SERIAL", "LOCAL_SERIAL"], "LOCAL_SERIAL");
  variant("compression", ["none", "lz4", "snappy"], "none");
  const timeout = number("requestTimeoutSecs");
  if (timeout !== null && (timeout < 1 || timeout > 3600))
    bad("the request timeout is 1 to 3600 seconds");
  const size = number("pageSize");
  if (size !== null && (size < 1 || size > 10_000)) bad("the page size is 1 to 10000 rows");
  const dc = raw.localDatacenter;
  if (typeof dc === "string" && dc !== "" && dc.trim() === "") bad("the local datacenter is blank");
  return {
    keyspace: (p.database ?? "").trim() || null,
    consistency,
    serial,
    tracing: false,
    paging: true,
    pageSize: size ?? 100,
    expand: false,
  };
}

/** `entry::validate`'s certificate pair. */
function validate(p: ConnectionProfile): void {
  const cert = (p.options.sslCertPath ?? "").trim() !== "";
  const key = (p.options.sslKeyPath ?? "").trim() !== "";
  if (cert !== key)
    mockError(
      "INVALID_INPUT",
      "Cassandra: a client certificate needs both its certificate and its key file",
    );
}

/** A ScyllaDB node: its `system.versions` names it (`NodeInfo::version`). */
const isScylla = (p: ConnectionProfile) => /scylla/i.test(p.host ?? "");

const version = (p: ConnectionProfile) => (isScylla(p) ? "ScyllaDB 6.2.1" : "Cassandra 4.1.7");

export const cassandraMock: MockDriver = {
  driver: "cassandra",
  serverVersion: version,
  seedProfiles: () => [
    profile(
      {
        id: "p-mock-cassandra",
        name: "Cassandra shop",
        driver: "cassandra",
        color: "teal",
        host: "127.0.0.1",
        port: 9042,
        user: "cassandra",
        database: "shop",
        ssl: "disable",
        sortOrder: 130,
      },
      "local",
    ),
    ((): ConnectionProfile => {
      const base = profile({
        id: "p-mock-scylla",
        name: "ScyllaDB iot (read-only)",
        driver: "cassandra",
        host: "scylla-1.iot.internal",
        port: 9042,
        user: "reader",
        database: "iot",
        ssl: "verify-full",
        sortOrder: 131,
      });
      return {
        ...base,
        options: {
          ...base.options,
          tag: "production",
          readOnly: true,
          driverParams: { consistency: "LOCAL_QUORUM", localDatacenter: "dc1" },
        },
      };
    })(),
  ],
  connect: (p) => {
    const editor = params(p);
    validate(p);
    const readOnly = p.options.readOnly;
    const startup = (p.options.startupSql ?? "").trim();
    const statements = startup ? splitCql(startup).map((s) => s.sql) : [];
    // A read-only profile whose startup commands write is refused before anything connects.
    const cqlshCommand = (sql: string) =>
      /^\s*(CONSISTENCY|SERIAL\s+CONSISTENCY|TRACING|PAGING|EXPAND)\b/i.test(sql);
    if (readOnly && statements.some((s) => !cqlshCommand(s) && writes(s)))
      mockError(
        "INVALID_INPUT",
        "Cassandra: this connection is read-only, and its startup commands write (or are not known to only read)",
      );
    const keyspaces = clusterKeyspaces();
    if (editor.keyspace !== null && !keyspaces.has(editor.keyspace))
      mockError("DB_ERROR", `Keyspace '${editor.keyspace}' does not exist`);
    for (const sql of statements) {
      try {
        runStatement(sql, { keyspaces, readOnly, editor }, () => undefined, writes);
      } catch (e) {
        // A server error reads as the startup commands' failure (`CassandraSession::open`).
        if (!(e instanceof ServerFailure)) throw e;
        mockError("INVALID_INPUT", `Cassandra: the startup commands failed: ${e.message}`);
      }
    }
    return cassandraSession({
      keyspaces,
      readOnly,
      confirm: p.options.passwordMode === "command" ? "passcode" : "password",
      editor,
    });
  },
};

const lane: MockLane = { drivers: [cassandraMock] };

export default lane;
