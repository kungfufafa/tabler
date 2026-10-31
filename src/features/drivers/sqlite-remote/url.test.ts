import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { profileToForm } from "@/features/connections/schema";
import { loadUrlSchemes, parseConnectionUrl, profileToUrl } from "@/features/connections/url";
import { CORE_KINDS } from "@/features/drivers/catalog";
import type { ConnectionProfile } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { useAppInfoStore } from "@/stores/appInfo";
import { d1Ui } from "./d1";
import { libsqlUi } from "./libsql";
import {
  formatD1Url,
  formatLibsqlUrl,
  libsqlBase,
  parseD1ConnectionUrl,
  parseLibsqlConnectionUrl,
  parseLibsqlUrl,
} from "./url";

// The lane's URLs: LibSQL's read as the backend reads them (`libsql/url.rs`), D1's
// `d1://<account>/<database>`, each written back without its secret and read again unchanged.

const ACCOUNT = "0123456789abcdef0123456789abcdef";

const libsql = (p: Partial<ConnectionProfile> = {}): ConnectionProfile =>
  profile({
    id: "p-ls",
    name: "Edge",
    driver: "libsql",
    host: "db-org.turso.io",
    port: null,
    user: null,
    database: null,
    ssl: "verify-full",
    ...p,
  });

const d1 = (params: Record<string, unknown>, database = "shop"): ConnectionProfile => {
  const p = profile({ id: "p-d1", name: "D1", driver: "d1", host: null, port: null, user: null });
  return { ...p, database, options: { ...p.options, driverParams: params } };
};

describe("parseLibsqlUrl", () => {
  const base = (s: string) => {
    const u = parseLibsqlUrl(s);
    return u ? libsqlBase(u) : null;
  };

  it("maps every scheme to HTTP or HTTPS as the backend does", () => {
    expect(base("libsql://db-org.aws-eu-west-1.turso.io")).toBe(
      "https://db-org.aws-eu-west-1.turso.io",
    );
    expect(base("wss://db.turso.io/")).toBe("https://db.turso.io");
    expect(base("ws://127.0.0.1:8080")).toBe("http://127.0.0.1:8080");
    expect(base("HTTP://localhost:8080/")).toBe("http://localhost:8080");
    expect(base("db.turso.io")).toBe("https://db.turso.io");
    expect(base("libsql://127.0.0.1:8080?tls=0")).toBe("http://127.0.0.1:8080");
    expect(base("https://h/ns/db/")).toBe("https://h/ns/db");
    expect(base("http://[::1]:8080")).toBe("http://[::1]:8080");
    expect(base("http://h:443")).toBe("http://h:443");
    expect(base("https://h:443")).toBe("https://h");
    expect(base("libsql://h:80?tls=0")).toBe("http://h");
    // The key in any casing, as the backend reads it; the value exactly.
    expect(base("libsql://h/db?TLS=0")).toBe("http://h/db");
    expect(base("libsql://h/db?Tls=false")).toBe("http://h/db");
    expect(base("libsql://h/db?TLS=FALSE")).toBe("https://h/db");
    // Only libsql:// reads tls.
    expect(base("https://h?tls=0")).toBe("https://h");
    expect(base("https://h?TLS=0")).toBe("https://h");
    expect(parseLibsqlUrl("postgres://h")).toBeNull();
    expect(parseLibsqlUrl("")).toBeNull();
    expect(parseLibsqlUrl("libsql://")).toBeNull();
  });

  it("takes the token and the credentials out", () => {
    expect(parseLibsqlUrl("libsql://h?authToken=abc")).toMatchObject({ token: "abc", basic: null });
    expect(parseLibsqlUrl("libsql://h?auth_token=")).toMatchObject({
      token: null,
      hasSecret: true,
    });
    expect(parseLibsqlUrl("https://h/db")).toMatchObject({ hasSecret: false });
    expect(parseLibsqlUrl("https://@h/db")).toMatchObject({ basic: null, hasSecret: false });
    // A password without a user is a secret too.
    expect(parseLibsqlUrl("https://:pw@h/db")).toMatchObject({
      basic: { user: "", password: "pw" },
      hasSecret: true,
    });
    // A later empty key does not erase the token; any casing is the token.
    expect(parseLibsqlUrl("https://h/db?authToken=SECRET&authToken=")).toMatchObject({
      token: "SECRET",
      hasSecret: true,
    });
    expect(parseLibsqlUrl("https://h?AuthToken=S")).toMatchObject({ token: "S", hasSecret: true });
    expect(parseLibsqlUrl("http://u%40x:p%3Aw@h:8080")).toMatchObject({
      host: "h",
      port: 8080,
      basic: { user: "u@x", password: "p:w" },
    });
  });
});

