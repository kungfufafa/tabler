import type { Feature } from "../../../bindings/Feature";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import { DRIVER_CATALOG } from "../../../bindings/driverCatalog";

/**
 * The caps of a session on each engine, built as `PgSession::capabilities` builds them: the
 * PostgreSQL caps (`SessionCaps::base`) without what the flavors are not proven to do
 * (`flavor::adjust_caps`), then each engine's `adjust_caps` (`flavor/imp/*.rs`) by server
 * version. `tests/fixtures/pg_flavors/caps.json` holds the backend's own, which the tests compare.
 */

/** The engines (and versions) the mock serves. */
export type Engine = "cockroachdb" | "greenplum6" | "greenplum7" | "redshift";

/** `server_version_num` each engine reports (CockroachDB says 13.0.0, Redshift 8.0.2). */
export const VERSION_NUM: Record<Engine, number> = {
  cockroachdb: 130_000,
  greenplum6: 90_426,
  greenplum7: 120_012,
  redshift: 80_002,
};

/** A core PostgreSQL session's caps: every feature, in `Feature`'s order. */
const CORE = DRIVER_CATALOG.baseCaps.postgres as SessionCaps;

/** `flavor::UNPROVEN_ON_FLAVORS`. */
const UNPROVEN: readonly Feature[] = ["structureEdit", "backup", "restore", "users", "killProcess"];

const ORDER = CORE.features;

/** `SessionCaps::with`: added, in `Feature`'s order, once. */
function withF(caps: SessionCaps, add: readonly Feature[]): SessionCaps {
  const set = new Set([...caps.features, ...add]);
  return { ...caps, features: ORDER.filter((f) => set.has(f)) };
}

/** `SessionCaps::without`. */
function withoutF(caps: SessionCaps, drop: readonly Feature[]): SessionCaps {
  return { ...caps, features: caps.features.filter((f) => !drop.includes(f)) };
}

/** PostgreSQL's caps on a flavor before its hooks grant anything back. */
function flavored(flavor: "cockroachdb" | "greenplum"): SessionCaps {
  return withoutF({ ...CORE, flavor }, UNPROVEN);
}

/** `cockroach.rs` `adjust_caps`. */
function cockroachdb(): SessionCaps {
  return withoutF(withF(flavored("cockroachdb"), ["killProcess"]), [
    "editWithoutKey",
    "encoding",
    "createDatabase",
    "sessionDetails",
  ]);
}

/** `greenplum.rs` `adjust_caps` at `version` (`server_version_num`). */
function greenplum(version: number): SessionCaps {
  let caps = withoutF(withF(flavored("greenplum"), ["killProcess", "users"]), ["editWithoutKey"]);
  if (version < 100_000) caps = withoutF(caps, ["sequences", "diagram", "createDatabase"]);
  if (version < 90_500)
    caps = { ...withoutF(caps, ["sessionDetails"]), data: { ...caps.data, upsert: false } };
  return caps;
}

/** `SessionCaps::minimal` for Redshift SQL, then `redshift.rs` `adjust_caps`. */
function redshift(): SessionCaps {
  const minimal: SessionCaps = {
    ...CORE,
    flavor: "redshift",
    dialect: "redshift",
    features: ["editor", "structureView"],
    safety: { readOnly: "classifierOnly", hardened: false, confirm: "password" },
    session: { keepaliveSecs: null, pingBilled: false },
  };
  return withF(
    {
      ...minimal,
      data: {
        ...CORE.data,
        filterOps: CORE.data.filterOps.filter(
          (op) => op !== "jsonContains" && op !== "jsonNotContains",
        ),
        // `SPEC.knobs.set_default`: no DEFAULT in an UPDATE; no ON CONFLICT.
        defaultKeyword: false,
        upsert: false,
        serverExprs: ["now", "currentTimestamp", "currentDate"],
        maxParams: 32_000,
      },
    },
    [
      "explain",
      "format",
      "editRows",
      "insertRows",
      "deleteRows",
      "serverExpressions",
      "exactCount",
      "tableDdl",
      "viewDefinition",
      "routines",
      "switchDatabase",
      "processList",
      "killProcess",
      "timezone",
      "startupCommands",
      "exportTables",
    ],
  );
}

/** The caps of a session on `engine`. */
export function flavorCaps(engine: Engine): SessionCaps {
  switch (engine) {
    case "cockroachdb":
      return cockroachdb();
    case "greenplum6":
    case "greenplum7":
      return greenplum(VERSION_NUM[engine]);
    case "redshift":
      return redshift();
  }
}

/** The engine's name in the backend's refusals (`SessionCaps::engine`). */
export const ENGINE_LABEL: Record<Engine, string> = {
  cockroachdb: "CockroachDB",
  greenplum6: "Greenplum",
  greenplum7: "Greenplum",
  redshift: "Amazon Redshift",
};

/** `SessionCaps::require`'s refusal, as the mock's handlers throw it. */
export function notAvailable(engine: Engine, what: string): string {
  return `unsupported: ${ENGINE_LABEL[engine]}: ${what} is not available`;
}
