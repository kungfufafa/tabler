import { describe, expect, it } from "vitest";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { DEFAULT_PREFS } from "@/stores/prefs";
import {
  connectionDetail,
  connectionFormSchema,
  DEFAULT_CONNECT_TIMEOUT_SECS,
  DEFAULT_FORM,
  formToProfileInput,
  MYSQL_VERSION_LABEL,
  MYSQL_VERSIONS,
  newProfileForm,
  passwordModesFor,
  profileToForm,
  urlToForm,
} from "./schema";
import { mysqlVersionOf, parseConnectionUrl } from "./url";

describe("connection form schema", () => {
  it("requires host and user for server drivers, a path for sqlite", () => {
    const r1 = connectionFormSchema.safeParse({ ...DEFAULT_FORM, name: "x", host: "", user: "" });
    expect(r1.success).toBe(false);
    const paths = r1.success ? [] : r1.error.issues.map((i) => i.path.join("."));
    expect(paths).toEqual(expect.arrayContaining(["host", "user"]));
    const r2 = connectionFormSchema.safeParse({ ...DEFAULT_FORM, name: "x", driver: "sqlite" });
    expect(r2.success).toBe(false);
    const r3 = connectionFormSchema.safeParse({
      ...DEFAULT_FORM,
      name: "x",
      driver: "sqlite",
      sqlitePath: "/tmp/a.db",
    });
    expect(r3.success).toBe(true);
    const r4 = connectionFormSchema.safeParse({
      ...DEFAULT_FORM,
      name: "x",
      user: "u",
      port: "99999",
    });
    expect(r4.success).toBe(false);
  });

  it("round-trips a profile through the form", () => {
    const pg = SEED_PROFILES[0]!;
    const values = profileToForm(pg);
    expect(values.driver).toBe("postgres");
    expect(values.port).toBe("5432");
    const input = formToProfileInput({ ...values, password: "pw" }, pg, pg.id);
    expect(input.profile.id).toBe(pg.id);
    expect(input.profile.host).toBe("127.0.0.1");
    expect(input.profile.port).toBe(5432);
    expect(input.profile.ssh).toBeNull();
    expect(input.password).toBe("pw");
    expect(input.clearPassword).toBe(false);
    const sqlite = formToProfileInput(
      { ...DEFAULT_FORM, name: "s", driver: "sqlite", sqlitePath: "/x.db" },
      null,
      "id-1",
    );
    expect(sqlite.profile.host).toBeNull();
    expect(sqlite.profile.sqlitePath).toBe("/x.db");
    expect(sqlite.password).toBeNull();
    const ssh = formToProfileInput(
      {
        ...DEFAULT_FORM,
        name: "s",
        user: "u",
        sshEnabled: true,
        sshHost: "b",
        sshUser: "me",
        sshAuth: "password",
      },
      null,
      "id-2",
    );
    expect(ssh.profile.ssh).toEqual({
      host: "b",
      port: 22,
      user: "me",
      auth: "password",
      keyPath: null,
      localPort: null,
      keepaliveSecs: null,
      passwordMode: "keychain",
    });
  });

  it("keeps the PostgreSQL flavor and driver params of an edited profile", () => {
    const pg = SEED_PROFILES[0]!;
    const cockroach = {
      ...pg,
      port: null,
      options: {
        ...pg.options,
        pgFlavor: "cockroachdb" as const,
        driverParams: { cluster: "blue-fox-42" },
      },
    };
    const edited = formToProfileInput(
      { ...profileToForm(cockroach), name: "renamed" },
      cockroach,
      cockroach.id,
    );
    expect(edited.profile.port).toBeNull();
    expect(edited.profile.options.pgFlavor).toBe("cockroachdb");
    expect(edited.profile.options.driverParams).toEqual({ cluster: "blue-fox-42" });
    // Another driver has neither.
    const mysql = formToProfileInput(
      { ...profileToForm(cockroach), driver: "mysql" },
      cockroach,
      cockroach.id,
    );
    expect(mysql.profile.options.pgFlavor).toBeNull();
    expect(mysql.profile.options.driverParams).toBeNull();
    // A new profile has neither.
    const fresh = formToProfileInput({ ...newProfileForm(15), name: "n", user: "u" }, null, "id-1");
    expect(fresh.profile.options.pgFlavor).toBeNull();
    expect(fresh.profile.options.driverParams).toBeNull();
  });

  it("round-trips the SQLite foreign-keys switch (on by default)", () => {
    expect(newProfileForm(15).sqliteForeignKeys).toBe(true);
    const base = { ...DEFAULT_FORM, name: "s", driver: "sqlite" as const, sqlitePath: "/x.db" };
    const on = formToProfileInput(base, null, "id-1");
    expect(on.profile.options.sqliteForeignKeys).toBe(true);
    const off = formToProfileInput({ ...base, sqliteForeignKeys: false }, null, "id-1");
    expect(off.profile.options.sqliteForeignKeys).toBe(false);
    expect(profileToForm(off.profile).sqliteForeignKeys).toBe(false);
    expect(profileToForm(on.profile).sqliteForeignKeys).toBe(true);
    // Editing keeps the stored choice unless the switch changes it.
    const stored = formToProfileInput(profileToForm(off.profile), off.profile, "id-1");
    expect(stored.profile.options.sqliteForeignKeys).toBe(false);
    expect(connectionFormSchema.safeParse({ ...base, sqliteForeignKeys: false }).success).toBe(
      true,
    );
  });

  it("starts new profiles with the connect-timeout preference", () => {
    const values = newProfileForm(30);
    expect(values).toMatchObject({ port: "5432", connectTimeoutSecs: "30" });
    const input = formToProfileInput({ ...values, name: "n", user: "u" }, null, "id-1");
    expect(input.profile.options.connectTimeoutSecs).toBe(30);
  });

  it("new connections are tagged local; old profiles keep no tag until edited", () => {
    expect(newProfileForm(15).tag).toBe("local");
    const old = SEED_PROFILES.find((p) => p.id === "p-rotated-ssh")!;
    expect(old.options.tag).toBeNull();
    expect(profileToForm(old).tag).toBeNull();
    const kept = formToProfileInput(profileToForm(old), old, old.id);
    expect(kept.profile.options.tag).toBeNull();
    const tagged = formToProfileInput({ ...profileToForm(old), tag: "staging" }, old, old.id);
    expect(tagged.profile.options.tag).toBe("staging");
    expect(connectionFormSchema.safeParse({ ...DEFAULT_FORM, tag: "prod" }).success).toBe(false);
  });

  it("prefills a new connection from a URL, password included", () => {
    const values = urlToForm(
      parseConnectionUrl("mysql://root:pw@db.local:3307/shop?name=Shop"),
      20,
    );
    expect(values).toMatchObject({
      driver: "mysql",
      name: "Shop",
      host: "db.local",
      port: "3307",
      user: "root",
      database: "shop",
      password: "pw",
      tag: "local",
      connectTimeoutSecs: "20",
    });
    expect(connectionFormSchema.safeParse(values).success).toBe(true);
    const sqlite = urlToForm(parseConnectionUrl("sqlite:///tmp/a.db"), 15);
    expect(sqlite).toMatchObject({ driver: "sqlite", sqlitePath: "/tmp/a.db", password: "" });
    expect(connectionFormSchema.safeParse(sqlite).success).toBe(true);
  });
});