describe("a LibSQL connection URL", () => {
  it("fills host, port and SSL, moving the token into the secret", () => {
    const parsed = parseLibsqlConnectionUrl("libsql://db-org.turso.io?authToken=eyJ.x.y");
    expect(parsed).toEqual({
      values: {
        host: "db-org.turso.io",
        port: "",
        ssl: "verify-full",
        user: "",
        driverParams: { url: "", basicAuth: false },
      },
      password: "eyJ.x.y",
    });
    expect(parseLibsqlConnectionUrl("libsql://127.0.0.1:8080?tls=0")?.values).toMatchObject({
      host: "127.0.0.1",
      port: "8080",
      ssl: "disable",
    });
    expect(parseLibsqlConnectionUrl("libsql://h/db?TLS=0")?.values).toMatchObject({
      ssl: "disable",
      driverParams: { url: "http://h/db" },
    });
    // Copy as URL keeps a plain HTTP server plain.
    const upper = libsql({
      options: { ...libsql().options, driverParams: { url: "libsql://h/db?TLS=0" } },
    });
    expect(formatLibsqlUrl(upper)).toBe("libsql://h:80/db?tls=0");
    // Without a port a plain URL means port 80, which the form's empty port does not.
    expect(parseLibsqlConnectionUrl("ws://sqld.lan")?.values.port).toBe("80");
  });

  it("keeps a server under a path as the url param, and Basic credentials as user and password", () => {
    const parsed = parseLibsqlConnectionUrl("https://me:pw@gw.example.com:8443/ns/db/");
    expect(parsed?.values).toMatchObject({
      host: "gw.example.com",
      port: "8443",
      user: "me",
      driverParams: { url: "https://gw.example.com:8443/ns/db", basicAuth: true },
    });
    expect(parsed?.password).toBe("pw");
  });

  it("is written without the secret and read back to the same form", () => {
    const cases: [ConnectionProfile, string][] = [
      [libsql(), "libsql://db-org.turso.io"],
      [libsql({ port: 443 }), "libsql://db-org.turso.io"],
      [libsql({ host: "127.0.0.1", port: 8080, ssl: "disable" }), "libsql://127.0.0.1:8080?tls=0"],
      [libsql({ host: "::1", port: 9000, ssl: "disable" }), "libsql://[::1]:9000?tls=0"],
      [
        libsql({
          user: "me",
          options: { ...libsql().options, driverParams: { basicAuth: true } },
        }),
        "libsql://me@db-org.turso.io",
      ],
      [
        libsql({
          options: { ...libsql().options, driverParams: { url: "https://gw.example.com/ns/db" } },
        }),
        "libsql://gw.example.com/ns/db",
      ],
      [
        libsql({
          options: { ...libsql().options, driverParams: { url: "http://10.0.0.5:8080/db" } },
        }),
        "libsql://10.0.0.5:8080/db?tls=0",
      ],
    ];
    for (const [p, url] of cases) {
      expect(formatLibsqlUrl(p)).toBe(url);
      const back = parseLibsqlConnectionUrl(url)!;
      const form = profileToForm(p, libsqlUi);
      const params = form.driverParams as { url: string; basicAuth: boolean };
      expect(back.values.driverParams).toEqual(params);
      expect(back.values.user).toBe(params.basicAuth ? form.user : "");
      expect(back.password).toBeNull();
      // With a URL its scheme says HTTP or HTTPS, and the SSL mode only how the certificate is checked.
      if (params.url === "") {
        expect(back.values.ssl).toBe(form.ssl);
        expect(back.values.host).toBe(form.host);
        expect([form.port, form.port === "443" ? "" : form.port]).toContain(back.values.port);
      }
    }
    // The profile's empty port is 8080 without TLS: written out.
    expect(formatLibsqlUrl(libsql({ host: "h", ssl: "disable" }))).toBe("libsql://h:8080?tls=0");
    expect(formatLibsqlUrl(libsql({ host: null }))).toBeNull();
  });
});

