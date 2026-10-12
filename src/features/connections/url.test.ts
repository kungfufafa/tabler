import { describe, expect, it } from "vitest";
import type { ConnectionProfile } from "@/lib/ipc";
import { parseConnectionUrl, profileToUrl } from "./url";

const base: ConnectionProfile = {
  id: "00000000-0000-0000-0000-000000000001",
  name: "Local Postgres",
  color: "blue",
  group: null,
  driver: "postgres",
  host: "127.0.0.1",
  port: 5432,
  user: "app",
  database: "tabler_dev",
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

describe("parseConnectionUrl", () => {
  it("reads a PostgreSQL URL with credentials, port, database and options", () => {
    const r = parseConnectionUrl(
      "postgresql://ad%40min:p%40ss%3Aw@db.example.com:6543/my%20db?sslmode=require&name=Prod&color=red",
    );
    expect(r.values).toMatchObject({
      driver: "postgres",
      host: "db.example.com",
      port: "6543",
      user: "ad@min",
      database: "my db",
      ssl: "require",
      name: "Prod",
      color: "red",
    });
    expect(r.password).toBe("p@ss:w");
  });

  it("fills defaults: port per driver, a name from user and host, no password", () => {
    const r = parseConnectionUrl("mysql://root@127.0.0.1/tabler_dev");
    expect(r.values).toMatchObject({
      driver: "mysql",
      port: "3306",
      name: "root@127.0.0.1",
      database: "tabler_dev",
    });
    expect(r.password).toBeNull();
    expect(parseConnectionUrl("mariadb://h").values).toMatchObject({ driver: "mysql", user: "" });
  });

  it("accepts jdbc: prefixes, IPv6 hosts and MySQL ssl-mode names", () => {
    expect(parseConnectionUrl("jdbc:postgresql://[::1]:5432/x").values.host).toBe("::1");
    expect(parseConnectionUrl("mysql://u@h/d?ssl-mode=VERIFY_IDENTITY").values.ssl).toBe(
      "verify-full",
    );
  });

  it("reads SQLite file URLs", () => {
    expect(parseConnectionUrl("sqlite:///Users/me/app.sqlite").values).toMatchObject({
      driver: "sqlite",
      sqlitePath: "/Users/me/app.sqlite",
      name: "app.sqlite",
    });
    expect(parseConnectionUrl("sqlite:/tmp/a%20b.db").values.sqlitePath).toBe("/tmp/a b.db");
  });

  it("reads TablePlus's env and safe-mode parameters", () => {
    const r = parseConnectionUrl(
      "postgres://u@h/d?env=production&safeModeLevel=2&advancedSafeModeLevel=1&statusColor=red",
    );
    expect(r.values).toMatchObject({ tag: "production", safeMode: "safeNonSelect", color: "red" });
    expect(parseConnectionUrl("mysql://u@h?enviroment=Staging").values.tag).toBe("staging");
    expect(parseConnectionUrl("mysql://u@h?safeModeLevel=1").values.safeMode).toBe("alert");
    const ignored = parseConnectionUrl("mysql://u@h?env=moon&safeModeLevel=9").values;
    expect(ignored.tag).toBeUndefined();
    expect(ignored.safeMode).toBeUndefined();
  });

  it("rejects what is not a connection URL", () => {
    expect(() => parseConnectionUrl("https://example.com")).toThrow(/postgres/);
    expect(() => parseConnectionUrl("not a url")).toThrow();
    expect(() => parseConnectionUrl("sqlite://")).toThrow(/path/);
    expect(() => parseConnectionUrl("sqlite://relative.db")).toThrow(/absolute/);
    expect(parseConnectionUrl("sqlite://~/a.db").values.sqlitePath).toBe("~/a.db");
  });
});

describe("profileToUrl", () => {
  it("round-trips through parseConnectionUrl", () => {
    const p = { ...base, user: "a@b", database: "x/y", ssl: "verify-full" as const };
    const url = profileToUrl(p, "s3cr:t");
    expect(url).toBe(
      "postgresql://a%40b:s3cr%3At@127.0.0.1:5432/x%2Fy?name=Local+Postgres&sslmode=verify-full",
    );
    const back = parseConnectionUrl(url);
    expect(back.values).toMatchObject({ user: "a@b", database: "x/y", name: "Local Postgres" });
    expect(back.password).toBe("s3cr:t");
  });

  it("carries the tag and a safe mode other than silent", () => {
    const p: ConnectionProfile = {
      ...base,
      options: { ...base.options, tag: "staging", safeMode: "alertNonSelect" },
    };
    const url = profileToUrl(p);
    expect(url).toBe(
      "postgresql://app@127.0.0.1:5432/tabler_dev?name=Local+Postgres&env=staging&safeModeLevel=1&advancedSafeModeLevel=1",
    );
    expect(parseConnectionUrl(url).values).toMatchObject({
      tag: "staging",
      safeMode: "alertNonSelect",
    });
    const silent = { ...base, options: { ...base.options, safeMode: "silent" as const } };
    expect(profileToUrl(silent)).not.toContain("safeModeLevel");
  });

  it("leaves the password out unless given", () => {
    expect(profileToUrl(base)).toBe(
      "postgresql://app@127.0.0.1:5432/tabler_dev?name=Local+Postgres",
    );
    expect(profileToUrl({ ...base, driver: "sqlite", sqlitePath: "/tmp/a b.db" })).toBe(
      "sqlite:///tmp/a%20b.db",
    );
  });
});