describe("connection form — TablePlus parity fields", () => {
  const pgForm = { ...DEFAULT_FORM, name: "n", user: "u" };

  it("round-trips password mode, SSL client files, safe mode and bootstrap commands", () => {
    const input = formToProfileInput(
      {
        ...pgForm,
        passwordMode: "ask",
        password: "typed",
        sslKeyPath: "/k.pem",
        sslCertPath: "/c.pem",
        sslCaPath: "/ca.pem",
        safeMode: "safeNonSelect",
        startupSql: "SET search_path = app;\n",
      },
      null,
      "id-1",
    );
    // "Ask every time" never sends the typed password to be stored.
    expect(input.password).toBeNull();
    expect(input.clearPassword).toBe(false);
    expect(input.profile.sslCaPath).toBe("/ca.pem");
    expect(input.profile.options).toMatchObject({
      passwordMode: "ask",
      sslKeyPath: "/k.pem",
      sslCertPath: "/c.pem",
      safeMode: "safeNonSelect",
      startupSql: "SET search_path = app;\n",
    });
    const back = profileToForm(input.profile);
    expect(back).toMatchObject({
      passwordMode: "ask",
      sslKeyPath: "/k.pem",
      sslCertPath: "/c.pem",
      safeMode: "safeNonSelect",
      startupSql: "SET search_path = app;\n",
    });
    // Blank bootstrap commands are stored as none; disabled SSL drops the client files.
    const plain = formToProfileInput(
      { ...pgForm, startupSql: "  \n", ssl: "disable", sslKeyPath: "/k.pem" },
      null,
      "id-2",
    );
    expect(plain.profile.options.startupSql).toBeNull();
    expect(plain.profile.options.sslKeyPath).toBeNull();
    // "No password" never stores or clears through the form either.
    const none = formToProfileInput(
      { ...pgForm, passwordMode: "none", password: "x", clearPassword: true },
      null,
      "id-3",
    );
    expect(none.password).toBeNull();
    expect(none.clearPassword).toBe(false);
  });

  it("old profiles read as Keychain and Silent Mode; new ones start from the preferences", () => {
    const old = SEED_PROFILES.find((p) => p.id === "p-local-pg")!;
    expect(profileToForm(old)).toMatchObject({
      passwordMode: "keychain",
      safeMode: "silent",
      sslKeyPath: "",
      startupSql: "",
    });
    expect(newProfileForm(15, "alert").safeMode).toBe("alert");
    expect(newProfileForm(15).safeMode).toBe("silent");
  });

  it("an SSL key needs its certificate and the other way round (unless SSL is disabled)", () => {
    const paths = (v: typeof pgForm) => {
      const r = connectionFormSchema.safeParse(v);
      return r.success ? [] : r.error.issues.map((i) => i.path.join("."));
    };
    expect(paths({ ...pgForm, sslKeyPath: "/k.pem" })).toEqual(["sslCertPath"]);
    expect(paths({ ...pgForm, sslCertPath: "/c.pem" })).toEqual(["sslKeyPath"]);
    expect(paths({ ...pgForm, sslKeyPath: "/k.pem", sslCertPath: "/c.pem" })).toEqual([]);
    expect(paths({ ...pgForm, ssl: "disable", sslKeyPath: "/k.pem" })).toEqual([]);
  });

  it("SSH: user, key and port may be left to ~/.ssh/config; local port and keep-alive round-trip", () => {
    const ssh = {
      ...pgForm,
      sshEnabled: true,
      sshHost: "bastion",
      sshUser: "",
      sshKeyPath: "",
      sshPort: "",
      sshLocalPort: "15432",
      sshKeepaliveSecs: "20",
    };
    expect(connectionFormSchema.safeParse(ssh).success).toBe(true);
    const input = formToProfileInput(ssh, null, "id-1");
    expect(input.profile.ssh).toEqual({
      host: "bastion",
      port: 0,
      user: "",
      auth: "key",
      keyPath: null,
      localPort: 15432,
      keepaliveSecs: 20,
      passwordMode: null,
    });
    expect(profileToForm(input.profile)).toMatchObject({
      sshPort: "",
      sshLocalPort: "15432",
      sshKeepaliveSecs: "20",
    });
    expect(connectionFormSchema.safeParse({ ...ssh, sshLocalPort: "70000" }).success).toBe(false);
    expect(connectionFormSchema.safeParse({ ...ssh, sshKeepaliveSecs: "x" }).success).toBe(false);
    expect(connectionFormSchema.safeParse({ ...ssh, sshHost: "" }).success).toBe(false);
  });
});

