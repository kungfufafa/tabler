import { describe, expect, it } from "vitest";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { profile } from "@/lib/ipc/mock/fixtures";
import type { ConnectionProfile } from "@/lib/ipc";
import { mongoUi } from "./form";
import { formParams, mongoFormOf } from "./params";
import { formatMongoUrl, parseMongoConnectionUrl, parseMongoUrl } from "./url";

// MongoDB's form: its values as a profile and back, what `validate` says before the backend's
// `MongoParams::check` would, and connection strings read and written back.

const values = (v: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  driver: "mongodb",
  ...mongoUi.defaults!(),
  ...v,
});

const params = (p: Record<string, unknown>) => formParams({ ...mongoFormOf(null), ...p });

const base = (v: ConnectionFormValues): ConnectionProfile =>
  profile({
    id: "p",
    name: "Mongo",
    driver: "mongodb",
    host: v.host,
    port: v.port === "" ? null : Number(v.port),
    user: v.user || null,
    database: v.database || null,
    ssl: v.ssl,
    sslCaPath: v.sslCaPath || null,
  });

const toProfile = (v: ConnectionFormValues) => mongoUi.toProfile!(v, base(v));

const issues = (v: Partial<ConnectionFormValues>) =>
  mongoUi.validate!(values(v)).map((i) => `${i.path}: ${i.message}`);

