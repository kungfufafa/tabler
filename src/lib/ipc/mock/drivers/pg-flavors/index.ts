import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { PgFlavor } from "../../../bindings/PgFlavor";
import { profile } from "../../fixtures";
import type { MockDriver, MockLane } from "../types";
import type { Engine } from "./caps";
import { flavorSession, VERSION_TEXT } from "./session";

/**
 * The mock of the pg-flavors lane (CockroachDB, Greenplum and Amazon Redshift). These databases
 * are the PostgreSQL driver's, so the lane has no driver of its own (`drivers` is empty) and
 * gives one mock per engine in `flavors`: a PostgreSQL profile whose `pgFlavor` names it is
 * served by that engine's session (`session.ts`), with the caps and refusals the backend's hooks
 * give. A Greenplum profile is Greenplum 6 when its host names `gp6` (the backend reads the
 * version from the server), 7 otherwise.
 *
 * Until the mock's handlers send such a profile here (a change request to the lead: today a
 * flavor profile is served by the core PostgreSQL mock), `flavors` is reached by tests and by
 * whoever imports this module.
 */

/** The engine a profile of `flavor` connects to. */
export function engineOf(flavor: PgFlavor, p: ConnectionProfile): Engine {
  if (flavor === "greenplum") return /gp6/i.test(p.host ?? "") ? "greenplum6" : "greenplum7";
  return flavor;
}

function seed(id: string, name: string, flavor: PgFlavor, over: Partial<ConnectionProfile>) {
  const p = profile({ id, name, driver: "postgres", sortOrder: 200, ...over });
  return { ...p, options: { ...p.options, pgFlavor: flavor } };
}

function flavorDriver(flavor: PgFlavor, seeds: () => ConnectionProfile[]): MockDriver {
  return {
    driver: "postgres",
    serverVersion: (p) => VERSION_TEXT[engineOf(flavor, p)],
    seedProfiles: seeds,
    connect: (p) => flavorSession(engineOf(flavor, p), p),
  };
}

/** One mock per engine, by the profile's `pgFlavor`. */
export const flavors: Record<PgFlavor, MockDriver> = {
  cockroachdb: flavorDriver("cockroachdb", () => [
    seed("p-cockroachdb", "CockroachDB (movr)", "cockroachdb", {
      host: "crdb.local",
      port: 26257,
      user: "root",
      database: "defaultdb",
    }),
  ]),
  greenplum: flavorDriver("greenplum", () => [
    seed("p-greenplum6", "Greenplum 6 warehouse", "greenplum", {
      host: "gp6.local",
      user: "gpadmin",
      database: "warehouse",
    }),
    seed("p-greenplum7", "Greenplum 7 warehouse", "greenplum", {
      host: "gp7.local",
      user: "gpadmin",
      database: "warehouse",
    }),
  ]),
  redshift: flavorDriver("redshift", () => [
    seed("p-redshift", "Redshift (dev)", "redshift", {
      host: "examplecluster.abc123xyz789.us-west-2.redshift.amazonaws.com",
      port: 5439,
      user: "awsuser",
      database: "dev",
      ssl: "require",
    }),
  ]),
};

const lane: MockLane & { flavors: Record<PgFlavor, MockDriver> } = { drivers: [], flavors };

export default lane;
