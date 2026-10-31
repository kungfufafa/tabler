import { describe, expect, it } from "vitest";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { dynamoUi } from "./form";
import { dynamoFormOf, formParams, formTlsApplies, isAwsHost, normalizeEndpoint } from "./params";
import { formatDynamoUrl, parseDynamoUrl } from "./url";

// DynamoDB's form: its values as a profile and back, what `validate` says before the backend's
// `entry::validate` and `DynamoParams::check` would, and `dynamodb://` URLs read and written back.

const values = (v: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  driver: "dynamodb",
  ...dynamoUi.defaults!(),
  ...v,
});

const params = (p: Record<string, unknown>) => formParams({ ...dynamoFormOf(null), ...p });

const base = (v: ConnectionFormValues): ConnectionProfile =>
  profile({
    id: "p",
    name: "Dynamo",
    driver: "dynamodb",
    host: v.host || null,
    port: null,
    user: v.user || null,
    database: v.database || null,
    ssl: v.ssl,
    options: {
      ...profile({ id: "x", name: "x", driver: "dynamodb" }).options,
      passwordMode: v.passwordMode,
      passwordCommand: v.passwordCommand || null,
    },
  });

const toProfile = (v: ConnectionFormValues) => dynamoUi.toProfile!(v, base(v));

const issues = (v: Partial<ConnectionFormValues>) =>
  dynamoUi.validate!(values(v)).map((i) => `${i.path}: ${i.message}`);

describe("the form", () => {
  it("starts on an access key in us-east-1, an endpoint of its own verified", () => {
    expect(dynamoUi.defaults!()).toMatchObject({ database: "us-east-1", ssl: "verify-full" });
    expect(dynamoFormOf(dynamoUi.defaults!().driverParams)).toMatchObject({
      auth: "accessKey",
      sampleSize: "100",
      pageRequests: "5",
    });
    // Every row is the lane's: which ones show depends on the credential source.
    expect(dynamoUi.rows).toEqual([]);
    expect(dynamoUi.sslModes?.map(([m]) => m)).toEqual(["require", "verify-full"]);
  });

  it("saves every param the backend reads, and reads them back", () => {
    const v = values({
      user: "AKIAEXAMPLE",
      driverParams: params({
        auth: "profile",
        profileName: "dev",
        endpoint: "https://vpce-1.dynamodb.us-east-1.vpce.amazonaws.com",
        consistentReads: true,
        sampleSize: "250",
        pageRequests: "8",
        consumedCapacity: true,
      }),
    });
    const p = toProfile(v);
    expect(p.options.driverParams).toEqual({
      auth: "profile",
      profileName: "dev",
      endpoint: "https://vpce-1.dynamodb.us-east-1.vpce.amazonaws.com",
      consistentReads: true,
      sampleSize: 250,
      pageRequests: 8,
      consumedCapacity: true,
    });
    // A profile signs with its own keys: no key ID, no secret to ask for.
    expect(p.user).toBeNull();
    expect(p.options.passwordMode).toBe("none");
    expect([p.host, p.port, p.ssh]).toEqual([null, null, null]);
    expect(dynamoUi.fromProfile!(p)).toEqual({ driverParams: v.driverParams });
  });

  it("keeps the key ID and the secret's mode for an access key and the Command source", () => {
    const key = toProfile(values({ user: "AKIAEXAMPLE", passwordMode: "ask" }));
    expect([key.user, key.options.passwordMode]).toEqual(["AKIAEXAMPLE", "ask"]);
    expect(key.options.driverParams).toEqual({
      auth: "accessKey",
      consistentReads: false,
      sampleSize: 100,
      pageRequests: 5,
      consumedCapacity: false,
    });
    const command = toProfile(
      values({
        driverParams: params({ auth: "command" }),
        passwordMode: "command",
        passwordCommand: "aws configure export-credentials --format process",
      }),
    );
    expect(command.options.passwordMode).toBe("command");
    expect(command.options.passwordCommand).toBe(
      "aws configure export-credentials --format process",
    );
    // A profile name is saved only for the profile source.
    const env = toProfile(
      values({ driverParams: params({ auth: "environment", profileName: "x" }) }),
    );
    expect(env.options.driverParams).not.toHaveProperty("profileName");
  });

  it("says what the backend would refuse", () => {
    expect(issues({ user: "AKIA" })).toEqual([]);
    expect(issues({ user: "AKIA", database: "" })).toEqual([
      "database: Enter the region (like us-east-1)",
    ]);
    expect(issues({ user: "AKIA", database: "US East" })).toEqual([
      "database: “US East” is not a region name (like us-east-1)",
    ]);
    expect(issues({})).toEqual(["user: Enter the access key ID"]);
    expect(issues({ driverParams: params({ auth: "command" }) })).toEqual([
      "passwordMode: The Command source reads the secret's Command Line: choose Command Line",
    ]);
    expect(issues({ driverParams: params({ auth: "command" }), passwordMode: "command" })).toEqual(
      [],
    );
    expect(
      issues({
        driverParams: params({
          auth: "local",
          endpoint: "ftp://x",
          sampleSize: "0",
          pageRequests: "51",
        }),
      }),
    ).toEqual([
      "driverParams.endpoint: The endpoint must start with http:// or https://",
      "driverParams.sampleSize: Between 1 and 1000",
      "driverParams.pageRequests: Between 1 and 50",
    ]);
    expect(
      issues({ driverParams: params({ auth: "local", endpoint: "http://u:p@h:8000" }) }),
    ).toEqual(["driverParams.endpoint: The endpoint must not hold credentials"]);
    expect(issues({ driverParams: params({ auth: "profile", profileName: "my dev" }) })).toEqual([
      "driverParams.profileName: A profile name has no spaces",
    ]);
  });

  it("normalizes endpoints and knows AWS's own hosts", () => {
    expect(normalizeEndpoint(" localhost:4566 ")).toEqual({ url: "https://localhost:4566/" });
    expect(normalizeEndpoint("http://127.0.0.1:8000")).toEqual({ url: "http://127.0.0.1:8000/" });
    expect(normalizeEndpoint("https://vpce.example/ddb")).toEqual({
      url: "https://vpce.example/ddb/",
    });
    for (const bad of ["http://h/?a=1", "http://h/#f", "file:///x", "http://"])
      expect("error" in normalizeEndpoint(bad), bad).toBe(true);
    expect(isAwsHost("https://dynamodb.eu-west-1.api.aws/")).toBe(true);
    expect(isAwsHost("https://dynamodb.us-east-1.amazonaws.com.evil.test/")).toBe(false);
    // The form's SSL mode applies only to an https endpoint off AWS's domains.
    const tls = (p: Record<string, unknown>) => formTlsApplies(dynamoFormOf(params(p)));
    expect(tls({ endpoint: "https://ddb.internal:8443" })).toBe(true);
    expect(tls({ endpoint: "http://localhost:8000" })).toBe(false);
    expect(tls({ endpoint: "https://dynamodb-fips.us-east-1.amazonaws.com" })).toBe(false);
    expect(tls({})).toBe(false);
    expect(tls({ auth: "local" })).toBe(false);
  });

  it("summarizes the region, the credentials and the endpoint", () => {
    const summary = (v: Partial<ConnectionFormValues>) => dynamoUi.summary!(toProfile(values(v)));
    expect(summary({ user: "AKIAEXAMPLE" })).toBe("us-east-1 · AKIAEXAMPLE");
    expect(summary({ driverParams: params({ auth: "profile" }) })).toBe(
      "us-east-1 · profile default",
    );
    expect(summary({ database: "eu-west-1", driverParams: params({ auth: "local" }) })).toBe(
      "eu-west-1 · local · http://localhost:8000/",
    );
  });
});

