import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { mongoSession, optionsOf } from "./session";

/**
 * The mock drivers of the mongodb lane (MongoDB), served in `vite --mode mock` with
 * `?mockDrivers=mongodb`: a standalone server (changes applied one after the other), a replica
 * set (one transaction), and a read-only connection, over one in-memory server (`data.ts`).
 */

const MONGODB: MockDriver = {
  driver: "mongodb",
  serverVersion: (p) =>
    optionsOf(p).transactions ? "MongoDB 8.0.4 (replica set rs0)" : "MongoDB 8.0.4",
  seedProfiles: () => [
    profile({
      id: "p-mock-mongodb",
      name: "MongoDB (mock)",
      driver: "mongodb",
      port: 27017,
      user: "app",
      database: "shop",
      ssl: "disable",
      sortOrder: 110,
      options: {
        tag: null,
        safeMode: null,
        timezone: null,
        connectTimeoutSecs: null,
        readOnly: false,
        startupSql: null,
        defaultRowLimit: null,
        sqliteForeignKeys: true,
        driverParams: { authMechanism: "default", readPreference: "primary" },
      },
    }),
    profile({
      id: "p-mock-mongodb-rs",
      name: "MongoDB replica set (mock)",
      driver: "mongodb",
      port: 27017,
      user: "app",
      database: null,
      ssl: "disable",
      sortOrder: 111,
      options: {
        tag: null,
        safeMode: null,
        timezone: null,
        connectTimeoutSecs: null,
        readOnly: false,
        startupSql: null,
        defaultRowLimit: null,
        sqliteForeignKeys: true,
        driverParams: { authMechanism: "default", readPreference: "primary", replicaSet: "rs0" },
      },
    }),
    profile({
      id: "p-mock-mongodb-ro",
      name: "MongoDB read-only (mock)",
      driver: "mongodb",
      port: 27017,
      user: "reader",
      database: "shop",
      ssl: "disable",
      sortOrder: 112,
      options: {
        tag: null,
        safeMode: null,
        timezone: null,
        connectTimeoutSecs: null,
        readOnly: true,
        startupSql: null,
        defaultRowLimit: null,
        sqliteForeignKeys: true,
        driverParams: { authMechanism: "default", readPreference: "primary" },
      },
    }),
  ],
  connect: mongoSession,
  // The connection form has no MongoDB calls (`entry::util`): every one is unknown.
  util: (method) => mockError("UNSUPPORTED", `unsupported: MongoDB: unknown call “${method}”`),
};

const lane: MockLane = { drivers: [MONGODB] };

export default lane;
