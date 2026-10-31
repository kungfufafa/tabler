import {
  cloudIdUrl,
  isServerlessHost,
  parseBaseUrl,
  regionForHost,
  rootOf,
} from "@/features/drivers/elasticsearch/address";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { MockDriver, MockLane } from "../types";
import { esSession, optionsOf } from "./session";

/**
 * The mock drivers of the elasticsearch lane (Elasticsearch and OpenSearch), served in
 * `vite --mode mock` with `?mockDrivers=elasticsearch`: an Elasticsearch 9 cluster, an
 * OpenSearch 2 cluster and a read-only connection, each over its in-memory cluster (`data.ts`).
 */

const seed = (
  id: string,
  name: string,
  sortOrder: number,
  extra: Partial<ConnectionProfile> & { readOnly?: boolean } = {},
): ConnectionProfile => {
  const { readOnly = false, ...rest } = extra;
  return profile({
    id,
    name,
    driver: "elasticsearch",
    host: "127.0.0.1",
    port: 9200,
    user: "elastic",
    database: null,
    ssl: "require",
    sortOrder,
    ...rest,
    options: {
      tag: null,
      safeMode: null,
      timezone: null,
      connectTimeoutSecs: null,
      readOnly,
      startupSql: null,
      defaultRowLimit: null,
      sqliteForeignKeys: true,
      driverParams: {
        urls: [],
        auth: "basic",
        awsCredentials: "accessKey",
        showSystemIndices: false,
      },
    },
  });
};

const arg = (args: unknown, name: string): string => {
  const v = (args as Record<string, unknown> | null)?.[name];
  if (typeof v !== "string" || v.trim() === "")
    mockError("INVALID_INPUT", `Elasticsearch: ${name} is required`);
  return v.trim();
};

const ELASTICSEARCH: MockDriver = {
  driver: "elasticsearch",
  serverVersion: (p) => {
    const o = optionsOf(p);
    return o.flavor === "opensearch" ? `OpenSearch ${o.version}` : `Elasticsearch ${o.version}`;
  },
  seedProfiles: () => [
    seed("p-mock-elasticsearch", "Elasticsearch (mock)", 120),
    seed("p-mock-opensearch", "OpenSearch (mock)", 121, {
      host: "opensearch.local",
      user: "admin",
    }),
    seed("p-mock-elasticsearch-ro", "Elasticsearch read-only (mock)", 122, {
      user: "viewer",
      readOnly: true,
    }),
  ],
  connect: esSession,
  // The connection form's calls (`entry::util`), both reads.
  util: (method, args) => {
    if (method === "cloudIdUrl") {
      const base = cloudIdUrl(arg(args, "cloudId"));
      if (!base) mockError("INVALID_INPUT", "the Cloud ID is not valid");
      return { url: rootOf(base) };
    }
    if (method === "awsEndpoint") {
      const base = parseBaseUrl(arg(args, "host"), true);
      if ("error" in base) mockError("INVALID_INPUT", `URL ${base.error}`);
      return {
        region: regionForHost(base.host),
        service: isServerlessHost(base.host) ? "aoss" : "es",
      };
    }
    return mockError("UNSUPPORTED", `unsupported: Elasticsearch: unknown call “${method}”`);
  },
};

const lane: MockLane = { drivers: [ELASTICSEARCH] };

export default lane;
