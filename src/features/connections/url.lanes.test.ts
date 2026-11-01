import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { planOpen } from "@/features/deeplink/route";
import { fakeLane } from "@/features/drivers/__fixtures__/fakeLane";
import { CORE_KINDS } from "@/features/drivers/catalog";
import { registerLaneOverride } from "@/features/drivers/registry";
import type { ConnectionProfile, DatabaseKind } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useAppInfoStore } from "@/stores/appInfo";
import {
  loadUrlSchemes,
  parseConnectionUrl,
  parseDeepLink,
  profileToUrl,
  profileUrlQuery,
} from "./url";

// The URLs of a lane's database (F9b): its lane reads and writes them, found by scheme; the fake
// lane stands in for MongoDB's (its scheme is `fake://host:port/authSource`).

const built = (...kinds: DatabaseKind[]) =>
  useAppInfoStore.setState({
    info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, ...kinds] },
  });

const docs: ConnectionProfile = {
  ...SEED_PROFILES[0]!,
  id: "p-docs",
  name: "Docs",
  driver: "mongodb",
  host: "db.example",
  port: 27018,
  user: null,
  database: null,
  ssh: null,
  options: { ...SEED_PROFILES[0]!.options, driverParams: { authSource: "audit" } },
};

beforeEach(() => registerLaneOverride("mongodb", fakeLane("mongodb", "mongodb")));
afterEach(() => {
  registerLaneOverride("mongodb", null);
  registerLaneOverride("redis", null);
  useAppInfoStore.setState({ info: null });
});

describe("a lane's URLs", () => {
  it("are not read without the database in this build", () => {
    expect(() => parseConnectionUrl("fake://db.example:27018/audit")).toThrow(/postgres/);
    expect(() => parseConnectionUrl("mongodb://db.example")).toThrow(/postgres/);
  });

  it("wait for the lane: its driver's scheme is known before the lane has loaded", async () => {
    built("mongodb");
    expect(() => parseConnectionUrl("mongodb://db.example")).toThrow(/still loading/);
    await loadUrlSchemes();
    // The loaded lane says which schemes it reads.
    const parsed = parseConnectionUrl("fake://db.example:27018/audit");
    expect(parsed.values).toMatchObject({
      driver: "mongodb",
      pgFlavor: null,
      host: "db.example",
      port: "27018",
      database: "audit",
      name: "db.example",
    });
    expect(parsed.password).toBeNull();
    expect(() => parseConnectionUrl("fake://")).toThrow("This is not a valid MongoDB URL.");
  });

  it("keep the SSH server's part of a +ssh URL", async () => {
    built("mongodb");
    await loadUrlSchemes();
    const parsed = parseConnectionUrl("fake+ssh://me:pw@bastion:2222/db.example:27017/admin");
    expect(parsed.values).toMatchObject({
      driver: "mongodb",
      host: "db.example",
      sshEnabled: true,
      sshHost: "bastion",
      sshPort: "2222",
      sshUser: "me",
      sshAuth: "password",
      sshPassword: "pw",
      name: "db.example via bastion",
    });
  });

  it("are written by the lane, over SSH as a +ssh URL", async () => {
    built("mongodb");
    await loadUrlSchemes();
    expect(profileToUrl(docs)).toBe("fake://db.example:27018/audit");
    const ssh = {
      ...SEED_PROFILES.find((p) => p.ssh)!.ssh!,
      user: "me",
      host: "bastion",
      port: 22,
    };
    expect(profileToUrl({ ...docs, ssh })).toBe("fake+ssh://me@bastion/db.example:27018/audit");
  });

  it("can carry the common query a lane's format appends", () => {
    const query = profileUrlQuery({
      ...docs,
      options: { ...docs.options, safeMode: "safe", lazyLoad: true },
    });
    expect(query.toString()).toBe(
      "name=Docs&statusColor=2279DC&env=local&safeModeLevel=2&lazyload=true",
    );
  });

  it("fall back to a plain URL of the driver's scheme while no lane writes them", () => {
    const cache = { ...docs, driver: "redis" as const, port: 6379, user: "app" };
    expect(profileToUrl(cache, "s3cret")).toBe(
      "redis://app:s3cret@db.example:6379?name=Docs&statusColor=2279DC&env=local",
    );
  });

  it("leave the core URLs as they were", async () => {
    built("mongodb");
    await loadUrlSchemes();
    expect(parseConnectionUrl("postgres://u@h:5433/db").values).toMatchObject({
      driver: "postgres",
      port: "5433",
    });
    expect(() => parseConnectionUrl("https://example.com")).toThrow(/postgres/);
  });
});

describe("where a lane's URL comes from", () => {
  const sources: string[] = [];
  const base = fakeLane("bigquery", "bigquery").kinds!.bigquery!;
  const tokenLane = {
    kinds: {
      bigquery: {
        ...base,
        url: {
          schemes: ["faketoken"],
          parse: (_url: string, source?: "link" | "paste") => {
            sources.push(source ?? "none");
            return { values: { host: "h", port: "443", database: "d" }, password: "tok" };
          },
          format: () => null,
        },
      },
    },
  };
  const saved: ConnectionProfile = {
    ...docs,
    id: "44444444-4444-4444-8444-444444444444",
    driver: "bigquery",
    host: "h",
    port: 443,
    database: "d",
  };

  afterEach(() => registerLaneOverride("bigquery", null));

  it("is told to the lane: pasted, or a clicked link", async () => {
    registerLaneOverride("bigquery", tokenLane);
    built("bigquery");
    await loadUrlSchemes();
    sources.length = 0;
    parseConnectionUrl("faketoken://h/d");
    parseDeepLink("faketoken://h/d");
    parseDeepLink(`tabler://open?url=${encodeURIComponent("faketoken://h/d")}`);
    expect(sources).toEqual(["paste", "link", "link"]);
  });

  it("never signs a saved token connection in with the link's token", async () => {
    registerLaneOverride("bigquery", tokenLane);
    built("bigquery");
    await loadUrlSchemes();
    const plan = planOpen(
      { kind: "url", url: "faketoken://h/d" },
      { profiles: [saved], temporary: new Set(), activeConnectionId: null },
    );
    expect(plan).toMatchObject({ kind: "profile", profileId: saved.id, password: null });
    // A temporary connection is the link's own: its token is what it signs in with.
    const temporary = planOpen(
      { kind: "url", url: "faketoken://h/d" },
      { profiles: [], temporary: new Set(), activeConnectionId: null },
    );
    expect(temporary).toMatchObject({ kind: "temporary", password: "tok" });
  });
});
