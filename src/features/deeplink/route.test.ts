import { describe, expect, it } from "vitest";
import { parseDeepLink } from "@/features/connections/url";
import { ANY_COLUMN, RAW_SQL_COLUMN } from "@/features/table-tab/filters";
import type { ConnectionProfile } from "@/lib/ipc";
import { filterOpOf, filterRowOf, findMatchingProfile, planOpen, type RouteContext } from "./route";

const pg: ConnectionProfile = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "Local PG",
  color: "blue",
  group: null,
  driver: "postgres",
  host: "127.0.0.1",
  port: 5432,
  user: "app",
  database: "shop",
  ssl: "prefer",
  sslCaPath: null,
  sqlitePath: null,
  ssh: null,
  options: {
    tag: null,
    safeMode: null,
    timezone: null,
    connectTimeoutSecs: null,
    readOnly: false,
    startupSql: null,
    defaultRowLimit: null,
    sqliteForeignKeys: true,
  },
  sortOrder: 0,
  createdAt: "",
  updatedAt: "",
  lastConnectedAt: null,
};
const viaSsh: ConnectionProfile = {
  ...pg,
  id: "22222222-2222-4222-8222-222222222222",
  name: "Prod via bastion",
  ssh: {
    host: "bastion.example.com",
    port: 22,
    user: "ops",
    auth: "key",
    keyPath: null,
    localPort: null,
    keepaliveSecs: null,
  },
};
const lite: ConnectionProfile = {
  ...pg,
  id: "33333333-3333-4333-8333-333333333333",
  name: "App DB",
  driver: "sqlite",
  host: null,
  port: null,
  user: null,
  database: null,
  sqlitePath: "/Users/me/app.sqlite",
};

const ctx = (over: Partial<RouteContext> = {}): RouteContext => ({
  profiles: [pg, viaSsh, lite],
  temporary: new Set(),
  activeConnectionId: null,
  ...over,
});

describe("planOpen: connection URLs", () => {
  it("opens the saved connection a URL describes, with the URL's password for this connect", () => {
    const plan = planOpen(
      { kind: "url", url: "postgres://app:pw@localhost/shop?table=orders&column=id&value=7" },
      ctx(),
    );
    expect(plan).toEqual({
      kind: "profile",
      profileId: pg.id,
      password: "pw",
      sshPassword: null,
      target: {
        schema: null,
        table: "orders",
        filter: { kind: "condition", column: "id", operator: "=", value: "7" },
      },
      isolated: false,
      lazyLoad: null,
    });
  });

  it("carries the link's lazyload for a saved connection too", () => {
    const lazy = (url: string) => {
      const plan = planOpen({ kind: "url", url }, ctx());
      return plan.kind === "profile" ? plan.lazyLoad : "not a profile plan";
    };
    expect(lazy("postgres://app@localhost/shop?lazyload=true")).toBe(true);
    expect(lazy("postgres://app@localhost/shop?lazyload=0")).toBe(false);
    expect(lazy("postgres://app@localhost/shop")).toBeNull();
    expect(lazy(`tabler://connection/${pg.id}?lazyload=true`)).toBe(true);
    expect(lazy(`tabler://connection/${pg.id}`)).toBeNull();
  });

  it("opens a temporary connection when nothing matches", () => {
    const plan = planOpen(
      { kind: "url", url: "postgres://app@127.0.0.1:6543/shop?windowMode=isolated" },
      ctx(),
    );
    expect(plan).toMatchObject({
      kind: "temporary",
      values: { driver: "postgres", host: "127.0.0.1", port: "6543", database: "shop" },
      password: null,
      isolated: true,
      lazyLoad: null,
    });
    // `lazyload` travels with the plan of a temporary connection.
    expect(
      planOpen({ kind: "url", url: "postgres://app@127.0.0.1:6543/shop?lazyload=0" }, ctx()),
    ).toMatchObject({ kind: "temporary", lazyLoad: false });
  });

  it("matches +ssh URLs on the SSH server too, and keeps the SSH password apart", () => {
    const url = "postgresql+ssh://ops:sshpw@bastion.example.com/app:dbpw@127.0.0.1/shop";
    expect(planOpen({ kind: "url", url }, ctx())).toMatchObject({
      kind: "profile",
      profileId: viaSsh.id,
      password: "dbpw",
      sshPassword: "sshpw",
    });
    const other = planOpen(
      { kind: "url", url: "postgresql+ssh://ops:sshpw@other-bastion/app@127.0.0.1/shop" },
      ctx(),
    );
    expect(other.kind).toBe("temporary");
    if (other.kind === "temporary") {
      expect(other.sshPassword).toBe("sshpw");
      expect(other.values.sshPassword).toBeUndefined();
      expect(other.values).toMatchObject({ sshEnabled: true, sshHost: "other-bastion" });
    }
  });

  it("prefers a saved connection over a temporary one with the same settings", () => {
    const temp = { ...pg, id: "44444444-4444-4444-8444-444444444444" };
    const found = findMatchingProfile(
      { driver: "postgres", host: "127.0.0.1", port: "5432", user: "app", database: "shop" },
      [temp, pg],
      new Set([temp.id]),
    );
    expect(found?.id).toBe(pg.id);
    // Only a temporary one: reused (no second temporary profile).
    expect(
      findMatchingProfile(
        { driver: "postgres", host: "::1", user: "app", database: "shop" },
        [temp],
        new Set([temp.id]),
      )?.id,
    ).toBe(temp.id);
    // Another user or database is another connection.
    expect(
      findMatchingProfile({ driver: "postgres", host: "127.0.0.1", user: "x", database: "shop" }, [
        pg,
      ]),
    ).toBeNull();
  });

  it("opens saved connections by id and reports bad links", () => {
    expect(
      planOpen({ kind: "url", url: `tabler://connection/${lite.id}?table=users` }, ctx()),
    ).toMatchObject({ kind: "profile", profileId: lite.id, target: { table: "users" } });
    expect(
      planOpen(
        { kind: "url", url: "tabler://connection/55555555-5555-4555-8555-555555555555" },
        ctx(),
      ),
    ).toMatchObject({ kind: "error" });
    expect(planOpen({ kind: "url", url: "ftp://nope" }, ctx()).kind).toBe("error");
  });
});

