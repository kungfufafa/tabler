import { describe, expect, it } from "vitest";
import {
  connectionDetail,
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import lane from ".";
import { API_BASE_SECRET } from "./d1";

// The two connection forms: what they save (only the params each backend reads, the common
// fields it does not use cleared), what they read back, and their checks, next to the schema's.

const libsqlUi = lane.kinds!.libsql!;
const d1Ui = lane.kinds!.d1!;
const ACCOUNT = "0123456789abcdef0123456789abcdef";

const form = (values: Partial<ConnectionFormValues>): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  name: "n",
  ...values,
});

/** The schema's issues and the lane's, as `path: message`. */
function issues(values: ConnectionFormValues, ui = libsqlUi): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui.validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

describe("the LibSQL form", () => {
  const values = form({
    driver: "libsql",
    ...libsqlUi.defaults?.(),
    host: "db-org.turso.io",
    password: "tok",
    driverParams: { url: "", basicAuth: false },
  });

  it("starts on HTTPS that checks the certificate, with the port of the SSL mode", () => {
    expect(libsqlUi.defaults?.()).toEqual({ host: "", port: "", ssl: "verify-full" });
    expect(libsqlUi.rows).toEqual(["hostPort", "secret", "ssl"]);
  });

  it("saves host, port and SSL with no params, and reads them back", () => {
    const input = formToProfileInput(values, null, "id-1", libsqlUi);
    expect(input.profile).toMatchObject({
      driver: "libsql",
      host: "db-org.turso.io",
      port: null,
      user: null,
      database: null,
      ssl: "verify-full",
    });
    expect(input.profile.options.driverParams).toBeNull();
    expect(input.password).toBe("tok");
    const back = profileToForm(input.profile, libsqlUi);
    expect(back).toMatchObject({
      host: "db-org.turso.io",
      ssl: "verify-full",
      driverParams: { url: "", basicAuth: false },
    });
    expect(connectionDetail(input.profile, libsqlUi)).toBe("db-org.turso.io");
  });

  it("saves a URL and Basic authentication as params, the user only with Basic", () => {
    const withUrl = form({
      ...values,
      user: "me",
      driverParams: { url: " https://gw.example.com/ns/db ", basicAuth: true },
    });
    const p = formToProfileInput(withUrl, null, "id-1", libsqlUi).profile;
    expect(p.user).toBe("me");
    expect(p.options.driverParams).toEqual({
      url: "https://gw.example.com/ns/db",
      basicAuth: true,
    });
    expect(profileToForm(p, libsqlUi).driverParams).toEqual({
      url: "https://gw.example.com/ns/db",
      basicAuth: true,
    });
    expect(connectionDetail(p, libsqlUi)).toBe("https://gw.example.com/ns/db");
    const noBasic = formToProfileInput(
      { ...withUrl, driverParams: { url: "", basicAuth: false } },
      null,
      "id-1",
      libsqlUi,
    ).profile;
    expect(noBasic.user).toBeNull();
    expect(connectionDetail({ ...noBasic, port: 8443 }, libsqlUi)).toBe("db-org.turso.io : 8443");
  });

  it("checks what the backend's validate checks, in the field it concerns", () => {
    expect(issues(values)).toEqual([]);
    expect(issues({ ...values, host: "" })).toEqual(["host: Enter the host, or the server's URL"]);
    expect(issues({ ...values, host: "libsql://db.turso.io" })).toEqual([
      "host: “libsql://db.turso.io” is not a host name: paste a URL in URL",
    ]);
    const url = (u: string, more: Partial<ConnectionFormValues> = {}) =>
      issues({ ...values, host: "", ...more, driverParams: { url: u, basicAuth: false } });
    expect(url("https://gw.example.com/db")).toEqual([]);
    expect(url("postgres://h")).toEqual([
      "driverParams.url: Use a libsql://, https://, http://, wss:// or ws:// URL",
    ]);
    expect(url("libsql://h?authToken=x")).toEqual([
      "driverParams.url: Put the token in Auth Token (and the user in User), not in the URL",
    ]);
    expect(url("https://u:p@h")).toHaveLength(1);
    // A password without a user, a token key repeated or empty, a token key in another casing.
    for (const secret of [
      "https://:pw@h/db",
      "https://h/db?authToken=SECRET&authToken=",
      "https://h/db?authToken=",
      "https://h/db?AuthToken=S",
    ])
      expect(url(secret)).toEqual([
        "driverParams.url: Put the token in Auth Token (and the user in User), not in the URL",
      ]);
    // The list never shows one, in a profile stored before the check.
    const legacy = formToProfileInput(
      { ...values, driverParams: { url: "https://:pw@h/db?authToken=S", basicAuth: false } },
      null,
      "id-1",
      libsqlUi,
    ).profile;
    expect(libsqlUi.summary?.(legacy)).toBe("https://h/db");
    // Nor in one that does not parse at all.
    const stored = (url: string) =>
      libsqlUi.summary?.({ ...legacy, options: { ...legacy.options, driverParams: { url } } });
    expect(stored("https://u:p@h:99999/db")).toBe("Invalid URL");
    expect(stored("ftp://u:p@h/db")).toBe("Invalid URL");
    expect(stored("ftp://h/db?authToken=S")).toBe("Invalid URL");
    expect(stored("ftp://h/db")).toBe("ftp://h/db");
    // Nor a token the checks do not read as one: the list shows scheme, host, port and path.
    expect(stored("https://h/db#authToken=SECRET")).toBe("https://h/db");
    expect(stored("libsql://h/db?a=1;authToken=SECRET")).toBe("https://h/db");
    expect(stored("http://h:8080/db?x=1")).toBe("http://h:8080/db");
    expect(url("https://h/db", { sshEnabled: true, sshHost: "bastion" })).toEqual([
      "driverParams.url: Over SSH, enter the host and port instead of a URL",
    ]);
    expect(issues({ ...values, driverParams: { url: "", basicAuth: true } })).toEqual([
      "user: Basic authentication needs a user",
    ]);
  });
});

