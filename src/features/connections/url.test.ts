import { describe, expect, it } from "vitest";
import type { ConnectionProfile } from "@/lib/ipc";
import { ACCENT_PRESETS } from "@/lib/theme";
import { connectionFormSchema, formToProfileInput, urlToForm } from "./schema";
import {
  mysqlVersionOf,
  parseConnectionUrl,
  parseDeepLink,
  presetForColor,
  profileToUrl,
  quoteParamValue,
  splitParams,
  statusColorOf,
} from "./url";

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
    // Custom tags come through; an unknown safe-mode level is ignored.
    const custom = parseConnectionUrl("mysql://u@h?env=QA%20Lab&safeModeLevel=9").values;
    expect(custom.tag).toBe("QA Lab");
    expect(custom.safeMode).toBeUndefined();
    expect(parseConnectionUrl(`mysql://u@h?env=${"x".repeat(41)}`).values.tag).toBeUndefined();
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
      "postgresql://a%40b:s3cr%3At@127.0.0.1:5432/x%2Fy?name=Local+Postgres&statusColor=2279DC&sslmode=verify-full",
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
      "postgresql://app@127.0.0.1:5432/tabler_dev?name=Local+Postgres&statusColor=2279DC&env=staging&safeModeLevel=1&advancedSafeModeLevel=1",
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
      "postgresql://app@127.0.0.1:5432/tabler_dev?name=Local+Postgres&statusColor=2279DC",
    );
    // SQLite: the file (the password slot is its encryption key, never part of the URL).
    expect(profileToUrl({ ...base, driver: "sqlite", sqlitePath: "/tmp/a b.db" }, "key")).toBe(
      "sqlite:///tmp/a%20b.db?name=Local+Postgres&statusColor=2279DC",
    );
  });

  it("round-trips a custom colour unchanged", () => {
    for (const custom of ["#ff8a00", "#007f3d", "#123456", "#fedcba"]) {
      const url = profileToUrl({ ...base, color: custom });
      expect(new URL(url).searchParams.get("statusColor")).toBe(custom.slice(1).toUpperCase());
      expect(parseConnectionUrl(url).values.color, custom).toBe(custom);
    }
  });

  it("writes statusColor as hex, and every preset reads back as itself", () => {
    expect(statusColorOf(null)).toBeNull();
    expect(statusColorOf("#a1b2c3")).toBe("A1B2C3");
    expect(statusColorOf("nope")).toBeNull();
    for (const color of ACCENT_PRESETS) {
      const url = profileToUrl({ ...base, color });
      const hex = new URL(url).searchParams.get("statusColor");
      expect(hex, color).toMatch(/^[0-9A-F]{6}$/);
      expect(parseConnectionUrl(url).values.color, color).toBe(color);
    }
    // A custom colour is written as its hex; no colour, no parameter.
    expect(
      new URL(profileToUrl({ ...base, color: "#007F3D" })).searchParams.get("statusColor"),
    ).toBe("007F3D");
    expect(profileToUrl({ ...base, color: null })).not.toContain("statusColor");
  });

  it("writes the +ssh form for an SSH profile and reads it back", () => {
    const ssh: ConnectionProfile = {
      ...base,
      name: "Prod via bastion",
      color: "red",
      ssh: {
        host: "bastion.example.com",
        port: 2222,
        user: "deploy",
        auth: "key",
        keyPath: null,
        localPort: 5433,
        keepaliveSecs: null,
      },
      options: { ...base.options, tag: "production" },
    };
    const url = profileToUrl(ssh, "dbpass");
    expect(url).toBe(
      "postgresql+ssh://deploy@bastion.example.com:2222/app:dbpass@127.0.0.1:5432/tabler_dev?name=Prod+via+bastion&statusColor=DB4241&usePrivateKey=true&env=production",
    );
    const back = parseConnectionUrl(url);
    expect(back.password).toBe("dbpass");
    expect(back.values).toMatchObject({
      driver: "postgres",
      host: "127.0.0.1",
      port: "5432",
      user: "app",
      database: "tabler_dev",
      name: "Prod via bastion",
      color: "red",
      tag: "production",
      sshEnabled: true,
      sshHost: "bastion.example.com",
      sshPort: "2222",
      sshUser: "deploy",
      sshAuth: "key",
    });
    // The default port and the password-less form; MySQL says mysql+ssh.
    const mysql: ConnectionProfile = {
      ...ssh,
      driver: "mysql",
      port: 3306,
      ssh: { ...ssh.ssh!, port: 22, auth: "agent" },
    };
    expect(profileToUrl(mysql)).toMatch(
      /^mysql\+ssh:\/\/deploy@bastion\.example\.com\/app@127\.0\.0\.1:3306\/tabler_dev\?name=/,
    );
    expect(profileToUrl(mysql)).not.toContain("usePrivateKey");
    expect(parseConnectionUrl(profileToUrl(mysql)).values).toMatchObject({
      sshPort: "22",
      sshHost: "bastion.example.com",
    });
  });

  it("writes SQLite over SSH with the file's path on the server", () => {
    const lite: ConnectionProfile = {
      ...base,
      name: "remote.db",
      color: null,
      driver: "sqlite",
      host: null,
      port: null,
      user: null,
      database: null,
      sqlitePath: "/srv/app/data.sqlite",
      ssh: {
        host: "files.example.com",
        port: 22,
        user: "deploy",
        auth: "agent",
        keyPath: null,
        localPort: null,
        keepaliveSecs: null,
      },
    };
    const url = profileToUrl(lite);
    expect(url).toBe("sqlite+ssh://deploy@files.example.com/srv/app/data.sqlite?name=remote.db");
    expect(parseConnectionUrl(url).values).toMatchObject({
      driver: "sqlite",
      sqlitePath: "/srv/app/data.sqlite",
      sshHost: "files.example.com",
      sshUser: "deploy",
      name: "remote.db",
    });
    const home = profileToUrl({ ...lite, sqlitePath: "~/db/app.sqlite" });
    expect(parseConnectionUrl(home).values.sqlitePath).toBe("~/db/app.sqlite");
  });

  it("writes the MySQL Ver. menu and the lazy-load option, and reads them back", () => {
    const my: ConnectionProfile = {
      ...base,
      driver: "mysql",
      port: 3306,
      options: { ...base.options, mysqlVersion: "mariadb", lazyLoad: true },
    };
    const url = profileToUrl(my);
    const params = new URL(url).searchParams;
    expect(params.get("driverVersion")).toBe("1");
    expect(params.get("lazyload")).toBe("true");
    const back = parseConnectionUrl(url);
    expect(back.values.mysqlVersion).toBe("mariadb");
    expect(back.extras).toMatchObject({ driverVersion: 1, lazyload: true });
    // Defaults add nothing; PostgreSQL has no "Ver.".
    const plain = profileToUrl({
      ...my,
      options: { ...my.options, mysqlVersion: null, lazyLoad: null },
    });
    expect(plain).not.toMatch(/driverVersion|lazyload/);
    expect(profileToUrl({ ...base, options: { ...base.options, lazyLoad: false } })).toContain(
      "lazyload=false",
    );
  });

  it("reads driverVersion as MySQL's Ver. menu, only for MySQL and only in range", () => {
    expect(parseConnectionUrl("mysql://u@h/d?driverVersion=2").values.mysqlVersion).toBe("old");
    expect(parseConnectionUrl("mysql://u@h/d?driverVersion=0").values.mysqlVersion).toBe("default");
    expect(parseConnectionUrl("mysql://u@h/d?driverVersion=9").values.mysqlVersion).toBeUndefined();
    expect(parseConnectionUrl("postgres://u@h/d?driverVersion=1").values.mysqlVersion).toBe(
      undefined,
    );
    expect(parseConnectionUrl("mysql+ssh://me@box/u@h/d?driverVersion=1").values.mysqlVersion).toBe(
      "mariadb",
    );
    expect(mysqlVersionOf(null)).toBeNull();
    expect(mysqlVersionOf(1)).toBe("mariadb");
  });
});

