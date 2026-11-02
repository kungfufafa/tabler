import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { MockDriver, MockLane } from "../types";
import { server } from "./data";
import { redisSession } from "./session";

/**
 * The mock drivers of the redis lane (Redis and Valkey), served in `vite --mode mock` with
 * `?mockDrivers=redis`: a Redis 7.4 server on db0, the same server's db1, and a read-only
 * connection to it, over one in-memory server (`data.ts`).
 */

const seed = (
  id: string,
  name: string,
  sortOrder: number,
  database: string | null,
  readOnly = false,
): ConnectionProfile =>
  profile({
    id,
    name,
    driver: "redis",
    host: "127.0.0.1",
    port: 6379,
    user: null,
    database,
    ssl: "disable",
    sortOrder,
    options: {
      tag: null,
      safeMode: null,
      timezone: null,
      connectTimeoutSecs: null,
      readOnly,
      startupSql: null,
      defaultRowLimit: null,
      sqliteForeignKeys: true,
      driverParams: null,
    },
  });

const REDIS: MockDriver = {
  driver: "redis",
  serverVersion: () => `${server().product} ${server().version}`,
  seedProfiles: () => [
    seed("p-mock-redis", "Redis (mock)", 130, null),
    seed("p-mock-redis-db1", "Redis db1 (mock)", 131, "1"),
    seed("p-mock-redis-ro", "Redis read-only (mock)", 132, null, true),
  ],
  connect: redisSession,
  // The connection form has no Redis calls (`entry::util`): every one is unknown.
  util: (method) => mockError("UNSUPPORTED", `unsupported: Redis: unknown call “${method}”`),
};

const lane: MockLane = { drivers: [REDIS] };

export default lane;