describe("a D1 connection URL", () => {
  it("names the account and the database, a token as the secret", () => {
    expect(parseD1ConnectionUrl(`d1://${ACCOUNT}/shop`)).toEqual({
      values: {
        host: "",
        port: "",
        user: "",
        database: "shop",
        driverParams: { accountId: ACCOUNT, apiBase: "" },
        name: "shop",
      },
      password: null,
    });
    expect(parseD1ConnectionUrl(`cloudflared1://:tok@${ACCOUNT}/my%20db`)).toMatchObject({
      values: { database: "my db" },
      password: "tok",
    });
    expect(
      parseD1ConnectionUrl(
        `d1://${ACCOUNT}/shop?apiToken=t2&apiBase=http%3A%2F%2F127.0.0.1%3A8787`,
      ),
    ).toMatchObject({
      values: { driverParams: { accountId: ACCOUNT, apiBase: "http://127.0.0.1:8787" } },
      password: "t2",
    });
    expect(parseD1ConnectionUrl("d1://")).toBeNull();
    expect(parseD1ConnectionUrl("libsql://h")).toBeNull();
  });

  it("is written without the secret and read back to the same form", () => {
    for (const p of [
      d1({ accountId: ACCOUNT }),
      d1({ accountId: ACCOUNT }, "5a0c1f9e-2b7d-4c51-9a3e-0d6f2e8b7c41"),
      d1({ accountId: ACCOUNT, apiBase: "https://gw.example.com/client/v4" }, "a/b c"),
    ]) {
      const url = formatD1Url(p)!;
      const back = parseD1ConnectionUrl(url)!;
      const form = profileToForm(p, d1Ui);
      expect(back.values.database).toBe(form.database);
      expect(back.values.driverParams).toEqual(form.driverParams);
      expect(back.password).toBeNull();
    }
    expect(formatD1Url(d1({ accountId: ACCOUNT }))).toBe(`d1://${ACCOUNT}/shop`);
    expect(formatD1Url(d1({}))).toBeNull();
    // An API base stored before it was checked leaves its credentials out of the link.
    for (const [apiBase, kept] of [
      ["https://u:SECRET@gw.example/x", "https://gw.example/x"],
      ["https://gw.example/?key=SECRET", "https://gw.example"],
      ["https://gw.example/v4#SECRET", "https://gw.example/v4"],
      ["https://u:SECRET@h:99999/x", ""],
    ] as const) {
      const link = formatD1Url(d1({ accountId: ACCOUNT, apiBase }))!;
      expect(link).not.toContain("SECRET");
      expect(parseD1ConnectionUrl(link)!.values.driverParams).toEqual({
        accountId: ACCOUNT,
        apiBase: kept,
      });
    }
  });
});

describe("the lane's URLs in the app", () => {
  beforeEach(() =>
    useAppInfoStore.setState({
      info: {
        version: "1",
        platform: "test",
        debug: true,
        kinds: [...CORE_KINDS, "libsql", "d1"],
      },
    }),
  );
  afterEach(() => useAppInfoStore.setState({ info: null }));

  it("are read and written through the lane module, over SSH as a +ssh URL", async () => {
    await loadUrlSchemes();
    const parsed = parseConnectionUrl("libsql://db-org.turso.io?authToken=abc&name=Edge");
    expect(parsed.values).toMatchObject({
      driver: "libsql",
      host: "db-org.turso.io",
      name: "Edge",
    });
    expect(parsed.password).toBe("abc");
    const ssh = parseConnectionUrl("libsql+ssh://me@bastion/10.0.0.5:8080?tls=0");
    expect(ssh.values).toMatchObject({
      driver: "libsql",
      host: "10.0.0.5",
      port: "8080",
      ssl: "disable",
      sshEnabled: true,
      sshHost: "bastion",
    });
    expect(parseConnectionUrl(`cloudflared1://${ACCOUNT}/shop`).values).toMatchObject({
      driver: "d1",
      database: "shop",
      driverParams: { accountId: ACCOUNT, apiBase: "" },
    });
    expect(profileToUrl(d1({ accountId: ACCOUNT }), "secret")).toBe(`d1://${ACCOUNT}/shop`);
    expect(profileToUrl(libsql(), "secret")).toBe("libsql://db-org.turso.io");
    expect(() => parseConnectionUrl("d1+ssh://me@bastion/x/y")).toThrow(/cannot go over SSH/);
  });
});