describe("MySQL socket URLs", () => {
  it("reads ?socket= (with or without a host) and writes it back", () => {
    const r = parseConnectionUrl("mysql://root@localhost/shop?socket=%2Ftmp%2Fmysql.sock");
    expect(r.values).toMatchObject({
      driver: "mysql",
      host: "localhost",
      user: "root",
      database: "shop",
      useSocket: true,
      socketPath: "/tmp/mysql.sock",
    });
    const hostless = parseConnectionUrl("mysql://root:pw@/shop?socket=/var/run/mysqld/mysqld.sock");
    expect(hostless.values).toMatchObject({
      host: "localhost",
      useSocket: true,
      socketPath: "/var/run/mysqld/mysqld.sock",
    });
    expect(hostless.password).toBe("pw");
    // An empty socket: detected when connecting.
    expect(parseConnectionUrl("mysql://root@localhost/shop?socket=").values).toMatchObject({
      useSocket: true,
      socketPath: "",
    });
    // Without the parameter (and for PostgreSQL) nothing changes.
    expect(parseConnectionUrl("mysql://root@db/shop").values.useSocket).toBeUndefined();
    expect(
      parseConnectionUrl("postgres://u@db/x?socket=/tmp/.s.PGSQL.5432").values.useSocket,
    ).toBeUndefined();
    expect(() => parseConnectionUrl("mysql://root@/shop")).toThrow();

    const p: ConnectionProfile = {
      ...base,
      driver: "mysql",
      host: "localhost",
      port: 3306,
      options: { ...base.options, useSocket: true, socketPath: "/tmp/mysql.sock" },
    };
    const url = profileToUrl(p);
    expect(new URL(url).searchParams.get("socket")).toBe("/tmp/mysql.sock");
    expect(parseConnectionUrl(url).values).toMatchObject({
      useSocket: true,
      socketPath: "/tmp/mysql.sock",
    });
    expect(profileToUrl({ ...p, options: { ...p.options, useSocket: null } })).not.toContain(
      "socket",
    );
  });
});