describe("connectionDetail", () => {
  it("reads host : port : user : database, leaving out what is not set", () => {
    const pg = SEED_PROFILES[0]!;
    expect(connectionDetail(pg)).toBe("127.0.0.1 : 5432 : tabler : tabler_dev");
    expect(connectionDetail({ ...pg, database: null, user: "" })).toBe("127.0.0.1 : 5432");
    const sqlite = SEED_PROFILES.find((p) => p.driver === "sqlite")!;
    expect(connectionDetail(sqlite)).toBe(sqlite.sqlitePath);
  });
});

describe("Command Line (IAM authentication) and MySQL Use socket", () => {
  const pg = SEED_PROFILES.find((p) => p.driver === "postgres")!;
  const my = SEED_PROFILES.find((p) => p.driver === "mysql" && p.ssh === null)!;
  const IAM = "aws rds generate-db-auth-token --hostname db --port 5432 --username app";

  it("round-trips the password command; it is required, and never a stored password", () => {
    const values = { ...profileToForm(pg), passwordMode: "command" as const, password: "typed" };
    const missing = connectionFormSchema.safeParse(values);
    expect(missing.success).toBe(false);
    expect(missing.success ? [] : missing.error.issues.map((i) => i.path.join("."))).toEqual([
      "passwordCommand",
    ]);
    const filled = { ...values, passwordCommand: `  ${IAM}\n` };
    expect(connectionFormSchema.safeParse(filled).success).toBe(true);
    const input = formToProfileInput(filled, pg, pg.id);
    expect(input.profile.options.passwordMode).toBe("command");
    expect(input.profile.options.passwordCommand).toBe(IAM);
    expect(input.password).toBeNull();
    expect(input.clearPassword).toBe(false);
    const back = profileToForm(input.profile);
    expect(back.passwordMode).toBe("command");
    expect(back.passwordCommand).toBe(IAM);
    // Another mode drops the command.
    const keychain = formToProfileInput({ ...filled, passwordMode: "keychain" }, pg, pg.id);
    expect(keychain.profile.options.passwordCommand).toBeNull();
  });

  it("round-trips Use socket: no host needed, path optional, over SSH it is the server's", () => {
    const values = { ...profileToForm(my), useSocket: true, host: "", socketPath: "" };
    expect(connectionFormSchema.safeParse(values).success).toBe(true);
    const input = formToProfileInput({ ...values, socketPath: "/tmp/mysql.sock" }, my, my.id);
    expect(input.profile.options.useSocket).toBe(true);
    expect(input.profile.options.socketPath).toBe("/tmp/mysql.sock");
    const back = profileToForm(input.profile);
    expect(back.useSocket).toBe(true);
    expect(back.socketPath).toBe("/tmp/mysql.sock");
    expect(connectionDetail(input.profile)).toBe(
      ["/tmp/mysql.sock", my.user, my.database].filter(Boolean).join(" : "),
    );
    // Empty path: auto-detected by the backend.
    const detect = formToProfileInput(values, my, my.id);
    expect(detect.profile.options.socketPath).toBeNull();
    expect(connectionDetail(detect.profile).startsWith("socket")).toBe(true);

    // Over SSH (TablePlus b228) the path is the SSH server's: empty (its usual places) or full.
    const over = { ...values, sshEnabled: true, sshHost: "b" };
    expect(connectionFormSchema.safeParse(over).success).toBe(true);
    expect(
      connectionFormSchema.safeParse({ ...over, socketPath: "/run/mysqld/m.sock" }).success,
    ).toBe(true);
    const relative = connectionFormSchema.safeParse({ ...over, socketPath: "~/m.sock" });
    expect(relative.success).toBe(false);
    expect(relative.success ? [] : relative.error.issues.map((i) => i.path.join("."))).toEqual([
      "socketPath",
    ]);
    const tunnelled = formToProfileInput({ ...over, socketPath: "/run/mysqld/m.sock" }, my, my.id);
    expect(tunnelled.profile).toMatchObject({
      ssh: { host: "b" },
      options: { useSocket: true, socketPath: "/run/mysqld/m.sock" },
    });
    // Only MySQL has it: a PostgreSQL form ignores the flag.
    const onPg = formToProfileInput({ ...profileToForm(pg), useSocket: true }, pg, pg.id);
    expect(onPg.profile.options.useSocket).toBeNull();
    expect(profileToForm({ ...pg, options: { ...pg.options, useSocket: true } }).useSocket).toBe(
      false,
    );
    // Off again: TCP, the socket settings are dropped.
    const off = formToProfileInput({ ...values, host: "db", useSocket: false }, my, my.id);
    expect(off.profile.options.useSocket).toBeNull();
    expect(off.profile.options.socketPath).toBeNull();
  });
});