describe("the D1 form", () => {
  const values = form({
    driver: "d1",
    host: "ignored",
    port: "5432",
    user: "ignored",
    database: "shop",
    password: "tok",
    driverParams: { accountId: ACCOUNT, apiBase: "" },
  });

  it("saves the account and the database only, and reads them back", () => {
    const p = formToProfileInput(values, null, "id-1", d1Ui).profile;
    expect(p).toMatchObject({ driver: "d1", host: null, port: null, user: null, database: "shop" });
    expect(p.ssh).toBeNull();
    expect(p.options.driverParams).toEqual({ accountId: ACCOUNT });
    const withBase = formToProfileInput(
      { ...values, driverParams: { accountId: ACCOUNT, apiBase: "http://127.0.0.1:8787" } },
      null,
      "id-1",
      d1Ui,
    ).profile;
    expect(withBase.options.driverParams).toEqual({
      accountId: ACCOUNT,
      apiBase: "http://127.0.0.1:8787",
    });
    expect(profileToForm(withBase, d1Ui)).toMatchObject({
      database: "shop",
      driverParams: { accountId: ACCOUNT, apiBase: "http://127.0.0.1:8787" },
    });
    expect(connectionDetail(p, d1Ui)).toBe("shop · account 01234567…");
  });

  it("checks the account ID and the API base as the backend does", () => {
    expect(issues(values, d1Ui)).toEqual([]);
    const params = (accountId: string, apiBase = "") =>
      issues({ ...values, driverParams: { accountId, apiBase } }, d1Ui);
    // The schema says so too; in the form the lane's words replace it on the same field.
    expect(params("")).toEqual([
      "driverParams.accountId: accountId is required",
      "driverParams.accountId: Enter the account ID",
    ]);
    expect(params("abc")).toEqual([
      "driverParams.accountId: The account ID is 32 hexadecimal characters (Workers & Pages ▸ Overview)",
    ]);
    expect(params(ACCOUNT, "https://gw.example.com")).toEqual([]);
    expect(params(ACCOUNT, "http://localhost:8787")).toEqual([]);
    expect(params(ACCOUNT, "http://[::1]:8787")).toEqual([]);
    expect(params(ACCOUNT, "http://gw.example.com")).toEqual([
      "driverParams.apiBase: The API base must be an https:// URL (http:// only to this machine)",
    ]);
    // No credential in a base stored as plain text (`D1Params::api_base`), even over HTTPS.
    for (const secret of [
      "https://u:p@gw.example/client/v4",
      "https://u@gw.example",
      "https://:p@gw.example",
      "https://gw.example/?key=SECRET",
      "https://gw.example/v4?",
      "https://gw.example/v4#t",
      "http://u:p@127.0.0.1:8787",
    ])
      expect(params(ACCOUNT, secret)).toEqual([`driverParams.apiBase: ${API_BASE_SECRET}`]);
    expect(params(ACCOUNT, "not a url")).toEqual([
      "driverParams.apiBase: Enter the API base as a URL (https://…)",
    ]);
    expect(issues({ ...values, database: "" }, d1Ui)).toEqual(["database: Database is required"]);
  });

  it("offers no SSL modes: the API's certificate is always checked", () => {
    expect(d1Ui.sslModes).toBeNull();
    expect(d1Ui.rows).toEqual(["secret"]);
  });
});