describe("dynamodb:// URLs", () => {
  /** A URL read into a profile and written back. */
  const roundTrip = (url: string): string | null => {
    const parsed = parseDynamoUrl(url);
    if (!parsed) return null;
    return formatDynamoUrl(toProfile(values(parsed.values)));
  };

  it("round-trip for every credential source", () => {
    for (const url of [
      "dynamodb://AKIAEXAMPLE@us-east-1",
      "dynamodb://eu-central-1?auth=profile&profile=dev",
      "dynamodb://ap-southeast-2?auth=environment&consistentReads=true",
      "dynamodb://AKIAEXAMPLE@us-west-2?auth=command",
      "dynamodb://us-east-1?auth=local&endpoint=http%3A%2F%2Flocalhost%3A8000",
      "dynamodb://AKIAEXAMPLE@us-east-1?endpoint=https%3A%2F%2Fddb.internal%3A8443&sampleSize=20&pageRequests=3&consumedCapacity=true&sslmode=require",
    ])
      expect(roundTrip(url), url).toBe(url);
  });

  it("read a URL's sslmode as one of the form's two", () => {
    const ssl = (mode: string) =>
      parseDynamoUrl(`dynamodb://us-east-1?endpoint=https%3A%2F%2Fddb.internal&sslmode=${mode}`)
        ?.values.ssl;
    expect(["disable", "prefer", "require", "verify-ca", "verify-full", "bogus"].map(ssl)).toEqual([
      "require",
      "require",
      "require",
      "verify-full",
      "verify-full",
      "verify-full",
    ]);
    expect(dynamoUi.sslModes?.map(([m]) => m)).toEqual(["require", "verify-full"]);
  });

  it("read the secret as the password, never into the profile", () => {
    const parsed = parseDynamoUrl("dynamodb://AKIAEXAMPLE:wJalr%2FKEY@us-east-1?name=Prod")!;
    expect(parsed.password).toBe("wJalr/KEY");
    expect(parsed.values).toMatchObject({ user: "AKIAEXAMPLE", database: "us-east-1" });
    expect(JSON.stringify(parsed.values)).not.toContain("wJalr");
    const command = parseDynamoUrl("dynamodb://us-east-1?auth=command")!;
    expect(command.values.passwordMode).toBe("command");
  });

  it("refuse what is not one", () => {
    for (const bad of [
      "mongodb://x",
      "dynamodb://us-east-1/table",
      "dynamodb://us-east-1?auth=magic",
      "dynamodb://%E0%A4%A@us-east-1",
    ])
      expect(parseDynamoUrl(bad), bad).toBeNull();
    expect(dynamoUi.url?.schemes).toEqual(["dynamodb"]);
  });
});