describe("planOpen: files", () => {
  it("opens SQLite files through their saved connection, or a temporary one", () => {
    expect(planOpen({ kind: "file", path: "/Users/me/app.sqlite" }, ctx())).toMatchObject({
      kind: "profile",
      profileId: lite.id,
    });
    expect(planOpen({ kind: "file", path: "/tmp/other.db" }, ctx())).toMatchObject({
      kind: "temporary",
      values: { driver: "sqlite", sqlitePath: "/tmp/other.db", name: "other.db" },
    });
    expect(planOpen({ kind: "file", path: "/tmp/x.SQLITE3" }, ctx()).kind).toBe("temporary");
  });

  it("opens SQL scripts in the active connection, or asks", () => {
    expect(
      planOpen({ kind: "file", path: "/q/report.sql" }, ctx({ activeConnectionId: "c1" })),
    ).toEqual({ kind: "sqlFile", path: "/q/report.sql", connectionId: "c1" });
    expect(planOpen({ kind: "file", path: "/q/dump.sql.gz" }, ctx())).toEqual({
      kind: "sqlFile",
      path: "/q/dump.sql.gz",
      connectionId: null,
    });
    expect(planOpen({ kind: "file", path: "/q/notes.txt" }, ctx()).kind).toBe("error");
  });
});

describe("filters from links", () => {
  it("maps TablePlus operators to filter ops", () => {
    expect(filterOpOf("=")).toBe("eq");
    expect(filterOpOf("!=")).toBe("ne");
    expect(filterOpOf(">=")).toBe("ge");
    expect(filterOpOf("like")).toBe("like");
    expect(filterOpOf("NOT  IN")).toBe("notIn");
    expect(filterOpOf("is null")).toBe("isNull");
    expect(filterOpOf("Contains")).toBe("contains");
    expect(filterOpOf("contains case insensitive")).toBe("containsCase");
    expect(filterOpOf("hasPrefix")).toBe("hasPrefix");
    expect(filterOpOf("starts with")).toBe("hasPrefix");
    expect(filterOpOf("~~~")).toBeNull();
  });

  it("applies column conditions; raw SQL lands unchecked", () => {
    expect(filterRowOf({ kind: "condition", column: "id", operator: ">", value: "5" })).toEqual({
      column: "id",
      op: "gt",
      value: "5",
      value2: "",
      enabled: true,
    });
    expect(
      filterRowOf({ kind: "condition", column: "n", operator: "BETWEEN", value: "1,10" }),
    ).toMatchObject({ op: "between", value: "1", value2: "10" });
    expect(filterRowOf({ kind: "raw", sql: "a = 1" })).toEqual({
      column: RAW_SQL_COLUMN,
      op: "eq",
      value: "a = 1",
      value2: "",
      enabled: false,
    });
    expect(filterRowOf({ kind: "condition", column: "id", operator: "??", value: "" })).toEqual({
      error: "Unknown filter operator “??”",
    });
  });

  it("maps * to the any-column pseudo-column explicitly", () => {
    expect(
      filterRowOf({ kind: "condition", column: "*", operator: "contains", value: "x" }),
    ).toEqual({
      column: ANY_COLUMN,
      op: "contains",
      value: "x",
      value2: "",
      enabled: true,
    });
  });

  it("never lets a link name a filter pseudo-column (no raw SQL through column=__raw_sql__)", () => {
    const injected = "1=1) AND (SELECT pg_sleep(30)) IS NOT NULL AND (1=1";
    expect(
      filterRowOf({ kind: "condition", column: RAW_SQL_COLUMN, operator: "=", value: injected }),
    ).toEqual({ error: `Unknown column “${RAW_SQL_COLUMN}”` });
    expect(
      filterRowOf({ kind: "condition", column: ANY_COLUMN, operator: "=", value: "x" }),
    ).toEqual({ error: `Unknown column “${ANY_COLUMN}”` });
    // End to end from the URL: the parsed link yields an error, not a (raw) filter row.
    const link = parseDeepLink(
      `postgres://app@127.0.0.1:5432/shop?table=users&column=${RAW_SQL_COLUMN}&operation=%3D&value=${encodeURIComponent(injected)}`,
    );
    if (link.kind !== "connection" || !link.parsed.target?.filter)
      throw new Error("link has no filter");
    expect(filterRowOf(link.parsed.target.filter)).toMatchObject({ error: expect.any(String) });
  });
});
