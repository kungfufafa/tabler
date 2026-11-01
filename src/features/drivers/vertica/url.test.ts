import { describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { SslMode } from "@/lib/ipc";
import lane from ".";
import { formatVerticaUrl, parseVerticaUrl } from "./url";

// Vertica URLs: vertica-python's and JDBC's keys read, a password or token kept out of the
// fields, and every profile written as a URL that reads back to the same profile.

const ui = lane.kinds!.vertica!;

const base = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "vertica",
  host: "vertica.example.com",
  user: "dbadmin",
  database: "VMart",
  ...values,
});

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

/** The profile a URL makes, through the form, as "New connection from URL…" saves it. */
function profileOf(url: string) {
  const parsed = parseVerticaUrl(url);
  if (!parsed) throw new Error(`not read: ${url}`);
  return saved(base({ ...parsed.values }));
}

describe("Vertica URLs", () => {
  it("read user, host, port and database, the password to the secret", () => {
    expect(parseVerticaUrl("vertica://dbadmin:s%40cret@db.example.com:5433/VMart")).toEqual({
      values: {
        host: "db.example.com",
        port: "5433",
        user: "dbadmin",
        database: "VMart",
        driverParams: { auth: "password", clientLabel: "", workload: "", manualCommit: false },
      },
      password: "s@cret",
    });
    // The default port; an IPv6 host without its brackets.
    expect(parseVerticaUrl("vertica://u@[::1]/d")?.values).toMatchObject({
      host: "::1",
      port: "5433",
    });
  });

  it("read JDBC's string, its keys in any case", () => {
    const read = parseVerticaUrl(
      "jdbc:vertica://vnode1.example.com:5433/VMart?user=analyst&password=pw&TLSmode=verify-full&Label=bi&Workload=reporting&AutoCommit=false&LoginTimeout=15",
    );
    expect(read).toEqual({
      values: {
        host: "vnode1.example.com",
        port: "5433",
        user: "analyst",
        database: "VMart",
        ssl: "verify-full",
        connectTimeoutSecs: "15",
        driverParams: {
          auth: "password",
          clientLabel: "bi",
          workload: "reporting",
          manualCommit: true,
        },
      },
      password: "pw",
    });
  });

  it("read the TLS mode in each spelling", () => {
    const ssl = (q: string) => parseVerticaUrl(`vertica://u@h/d?${q}`)?.values.ssl;
    expect(ssl("tlsmode=require")).toBe("require");
    expect(ssl("sslmode=verify-ca")).toBe("verify-ca");
    expect(ssl("ssl_mode=disabled")).toBe("disable");
    expect(ssl("tLSMode=4")).toBe("verify-full");
    expect(ssl("ssl=true")).toBe("require");
    expect(ssl("ssl=false")).toBe("disable");
    expect(ssl("")).toBeUndefined();
  });

  it("are refused when they name a TLS mode this does not know", () => {
    // Keeping the form's default could be weaker than the URL asked for.
    for (const q of [
      "tlsmode=bogus",
      "tlsmode=requre",
      "tLSMode=7",
      "ssl=maybe",
      "tlsmode=constructor&oauth_access_token=t",
      "tlsmode=__proto__",
      "sslmode=toString",
    ])
      expect(parseVerticaUrl(`vertica://u@h/d?${q}`), q).toBeNull();
  });

  it("are neither read nor written with a host that holds a delimiter", () => {
    expect(parseVerticaUrl("vertica://u@h%40evil.example/d")).toBeNull();
    expect(parseVerticaUrl("vertica://u@h%2Fx/d")).toBeNull();
    for (const host of ["h/x?tlsmode=disable#", "h@evil.example", "h?auth=password&x=", "h x"])
      expect(
        formatVerticaUrl(
          saved(base({ host, ssl: "verify-full", driverParams: { auth: "oauth" } })),
        ),
        host,
      ).toBeNull();
  });

  it("take an OAuth token as the secret, over TLS", () => {
    expect(
      parseVerticaUrl("vertica://analyst@h/VMart?oauth_access_token=eyJ.tok.en"),
    ).toMatchObject({
      values: { ssl: "require", driverParams: { auth: "oauth" } },
      password: "eyJ.tok.en",
    });
    expect(
      parseVerticaUrl("vertica://analyst@h/VMart?auth=oauth&tlsmode=verify-full")?.values,
    ).toMatchObject({ ssl: "verify-full", driverParams: { auth: "oauth" } });
  });

  it("are not read without the scheme or a host", () => {
    expect(parseVerticaUrl("postgres://u@h/d")).toBeNull();
    expect(parseVerticaUrl("vertica:///VMart")).toBeNull();
    expect(parseVerticaUrl("vertica://")).toBeNull();
  });

  it("are written without a secret and read back to the same profile", () => {
    const modes: SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];
    const cases: Partial<ConnectionFormValues>[] = [
      ...modes.map((ssl) => ({ ssl })),
      { port: "15433", connectTimeoutSecs: "20" },
      { user: "first last", database: "VMart Dev" },
      { host: "::1" },
      {
        ssl: "verify-full",
        passwordMode: "command",
        passwordCommand: "oidc-token",
        driverParams: {
          auth: "oauth",
          clientLabel: "Tabler & co",
          workload: "reporting",
          manualCommit: true,
        },
      },
    ];
    for (const c of cases) {
      const p = saved(base({ password: "pw", ...c }));
      const url = ui.url!.format(p)!;
      expect(url, JSON.stringify(c)).not.toContain("pw");
      const back = profileOf(url);
      expect(
        { ...back, options: { ...back.options, passwordMode: null, passwordCommand: null } },
        url,
      ).toEqual({ ...p, options: { ...p.options, passwordMode: null, passwordCommand: null } });
      expect(ui.url!.format(back)).toBe(url);
    }
    expect(formatVerticaUrl(saved(base({ ssl: "require" })))).toBe(
      "vertica://dbadmin@vertica.example.com:5433/VMart?tlsmode=require",
    );
    expect(formatVerticaUrl({ ...saved(base()), host: null })).toBeNull();
  });
});