describe("TablePlus URL parameters", () => {
  it("keeps a custom statusColor as its hex; a preset name or a preset's own hex is the preset", () => {
    const color = (q: string) => parseConnectionUrl(`postgres://u@h/d?${q}`).values.color;
    // Not a preset: the colour itself, normalised to #rrggbb.
    expect(color("statusColor=FF8A00")).toBe("#ff8a00");
    expect(color("statusColor=ff8a00")).toBe("#ff8a00");
    expect(color("statusColor=%23FF8A00")).toBe("#ff8a00");
    expect(color("statusColor=f80")).toBe("#ff8800");
    expect(color("statusColor=007F3D")).toBe("#007f3d");
    expect(color("statusColor=%23F13A3A")).toBe("#f13a3a");
    expect(color("color=1E88E5")).toBe("#1e88e5");
    // A preset name, or the exact hex of a preset (what Copy as URL writes), stays the preset.
    expect(color("statusColor=orange")).toBe("orange");
    expect(color("statusColor=ORANGE")).toBe("orange");
    expect(color("statusColor=DE6F00")).toBe("orange");
    expect(color("statusColor=%23de6f00")).toBe("orange");
    expect(color("statusColor=2279DC")).toBe("blue");
    // Neither a colour nor a name: no colour.
    expect(color("statusColor=nope")).toBeUndefined();
    expect(color("statusColor=ff8a0")).toBeUndefined();
    expect(color("name=x")).toBeUndefined();
    // The nearest preset is still what presetForColor answers (names and callers that need one).
    expect(presetForColor("1E88E5")).toBe("blue");
    expect(presetForColor("FFA500")).toBe("orange");
    expect(presetForColor("FFD600")).toBe("yellow");
    expect(presetForColor("00ACC1")).toBe("teal");
    expect(presetForColor("8E24AA")).toBe("purple");
    expect(presetForColor("808080")).toBe("gray");
    expect(presetForColor("purple")).toBe("purple");
    expect(presetForColor("nope")).toBeNull();
  });

  it("reads tLSMode, windowMode, driverVersion and lazyload", () => {
    const r = parseConnectionUrl(
      "postgres://u@h/d?tLSMode=2&windowMode=isolated&driverVersion=3&lazyload=true",
    );
    expect(r.values.ssl).toBe("require");
    expect(r.extras).toEqual({ windowMode: "isolated", driverVersion: 3, lazyload: true });
    // An explicit sslmode wins over tLSMode.
    expect(parseConnectionUrl("postgres://u@h/d?tLSMode=0&sslmode=verify-full").values.ssl).toBe(
      "verify-full",
    );
    expect(parseConnectionUrl("mysql://u@h/d?tLSMode=9&lazyload=0").values.ssl).toBeUndefined();
    expect(parseConnectionUrl("mysql://u@h/d?lazyload=0").extras?.lazyload).toBe(false);
    expect(parseConnectionUrl("mysql://u@h/d").extras).toEqual({
      windowMode: null,
      driverVersion: null,
      lazyload: null,
    });
  });

  it("reads the target: schema, table or view, and a column filter", () => {
    expect(
      parseConnectionUrl(
        "postgres://u@h/d?schema=sales&table=orders&column=id&operation=%3E%3D&value=10",
      ).target,
    ).toEqual({
      schema: "sales",
      table: "orders",
      filter: { kind: "condition", column: "id", operator: ">=", value: "10" },
    });
    expect(
      parseConnectionUrl("mysql://u@h/d?view=active_users&column=name&operator=Contains&value=an")
        .target,
    ).toEqual({
      schema: null,
      table: "active_users",
      filter: { kind: "condition", column: "name", operator: "Contains", value: "an" },
    });
    // A column without operator: `=`; without value: empty.
    expect(parseConnectionUrl("mysql://u@h/d?table=t&column=c").target?.filter).toEqual({
      kind: "condition",
      column: "c",
      operator: "=",
      value: "",
    });
    expect(parseConnectionUrl("mysql://u@h/d").target).toBeNull();
  });

  it("reads a raw condition from condition, raw or query", () => {
    for (const key of ["condition", "raw", "query"]) {
      expect(
        parseConnectionUrl(
          `postgres://u@h/d?table=t&${key}=${encodeURIComponent("a > 1 AND b IS NULL")}`,
        ).target?.filter,
      ).toEqual({ kind: "raw", sql: "a > 1 AND b IS NULL" });
    }
    // The raw condition wins over a column filter.
    expect(
      parseConnectionUrl("postgres://u@h/d?table=t&column=c&value=1&raw=x%3D2").target?.filter,
    ).toEqual({ kind: "raw", sql: "x=2" });
  });

  it("reads SQLite targets and names", () => {
    const r = parseConnectionUrl("sqlite:///Users/me/app.sqlite?name=App&table=users&env=testing");
    expect(r.values).toMatchObject({
      sqlitePath: "/Users/me/app.sqlite",
      name: "App",
      tag: "testing",
    });
    expect(r.target?.table).toBe("users");
  });
});

