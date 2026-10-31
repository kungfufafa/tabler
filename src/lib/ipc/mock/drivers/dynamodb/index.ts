import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { MockDriver, MockLane } from "../types";
import { dynamoSession, optionsOf } from "./session";

/**
 * The mock drivers of the dynamodb lane (DynamoDB), served in `vite --mode mock` with
 * `?mockDrivers=dynamodb`: an AWS region signed with an access key, DynamoDB Local, and a
 * read-only connection, over one region's tables (`data.ts`: Music with a global and a local
 * index, sparse Users, and Events of several pages).
 */

/** The form's region names (`entry::REGIONS`, the first few). */
const REGIONS = [
  "us-east-1",
  "us-east-2",
  "us-west-1",
  "us-west-2",
  "eu-west-1",
  "eu-central-1",
  "ap-southeast-1",
  "ap-northeast-1",
];

const options = (
  readOnly: boolean,
  driverParams: Record<string, unknown>,
  passwordMode: ConnectionProfile["options"]["passwordMode"] = "keychain",
): ConnectionProfile["options"] => ({
  tag: null,
  safeMode: null,
  timezone: null,
  connectTimeoutSecs: null,
  readOnly,
  startupSql: null,
  defaultRowLimit: null,
  sqliteForeignKeys: true,
  passwordMode,
  driverParams: {
    consistentReads: false,
    sampleSize: 100,
    pageRequests: 5,
    consumedCapacity: false,
    ...driverParams,
  },
});

const DYNAMODB: MockDriver = {
  driver: "dynamodb",
  serverVersion: (p) => {
    const { endpoint } = optionsOf(p);
    return endpoint ? `DynamoDB API 2012-08-10 at ${endpoint}` : "DynamoDB API 2012-08-10";
  },
  seedProfiles: () => [
    profile({
      id: "p-mock-dynamodb",
      name: "DynamoDB (mock)",
      driver: "dynamodb",
      host: null,
      port: null,
      user: "AKIAIOSFODNN7EXAMPLE",
      database: "us-east-1",
      ssl: "verify-full",
      sortOrder: 130,
      options: options(false, { auth: "accessKey" }),
    }),
    profile({
      id: "p-mock-dynamodb-local",
      name: "DynamoDB Local (mock)",
      driver: "dynamodb",
      host: null,
      port: null,
      user: null,
      database: "us-east-1",
      ssl: "verify-full",
      sortOrder: 131,
      options: options(false, { auth: "local" }, "none"),
    }),
    profile({
      id: "p-mock-dynamodb-ro",
      name: "DynamoDB read-only (mock)",
      driver: "dynamodb",
      host: null,
      port: null,
      user: null,
      database: "us-east-1",
      ssl: "verify-full",
      sortOrder: 132,
      options: options(true, { auth: "profile", profileName: "prod-readonly" }, "none"),
    }),
  ],
  connect: dynamoSession,
  // The form's calls (`entry::util`), both reads.
  util: (method) => {
    switch (method) {
      case "regions":
        return REGIONS;
      case "awsProfiles":
        return ["default", "dev", "prod-readonly"];
      default:
        return mockError("UNSUPPORTED", `unsupported: DynamoDB: unknown call “${method}”`);
    }
  },
};

const lane: MockLane = { drivers: [DYNAMODB] };

export default lane;