describe("the form", () => {
  it("starts on this machine's MongoDB without TLS", () => {
    expect(mongoUi.defaults!()).toMatchObject({ host: "127.0.0.1", port: "27017", ssl: "disable" });
    expect(mongoUi.rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
    expect(mongoUi.sslModes?.[0]).toEqual(["disable", "Off"]);
  });

  it("saves the params that are set, and reads them back", () => {
    const v = values({
      driverParams: params({
        authMechanism: "scramSha256",
        authSource: "users",
        replicaSet: "rs0",
        directConnection: "off",
        readPreference: "secondaryPreferred",
        appName: "ops",
        serverSelectionTimeoutSecs: "5",
        sampleSize: "500",
      }),
    });
    const p = toProfile(v);
    expect(p.options.driverParams).toEqual({
      authMechanism: "scramSha256",
      authSource: "users",
      replicaSet: "rs0",
      directConnection: false,
      readPreference: "secondaryPreferred",
      appName: "ops",
      serverSelectionTimeoutSecs: 5,
      sampleSize: 500,
    });
    expect(mongoUi.fromProfile!(p)).toEqual({ driverParams: v.driverParams });
    // Unset ones are left out: the backend's defaults apply.
    expect(toProfile(values()).options.driverParams).toEqual({
      authMechanism: "default",
      readPreference: "primary",
    });
  });

  it("says what the backend would refuse", () => {
    expect(issues({})).toEqual([]);
    expect(issues({ driverParams: params({ sampleSize: "0" }) })).toEqual([
      "driverParams.sampleSize: Between 1 and 10000",
    ]);
    expect(issues({ driverParams: params({ serverSelectionTimeoutSecs: "x" }) })).toEqual([
      "driverParams.serverSelectionTimeoutSecs: At least 1 second",
    ]);
    expect(issues({ driverParams: params({ uri: "postgres://h/db" }) })).toEqual([
      "driverParams.uri: Use a mongodb:// or mongodb+srv:// URL with a host, and write a `/`, `?` or `@` in the user name, password, database or options as %2F, %3F, %40",
    ]);
    // A user part with a raw `@` (an LDAP or e-mail login) and a raw `/` or `?` in the password:
    // refused, so no piece of the password is saved in the URL.
    for (const uri of [
      "mongodb://me@corp.com:s3cr/et@db.example.com:27017/app",
      "mongodb://ldap@CORP:pa?ss@db.example.com/app",
    ])
      expect(issues({ driverParams: params({ uri }) })).toHaveLength(1);
    expect(issues({ driverParams: params({ uri: "mongodb://u:secret@h/db" }) })).toEqual([
      "driverParams.uri: Put the password in Password, not in the URL (it is saved in plain text)",
    ]);
    expect(
      issues({ driverParams: params({ uri: "mongodb://h/?tlsCertificateKeyFilePassword=x" }) }),
    ).toEqual([
      "driverParams.uri: Give the client key's password as the key passphrase, not in the URL",
    ]);
    expect(
      issues({ driverParams: params({ uri: "mongodb://h/?authMechanism=MONGODB-AWS" }) }),
    ).toEqual([
      "driverParams.uri: MONGODB-AWS sign-in is not built into Tabler: use SCRAM, X.509, PLAIN or OIDC",
    ]);
    expect(issues({ driverParams: params({ uri: "mongodb+srv://c.example.net/" }) })).toEqual([
      "ssl: The URL asks for TLS: turn SSL on, or set tls=false in the URL",
    ]);
    expect(issues({ driverParams: params({ authMechanism: "x509" }) })).toEqual([
      "ssl: X.509 sign-in needs TLS: turn SSL on",
    ]);
    expect(issues({ ssl: "require", driverParams: params({ authMechanism: "x509" }) })).toEqual([
      "sslCertPath: X.509 sign-in needs a client certificate",
    ]);
    expect(issues({ driverParams: params({ authMechanism: "plain" }) })).toEqual([
      "user: PLAIN (LDAP) sign-in needs a user",
    ]);
    expect(issues({ ssl: "require", sslCertPath: "/a.pem", sslKeyPath: "/b.pem" })).toEqual([
      "sslKeyPath: MongoDB reads the certificate and its key from one PEM file: choose it for both",
    ]);
  });

  it("over SSH, wants one node, the Host and Port fields', and no verified certificate", () => {
    const ssh = { sshEnabled: true, sshHost: "bastion", host: "db.internal", port: "27017" };
    expect(
      issues({ ...ssh, driverParams: params({ uri: "mongodb://db.internal:27017/" }) }),
    ).toEqual([]);
    expect(issues({ ...ssh, driverParams: params({ uri: "mongodb://a:27017,b:27017/" }) })).toEqual(
      [
        "driverParams.uri: Over SSH, give one host (mongodb://host:port), not SRV or a list of hosts",
      ],
    );
    expect(issues({ ...ssh, driverParams: params({ uri: "mongodb://other:27017/" }) })).toEqual([
      "driverParams.uri: Over SSH, the URL's host and port must be the Host and Port fields'",
    ]);
    expect(issues({ ...ssh, ssl: "verify-full" })).toEqual([
      "ssl: Over SSH the certificate cannot be verified: use SSL “On”",
    ]);
  });

  it("sums a profile up", () => {
    const p = toProfile(values({ host: "db", database: "shop" }));
    expect(mongoUi.summary!(p)).toBe("db : 27017 / shop");
    const srv = toProfile(
      values({ driverParams: params({ uri: "mongodb+srv://c.example.net/" }) }),
    );
    expect(mongoUi.summary!(srv)).toBe("srv: c.example.net");
  });
});

describe("connection strings", () => {
  it("read as the backend's UriParts reads them", () => {
    expect(parseMongoUrl("mongodb://u%40x:p%3Aw@h1:27018,h2/my%20db?replicaSet=rs0&w=1")).toEqual({
      srv: false,
      hosts: ["h1:27018", "h2"],
      user: "u@x",
      password: "p:w",
      database: "my db",
      options: [
        ["replicaSet", "rs0"],
        ["w", "1"],
      ],
    });
    expect(parseMongoUrl("mongodb+srv://a:1/")).toBeNull();
    expect(parseMongoUrl("mongodb+srv://a,b/")).toBeNull();
    expect(parseMongoUrl("mongodb:///db")).toBeNull();
    expect(parseMongoUrl("http://h")).toBeNull();
    // A raw `/` in the password: refused, not read as host `u`, port 12345, database `x@h`.
    expect(parseMongoUrl("mongodb://u:12345/x@h")).toBeNull();
    expect(parseMongoConnectionUrl("mongodb://u:12345/x@h")).toBeNull();
    // A raw `?` in the password: refused, not read as host `u`, port 12 and an option `x@h`.
    expect(parseMongoUrl("mongodb://u:12?x@h")).toBeNull();
    expect(parseMongoConnectionUrl("mongodb://u:12?x@h")).toBeNull();
    expect(parseMongoUrl("mongodb://u:1234?5678@h/db")).toBeNull();
    expect(parseMongoConnectionUrl("mongodb://u:1234?5678@h/db")).toBeNull();
    expect(parseMongoUrl("mongodb://u:12%3Fx@h")?.password).toBe("12?x");
    expect(parseMongoUrl("mongodb://u:p@h/x%40y")?.database).toBe("x@y");
    // The same with a user part (a raw `@` in the user name): every raw `@` after the authority
    // is refused, so the password's pieces never reach the host, port, database or options.
    for (const u of [
      "mongodb://me@corp.com:s3cr/et@db.example.com:27017/app",
      "mongodb://me@corp.com:1234/5678@db.example.com/app",
      "mongodb://ldap@CORP:pa?ss@db.example.com/app",
      "mongodb://u:p@ss/word@h/db",
      "mongodb://h/?appName=a@b",
    ]) {
      expect(parseMongoUrl(u), u).toBeNull();
      expect(parseMongoConnectionUrl(u), u).toBeNull();
    }
    expect(parseMongoUrl("mongodb://me%40corp.com:s3cr%2Fet@db.example.com/app")).toMatchObject({
      user: "me@corp.com",
      password: "s3cr/et",
      hosts: ["db.example.com"],
      database: "app",
    });
    expect(parseMongoUrl("mongodb://h/?appName=a%40b")?.options).toEqual([["appName", "a@b"]]);
  });

  it("spread over the form when it holds them, the password out of the URL", () => {
    const read = parseMongoConnectionUrl(
      "mongodb://ada:s3cret@db.example.com:27018/shop?authSource=admin&tls=true&appName=x&serverSelectionTimeoutMS=5000&name=Prod",
    );
    expect(read?.password).toBe("s3cret");
    expect(read?.values).toMatchObject({
      host: "db.example.com",
      port: "27018",
      user: "ada",
      database: "shop",
      ssl: "verify-full",
      driverParams: { uri: "", authSource: "admin", appName: "x", serverSelectionTimeoutSecs: "5" },
    });
  });

  it("kept whole when the form cannot hold them, without the password and Tabler's keys", () => {
    const read = parseMongoConnectionUrl(
      "mongodb+srv://ada:pw@cluster0.example.net/shop?retryWrites=true&w=majority&statusColor=red",
    );
    expect(read?.password).toBe("pw");
    expect(read?.values).toMatchObject({
      host: "cluster0.example.net",
      user: "ada",
      database: "shop",
      ssl: "verify-full",
      driverParams: { uri: "mongodb+srv://cluster0.example.net/shop?retryWrites=true&w=majority" },
    });
  });

  it.each([
    "mongodb://db.example.com:27017/shop",
    "mongodb://ada@db.example.com:27018/shop?authSource=admin&authMechanism=SCRAM-SHA-256&replicaSet=rs0&readPreference=secondary&directConnection=false&appName=ops&serverSelectionTimeoutMS=7000&connectTimeoutMS=3000&tls=true&tlsAllowInvalidCertificates=true&tlsCAFile=%2Fca.pem&tlsCertificateKeyFile=%2Fme.pem",
    "mongodb://[::1]:27017/",
    "mongodb+srv://ada@cluster0.example.net/shop?retryWrites=true&w=majority",
    "mongodb://a:27017,b:27017/shop?replicaSet=rs0&tls=true",
    "mongodb://h:27017/?compressors=zlib&tls=false",
  ])("round-trip through a profile: %s", (url) => {
    const read = parseMongoConnectionUrl(url)!;
    const v = values({ ...read.values });
    const p = toProfile({ ...v, connectTimeoutSecs: read.values.connectTimeoutSecs ?? "" });
    const withPaths: ConnectionProfile = {
      ...p,
      options: {
        ...p.options,
        connectTimeoutSecs: v.connectTimeoutSecs ? Number(v.connectTimeoutSecs) : null,
        sslCertPath: v.sslCertPath || null,
        sslKeyPath: v.sslKeyPath || null,
      },
    };
    const again = parseMongoConnectionUrl(formatMongoUrl(withPaths)!)!;
    expect(again.values).toEqual(read.values);
    expect(again.password).toBeNull();
  });

  it("write the form's SSL choice into a kept URL that does not say it", () => {
    const p = toProfile(
      values({
        ssl: "require",
        driverParams: params({ uri: "mongodb://a:1,b:2/db?replicaSet=rs" }),
      }),
    );
    expect(formatMongoUrl(p)).toBe(
      "mongodb://a:1,b:2/db?replicaSet=rs&tls=true&tlsAllowInvalidCertificates=true",
    );
  });
});