describe("+ssh URLs", () => {
  it("reads the SSH server, then the database as the SSH server sees it", () => {
    const r = parseConnectionUrl(
      "postgresql+ssh://sshuser:ssh%40pass@bastion.example.com:2222/dbuser:dbpass@127.0.0.1:5432/app?table=users",
    );
    expect(r.values).toMatchObject({
      driver: "postgres",
      host: "127.0.0.1",
      port: "5432",
      user: "dbuser",
      database: "app",
      sshEnabled: true,
      sshHost: "bastion.example.com",
      sshPort: "2222",
      sshUser: "sshuser",
      sshAuth: "password",
      sshPassword: "ssh@pass",
      name: "dbuser@127.0.0.1 via bastion.example.com",
    });
    expect(r.password).toBe("dbpass");
    expect(r.target?.table).toBe("users");
  });

  it("uses a key without an SSH password, or with usePrivateKey", () => {
    const r = parseConnectionUrl("mysql+ssh://deploy@bastion/root@db.internal/shop?name=Shop");
    expect(r.values).toMatchObject({
      driver: "mysql",
      sshHost: "bastion",
      sshPort: "22",
      sshUser: "deploy",
      sshAuth: "key",
      host: "db.internal",
      name: "Shop",
    });
    expect(r.values.sshPassword).toBeUndefined();
    expect(
      parseConnectionUrl("mysql+ssh://a:b@h/root@db/x?usePrivateKey=true").values.sshAuth,
    ).toBe("key");
    expect(() => parseConnectionUrl("postgres+ssh://bastion")).toThrow(/database/);
    expect(() => parseConnectionUrl("postgres+ssh://@/u@h/db")).toThrow(/SSH host/);
  });

  it("reads SQLite files on the SSH server", () => {
    expect(parseConnectionUrl("sqlite+ssh://me@box:2200/~/data/app.db").values).toMatchObject({
      driver: "sqlite",
      sqlitePath: "~/data/app.db",
      sshEnabled: true,
      sshHost: "box",
      sshPort: "2200",
      sshUser: "me",
      sshAuth: "key",
    });
    expect(parseConnectionUrl("sqlite+ssh://me@box/srv/app.db?name=X").values).toMatchObject({
      sqlitePath: "/srv/app.db",
      name: "X",
    });
    expect(() => parseConnectionUrl("sqlite+ssh://me@box/")).toThrow(/path/);
  });
});