describe("conn3a: SQLCipher, resource file, driver options, encoding, tags", () => {
  const pg = { ...DEFAULT_FORM, name: "pg", user: "u", host: "h" };
  const lite = { ...DEFAULT_FORM, name: "lite", driver: "sqlite" as const, sqlitePath: "/x.db" };

  it("round-trips the SQLite key mode, SQLCipher compatibility and resource file", () => {
    const input = formToProfileInput(
      {
        ...lite,
        password: "k3y",
        sqliteCipherCompatibility: "3",
        sqliteResourceFile: "~/.sqliterc",
      },
      null,
      "id-1",
    );
    // The key is the profile's password (Keychain).
    expect(input.password).toBe("k3y");
    expect(input.profile.options).toMatchObject({
      passwordMode: "keychain",
      sqliteCipherCompatibility: 3,
      sqliteResourceFile: "~/.sqliterc",
      encoding: null,
      sqliteSshUpload: null,
    });
    expect(profileToForm(input.profile)).toMatchObject({
      sqliteCipherCompatibility: "3",
      sqliteResourceFile: "~/.sqliterc",
    });
    // Default compatibility (4) is stored as none; "Ask every time" sends no key.
    const ask = formToProfileInput({ ...lite, passwordMode: "ask", password: "x" }, null, "id-1");
    expect(ask.password).toBeNull();
    expect(ask.profile.options.sqliteCipherCompatibility).toBeNull();
    expect(profileToForm(ask.profile).sqliteCipherCompatibility).toBe("");
    expect(passwordModesFor("sqlite")).toEqual(["keychain", "ask", "none"]);
    expect(passwordModesFor("mysql")).toContain("command");
  });

  it("SQLite over SSH keeps the SSH settings (no local port) and the upload switch", () => {
    const v = {
      ...lite,
      sqlitePath: "/srv/app.sqlite",
      sshEnabled: true,
      sshHost: "box",
      sshLocalPort: "15432",
      sqliteSshUpload: true,
    };
    expect(connectionFormSchema.safeParse(v).success).toBe(true);
    const input = formToProfileInput(v, null, "id-1");
    expect(input.profile.ssh).toMatchObject({ host: "box", localPort: null });
    expect(input.profile.sqlitePath).toBe("/srv/app.sqlite");
    expect(input.profile.options.sqliteSshUpload).toBe(true);
    expect(profileToForm(input.profile)).toMatchObject({ sshEnabled: true, sqliteSshUpload: true });
    // The remote path is required, and so is the SSH host.
    const r = connectionFormSchema.safeParse({ ...v, sqlitePath: "", sshHost: "" });
    const issues = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
    expect(issues).toEqual(
      expect.arrayContaining([
        "sqlitePath: Enter the path of the database file on the SSH server",
        "sshHost: SSH host is required",
      ]),
    );
  });

  it("round-trips the MySQL Ver., Other options and encoding (only for MySQL)", () => {
    const my = {
      ...pg,
      driver: "mysql" as const,
      mysqlVersion: "mariadb" as const,
      mysqlCleartextPlugin: true,
      mysqlLocalInfile: true,
      encoding: "latin1",
      pgParams: "jit=off",
    };
    const input = formToProfileInput(my, null, "id-1");
    expect(input.profile.options).toMatchObject({
      mysqlVersion: "mariadb",
      mysqlCleartextPlugin: true,
      mysqlLocalInfile: true,
      encoding: "latin1",
      pgParams: null,
      pgSslNegotiation: null,
    });
    expect(profileToForm(input.profile)).toMatchObject({
      mysqlVersion: "mariadb",
      mysqlCleartextPlugin: true,
      mysqlLocalInfile: true,
      encoding: "latin1",
    });
    const plain = formToProfileInput({ ...my, mysqlVersion: "default" }, null, "id-1");
    expect(plain.profile.options.mysqlVersion).toBeNull();
    const bad = connectionFormSchema.safeParse({ ...my, encoding: "latin1; DROP" });
    expect(bad.success).toBe(false);
  });

  it("SingleStore DB round-trips through the form and a link's driverVersion index 3", () => {
    // Append only: a link's `driverVersion=` is an index into this list.
    expect([...MYSQL_VERSIONS]).toEqual(["default", "mariadb", "old", "singlestore"]);
    expect(MYSQL_VERSION_LABEL.singlestore).toBe("SingleStore DB");
    expect(MYSQL_VERSION_LABEL.old).toBe("Old MySQL (5.x)");

    const my = { ...pg, driver: "mysql" as const, mysqlVersion: "singlestore" as const };
    expect(connectionFormSchema.safeParse(my).success).toBe(true);
    const input = formToProfileInput(my, null, "id-1");
    expect(input.profile.options.mysqlVersion).toBe("singlestore");
    expect(profileToForm(input.profile).mysqlVersion).toBe("singlestore");
    // Other drivers keep none.
    const other = formToProfileInput({ ...pg, mysqlVersion: "singlestore" }, null, "id-1");
    expect(other.profile.options.mysqlVersion).toBeNull();
    // Every version survives the profile -> form -> profile trip.
    for (const version of MYSQL_VERSIONS) {
      const trip = formToProfileInput(
        profileToForm(formToProfileInput({ ...my, mysqlVersion: version }, null, "id-2").profile),
        null,
        "id-2",
      );
      expect(trip.profile.options.mysqlVersion).toBe(version === "default" ? null : version);
    }
    // The link's index 3 is SingleStore DB; 4 is out of range.
    expect(mysqlVersionOf(3)).toBe("singlestore");
    expect(mysqlVersionOf(2)).toBe("old");
    expect(mysqlVersionOf(4)).toBeNull();
  });

  it("the form's connect timeout default is the backend's 60 s", () => {
    expect(DEFAULT_CONNECT_TIMEOUT_SECS).toBe(60);
    expect(newProfileForm(DEFAULT_PREFS.connectTimeout).connectTimeoutSecs).toBe("60");
  });

  it("round-trips the PostgreSQL SSL negotiation, channel binding and extra parameters", () => {
    const v = {
      ...pg,
      ssl: "require" as const,
      pgSslNegotiation: "direct" as const,
      pgDisableChannelBinding: true,
      pgParams: "  statement_timeout=5000 jit=off  ",
      mysqlLocalInfile: true,
    };
    expect(connectionFormSchema.safeParse(v).success).toBe(true);
    const input = formToProfileInput(v, null, "id-1");
    expect(input.profile.options).toMatchObject({
      pgSslNegotiation: "direct",
      pgDisableChannelBinding: true,
      pgParams: "statement_timeout=5000 jit=off",
      mysqlLocalInfile: null,
    });
    expect(profileToForm(input.profile)).toMatchObject({
      pgSslNegotiation: "direct",
      pgDisableChannelBinding: true,
    });
    // DIRECT needs require or stricter, as libpq says.
    const weak = connectionFormSchema.safeParse({ ...v, ssl: "prefer" });
    const paths = weak.success ? [] : weak.error.issues.map((i) => i.path.join("."));
    expect(paths).toContain("pgSslNegotiation");
  });

  it("round-trips the bootstrap script and the SSH password storage", () => {
    const v = {
      ...pg,
      bootstrapScript: "cloud-sql-proxy &",
      sshEnabled: true,
      sshHost: "bastion",
      sshAuth: "password" as const,
      sshPasswordMode: "ask" as const,
      sshPassword: "typed",
    };
    const input = formToProfileInput(v, null, "id-1");
    expect(input.profile.options.bootstrapScript).toBe("cloud-sql-proxy &");
    expect(input.profile.ssh?.passwordMode).toBe("ask");
    // Asked every time: never stored.
    expect(input.sshPassword).toBeNull();
    expect(profileToForm(input.profile)).toMatchObject({
      bootstrapScript: "cloud-sql-proxy &",
      sshPasswordMode: "ask",
    });
    const kept = formToProfileInput({ ...v, sshPasswordMode: "keychain" }, null, "id-1");
    expect(kept.sshPassword).toBe("typed");
    expect(
      formToProfileInput({ ...v, bootstrapScript: "  " }, null, "id-1").profile.options,
    ).toMatchObject({ bootstrapScript: null });
  });

  it("takes custom tags (any name up to 40 characters)", () => {
    const input = formToProfileInput({ ...pg, tag: "QA" }, null, "id-1");
    expect(input.profile.options.tag).toBe("QA");
    expect(profileToForm(input.profile).tag).toBe("QA");
    expect(connectionFormSchema.safeParse({ ...pg, tag: "x".repeat(41) }).success).toBe(false);
  });
});