/** The saved profile that "New connection from URL…" would make from a URL. */
function profileFromUrl(url: string, password: string | null = null): ConnectionProfile {
  const parsed = parseConnectionUrl(url);
  const form = urlToForm({ values: parsed.values, password }, 10);
  form.name ||= "from url";
  const checked = connectionFormSchema.safeParse(form);
  if (!checked.success) throw new Error(checked.error.issues[0]?.message);
  return formToProfileInput(checked.data, null, "00000000-0000-0000-0000-0000000000aa").profile;
}

describe("PostgreSQL libpq parameters in the URL", () => {
  const url =
    "postgres://u@h/d?jit=off&application_name=rep&options=-c%20geqo%3Doff&sslmode=require";

  it("keeps the extra parameters in the form's Parameters field", () => {
    const { values } = parseConnectionUrl(url);
    expect(values.pgParams).toBe("jit=off application_name=rep options='-c geqo=off'");
    expect(values.ssl).toBe("require");
    // The keys Tabler itself reads never leak into it.
    expect(values.pgParams).not.toMatch(/sslmode/);
    expect(parseConnectionUrl("postgres://u@h/d?statement_timeout=5000").values.pgParams).toBe(
      "statement_timeout=5000",
    );
    expect(parseConnectionUrl("postgres://u@h/d?sslmode=require").values.pgParams).toBeUndefined();
    expect(parseConnectionUrl("postgres://u@h/d").values.pgParams).toBeUndefined();
  });

  it("maps sslnegotiation, channel_binding and connect_timeout to their own fields", () => {
    const { values } = parseConnectionUrl(
      "postgres://u@h/d?sslmode=require&sslnegotiation=direct&channel_binding=disable&connect_timeout=7&jit=off",
    );
    expect(values).toMatchObject({
      pgSslNegotiation: "direct",
      pgDisableChannelBinding: true,
      connectTimeoutSecs: "7",
      pgParams: "jit=off",
    });
    // The other values of those keys are the defaults: nothing is set, nothing goes to pgParams.
    const defaults = parseConnectionUrl(
      "postgres://u@h/d?sslnegotiation=postgres&channel_binding=prefer&connect_timeout=0",
    ).values;
    expect(defaults.pgSslNegotiation).toBeUndefined();
    expect(defaults.pgDisableChannelBinding).toBeUndefined();
    expect(defaults.connectTimeoutSecs).toBeUndefined();
    expect(defaults.pgParams).toBeUndefined();
  });

  it("never passes Tabler's own query keys, the form's libpq keys or unusable names on", () => {
    const own = [
      "name=n",
      "statusColor=red",
      "env=staging",
      "enviroment=x",
      "tag=x",
      "safeModeLevel=1",
      "advancedSafeModeLevel=1",
      "sslmode=require",
      "ssl-mode=require",
      "ssl_mode=require",
      "tLSMode=2",
      "usePrivateKey=true",
      "socket=/tmp/s",
      "driverVersion=1",
      "lazyload=true",
      "windowMode=isolated",
      "schema=s",
      "table=t",
      "view=v",
      "column=c",
      "operation=%3D",
      "operator=%3D",
      "value=1",
      "condition=1%3D1",
      "raw=1%3D1",
      "query=1%3D1",
      "host=x",
      "hostaddr=1.2.3.4",
      "port=1",
      "user=u",
      "password=p",
      "dbname=d",
      "sslrootcert=/ca",
      "sslcert=/c",
      "sslkey=/k",
      "client_encoding=latin1",
      // libpq connection keys that are no server settings (they would fail the connect):
      "keepalives=1",
      "sslcompression=0",
      "gssencmode=disable",
      "ssl=true",
      // not a setting name
      "foo-bar=1",
      "1x=1",
    ].join("&");
    expect(parseConnectionUrl(`postgres://u@h/d?${own}&jit=off`).values.pgParams).toBe("jit=off");
    // Case does not matter for the keys (libpq's are lower case; the field lower-cases them).
    expect(parseConnectionUrl("postgres://u@h/d?JIT=off&Application_Name=x").values.pgParams).toBe(
      "jit=off application_name=x",
    );
  });

  it("quotes values that need it, and the Parameters field reads them back", () => {
    expect(quoteParamValue("off")).toBe("off");
    expect(quoteParamValue("")).toBe("''");
    expect(quoteParamValue("a b")).toBe("'a b'");
    expect(quoteParamValue("it's")).toBe("'it\\'s'");
    expect(quoteParamValue("a\\b")).toBe("'a\\\\b'");
    expect(quoteParamValue("100%")).toBe("'100%'");
    expect(quoteParamValue("x&y")).toBe("'x&y'");
    expect(quoteParamValue("a=b")).toBe("a=b");
    const nasty = ["it's", "a\\b", "x y\tz", "100%2F", "p&q", "", "a+b", "line\nbreak", "q'\\'"];
    for (const v of nasty)
      expect(splitParams(`k=${quoteParamValue(v)} z=1`), JSON.stringify(v)).toEqual([
        ["k", v],
        ["z", "1"],
      ]);
    // The backend's grammar: separators, spaces around =, quotes, percent-decoding, lower case.
    expect(splitParams("A=1  b = 2\nc='x y \\' z'&d=%2Fp+q ?e=")).toEqual([
      ["a", "1"],
      ["b", "2"],
      ["c", "x y ' z"],
      ["d", "/p q"],
      ["e", ""],
    ]);
    // A malformed rest ends the list instead of throwing.
    expect(splitParams("a=1 lonely b=2")).toEqual([["a", "1"]]);
    expect(splitParams("a=1 x='open")).toEqual([["a", "1"]]);
    expect(splitParams("")).toEqual([]);
  });

  it("round-trips: URL → form → profile → Copy as URL → form", () => {
    const original =
      "postgres://u@h/d?sslmode=verify-full&sslnegotiation=direct&channel_binding=disable&connect_timeout=9&jit=off&application_name=rep&options=-c%20geqo%3Doff%20-c%20work_mem%3D1MB&app.setting=it%27s%20%25";
    const profile = profileFromUrl(original);
    expect(profile.options).toMatchObject({
      pgSslNegotiation: "direct",
      pgDisableChannelBinding: true,
      connectTimeoutSecs: 9,
      pgParams:
        "jit=off application_name=rep options='-c geqo=off -c work_mem=1MB' app.setting='it\\'s %'",
    });
    const copied = profileToUrl(profile);
    const q = new URL(copied).searchParams;
    expect(q.get("sslnegotiation")).toBe("direct");
    expect(q.get("channel_binding")).toBe("disable");
    expect(q.get("connect_timeout")).toBe("9");
    expect(q.get("jit")).toBe("off");
    expect(q.get("application_name")).toBe("rep");
    expect(q.get("options")).toBe("-c geqo=off -c work_mem=1MB");
    expect(q.get("app.setting")).toBe("it's %");
    // And back: the same fields (stable from the second pass on).
    const again = profileFromUrl(copied);
    expect(again.options).toMatchObject({
      pgSslNegotiation: "direct",
      pgDisableChannelBinding: true,
      connectTimeoutSecs: 9,
      pgParams: profile.options.pgParams,
    });
    expect(profileToUrl(again)).toBe(copied);
  });

  it("writes only what is set, and leaves the usual URL as it was", () => {
    expect(profileToUrl(base)).not.toMatch(/sslnegotiation|channel_binding|connect_timeout|jit/);
    const plain = profileToUrl({
      ...base,
      options: { ...base.options, pgParams: "jit=off", pgSslNegotiation: "postgres" },
    });
    expect(plain).toBe(
      "postgresql://app@127.0.0.1:5432/tabler_dev?name=Local+Postgres&statusColor=2279DC&jit=off",
    );
    // A pair of the Parameters field whose key means something else in a URL is left out.
    const clash = profileToUrl({
      ...base,
      options: { ...base.options, pgParams: "name=x env=y jit=off lonely" },
    });
    expect(new URL(clash).searchParams.get("name")).toBe("Local Postgres");
    expect(new URL(clash).searchParams.get("env")).toBeNull();
    expect(new URL(clash).searchParams.get("jit")).toBe("off");
  });

  it("reaches the SSH form and the tabler:// links too", () => {
    const ssh = parseConnectionUrl("postgresql+ssh://me@bastion/u@10.0.0.1:5432/d?jit=off");
    expect(ssh.values).toMatchObject({ sshEnabled: true, pgParams: "jit=off" });
    const inner = encodeURIComponent("postgres://u@h/d?jit=off&sslnegotiation=direct");
    const link = parseDeepLink(`tabler://open?url=${inner}`);
    expect(link.kind === "connection" && link.parsed.values).toMatchObject({
      pgParams: "jit=off",
      pgSslNegotiation: "direct",
    });
  });

  it("leaves MySQL and SQLite URLs alone", () => {
    const my = parseConnectionUrl(
      "mysql://u@h/d?jit=off&sslnegotiation=direct&channel_binding=disable&connect_timeout=7",
    ).values;
    expect(my.pgParams).toBeUndefined();
    expect(my.pgSslNegotiation).toBeUndefined();
    expect(my.pgDisableChannelBinding).toBeUndefined();
    expect(my.connectTimeoutSecs).toBeUndefined();
    expect(parseConnectionUrl("sqlite:///tmp/a.db?jit=off").values.pgParams).toBeUndefined();
    const profile: ConnectionProfile = {
      ...base,
      driver: "mysql",
      options: { ...base.options, pgParams: "jit=off", connectTimeoutSecs: 5 },
    };
    expect(profileToUrl(profile)).not.toMatch(/jit|connect_timeout/);
  });
});

describe("parseDeepLink", () => {
  const id = "0f8fad5b-d9cb-469f-a165-70867728950e";

  it("opens saved connections by id, with a target", () => {
    expect(parseDeepLink(`tabler://connection/${id}?schema=public&table=users`)).toEqual({
      kind: "saved",
      profileId: id,
      target: { schema: "public", table: "users", filter: null },
      extras: { windowMode: null, driverVersion: null, lazyload: null },
    });
    expect(parseDeepLink(`TABLER://?id=${id.toUpperCase()}`)).toMatchObject({
      kind: "saved",
      profileId: id,
    });
  });

  it("wraps connection URLs", () => {
    expect(parseDeepLink("postgres://u@h/d").kind).toBe("connection");
    const inner = encodeURIComponent("mysql://root@127.0.0.1/shop?table=orders");
    const link = parseDeepLink(`tabler://open?url=${inner}`);
    expect(link.kind === "connection" && link.parsed.target?.table).toBe("orders");
  });

  it("rejects links without a connection", () => {
    expect(() => parseDeepLink("tabler://connection/not-an-id")).toThrow(/saved connection/);
    expect(() => parseDeepLink("https://example.com")).toThrow();
  });
});
