import { describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile } from "@/lib/ipc";
import lane from ".";
import { parseAdoString, parseConnectionString, parseSqlServerUrl } from "./url";

// SQL Server's connection strings: URLs of both schemes, JDBC and ADO.NET, read into the form
// (a password or token into the secret), and a profile written as a URL that reads back to it.

const ui = lane.kinds!.sqlserver!;
const url = ui.url!;

/** What a URL makes of a new form, then saved: the profile and the secret. */
function saved(
  text: string,
  parse: (text: string) => ParsedConnectionUrl | null = url.parse,
): { profile: ConnectionProfile; password: string | null } {
  const parsed = parse(text);
  if (!parsed) throw new Error(`not read: ${text}`);
  const values: ConnectionFormValues = {
    ...DEFAULT_FORM,
    ...ui.defaults!(),
    driver: "sqlserver",
    name: "n",
    ...parsed.values,
    driverParams: { ...ui.defaults!().driverParams, ...parsed.values.driverParams },
  };
  return {
    profile: formToProfileInput(values, null, "id-1", ui).profile,
    password: parsed.password,
  };
}

describe("sqlserver:// and mssql:// URLs", () => {
  it("read the user, password, host, port and database", () => {
    for (const scheme of url.schemes) {
      const p = parseSqlServerUrl(`${scheme}://sa:p%40ss@db.example.com:14330/shop`);
      expect(p?.password).toBe("p@ss");
      expect(p?.values).toMatchObject({
        host: "db.example.com",
        port: "14330",
        user: "sa",
        database: "shop",
        ssl: "verify-full",
        driverParams: { auth: "sqlServer", instance: "", strict: false },
      });
    }
  });

  it("read jdbc: before the scheme, a named instance and IPv6", () => {
    expect(parseSqlServerUrl("jdbc:sqlserver://host\\SQLEXPRESS")?.values).toMatchObject({
      host: "host",
      port: "",
      driverParams: { instance: "SQLEXPRESS" },
    });
    expect(parseSqlServerUrl("mssql://[::1]:1433/db")?.values).toMatchObject({
      host: "::1",
      port: "1433",
      database: "db",
    });
  });

  it("read encryption as the SSL mode and strict", () => {
    const ssl = (q: string) => {
      const v = parseSqlServerUrl(`sqlserver://h/db?${q}`)?.values;
      return [v?.ssl, (v?.driverParams as { strict: boolean }).strict];
    };
    expect(ssl("encrypt=false")).toEqual(["disable", false]);
    expect(ssl("encrypt=optional")).toEqual(["prefer", false]);
    expect(ssl("encrypt=true&trustServerCertificate=true")).toEqual(["require", false]);
    expect(ssl("encrypt=mandatory")).toEqual(["verify-full", false]);
    expect(ssl("Encrypt=yes")).toEqual(["verify-full", false]);
    expect(ssl("encrypt=strict")).toEqual(["verify-full", true]);
    expect(ssl("encrypt=strict&TrustServerCertificate=true")).toEqual(["require", true]);
    // JDBC's default: encrypted and checked.
    expect(ssl("")).toEqual(["verify-full", false]);
  });

  it("read the params, and leave out what they do not know", () => {
    const v = parseSqlServerUrl(
      "sqlserver://h?auth=windows&user=CORP%5Cann&instance=X1&hostNameInCertificate=db.corp&applicationIntent=ReadOnly&applicationName=Reports&multiSubnetFailover=true&packetSize=8192&loginTimeout=15&jit=off",
    )?.values;
    expect(v).toMatchObject({
      user: "CORP\\ann",
      connectTimeoutSecs: "15",
      driverParams: {
        auth: "windows",
        instance: "X1",
        hostnameInCertificate: "db.corp",
        readOnlyIntent: true,
        applicationName: "Reports",
        multiSubnetFailover: true,
        packetSize: "8192",
      },
    });
  });

  it("are not read from another scheme or without a host", () => {
    expect(parseSqlServerUrl("postgres://h/db")).toBeNull();
    expect(parseSqlServerUrl("sqlserver:///db")).toBeNull();
    expect(parseSqlServerUrl("sqlserver://")).toBeNull();
  });
});

describe("JDBC strings", () => {
  it("read properties after the server, braced values and the password as the secret", () => {
    const p = parseSqlServerUrl(
      "jdbc:sqlserver://db.example.com:1433;databaseName=shop;user=app;password={a;b}}c};encrypt=true;trustServerCertificate=true;",
    );
    expect(p?.password).toBe("a;b}c");
    expect(p?.values).toMatchObject({
      host: "db.example.com",
      port: "1433",
      database: "shop",
      user: "app",
      ssl: "require",
    });
  });

  it("read the server from serverName, integrated security and NTLM", () => {
    const v = parseConnectionString(
      "sqlserver://;serverName=db1;instanceName=INST;integratedSecurity=true",
    )?.values;
    expect(v).toMatchObject({
      host: "db1",
      user: "",
      driverParams: { auth: "integrated", instance: "INST" },
    });
    const ntlm = parseSqlServerUrl(
      "sqlserver://db1;integratedSecurity=true;authenticationScheme=NTLM;user=CORP\\ann;password=pw",
    );
    expect(ntlm?.values).toMatchObject({ user: "CORP\\ann", driverParams: { auth: "windows" } });
    expect(ntlm?.password).toBe("pw");
  });

  it("never pick the Windows user's own login from a link (url.parse, which deep links reach)", () => {
    expect(url.parse).toBe(parseSqlServerUrl);
    for (const text of [
      "sqlserver://evil.example.com?auth=integrated",
      "sqlserver://evil.example.com?auth=INTEGRATED",
      "sqlserver://evil.example.com;integratedSecurity=true",
      "jdbc:sqlserver://evil.example.com;trusted_connection=yes",
      "mssql://evil.example.com;integratedSecurity=true;authenticationScheme=NTLM",
    ]) {
      const p = parseSqlServerUrl(text);
      expect(p?.values, text).toMatchObject({
        host: "evil.example.com",
        user: "",
        driverParams: { auth: "sqlServer" },
      });
      // So the form asks for a user: nothing connects until one is given.
      const issues = ui.validate!({
        ...DEFAULT_FORM,
        ...ui.defaults!(),
        ...p!.values,
        driverParams: { ...ui.defaults!().driverParams, ...p!.values.driverParams },
      });
      expect(
        issues.map((i) => i.path),
        text,
      ).toContain("user");
      // The Import box of the open form, where the login it picks is in view, keeps it.
      expect(parseConnectionString(text)?.values.driverParams, text).toMatchObject({
        auth: "integrated",
      });
    }
  });

  it("read an access token as an Entra ID login whose secret is the token", () => {
    const p = parseSqlServerUrl(
      "sqlserver://x.database.windows.net;accessToken=eyJ.a.b;user=ignored",
    );
    expect(p?.values).toMatchObject({ user: "", driverParams: { auth: "entraToken" } });
    expect(p?.password).toBe("eyJ.a.b");
  });
});

describe("ADO.NET strings", () => {
  it("read the server with tcp:, its port after a comma, and the usual keys", () => {
    const p = parseAdoString(
      "Server=tcp:db.example.com,1444;Initial Catalog=shop;Persist Security Info=False;User ID=app;Password='it''s';Encrypt=True;TrustServerCertificate=False;Connection Timeout=30;Application Name=Reports;ApplicationIntent=ReadOnly",
    );
    expect(p?.password).toBe("it's");
    expect(p?.values).toMatchObject({
      host: "db.example.com",
      port: "1444",
      database: "shop",
      user: "app",
      ssl: "verify-full",
      connectTimeoutSecs: "30",
      driverParams: { applicationName: "Reports", readOnlyIntent: true },
    });
  });

  it("read this machine, a named instance and integrated security", () => {
    const v = parseAdoString(
      "Data Source=.\\SQLEXPRESS;Integrated Security=SSPI;Database=d",
    )?.values;
    expect(v).toMatchObject({
      host: "localhost",
      database: "d",
      driverParams: { auth: "integrated", instance: "SQLEXPRESS" },
    });
    expect(parseAdoString("Server=(local);Trusted_Connection=yes")?.values.host).toBe("localhost");
  });

  it("refuse named pipes and shared memory, and text without a server", () => {
    expect(parseAdoString("Server=np:\\\\.\\pipe\\sql\\query;Database=d")).toBeNull();
    expect(parseAdoString("Server=lpc:.;Database=d")).toBeNull();
    expect(parseAdoString("Database=d;User ID=u")).toBeNull();
    expect(parseAdoString("")).toBeNull();
  });

  it("are told apart from URLs by parseConnectionString", () => {
    expect(parseConnectionString("Server=h;Database=d")?.values.host).toBe("h");
    expect(parseConnectionString("jdbc:sqlserver://h2;databaseName=d")?.values.host).toBe("h2");
    expect(parseConnectionString("mssql://h3")?.values.host).toBe("h3");
  });
});

describe("a profile's URL", () => {
  it("round-trips through the form for every scheme, login and SSL mode the form offers", () => {
    const cases = [
      "sqlserver://sa@db.example.com:1433/shop?encrypt=true",
      "sqlserver://db.example.com/master?encrypt=false",
      "sqlserver://app@10.0.0.5:1500?encrypt=optional",
      "sqlserver://app@h?encrypt=true&trustServerCertificate=true&instance=SQLEXPRESS",
      "sqlserver://CORP%5Cann@h:1433/d?encrypt=strict&auth=windows&hostNameInCertificate=db.corp&applicationIntent=ReadOnly&applicationName=Reports&multiSubnetFailover=true&packetSize=8192&loginTimeout=20",
      "sqlserver://x.database.windows.net:1433/db?encrypt=true&auth=entraToken",
      "sqlserver://[::1]:1433?encrypt=true",
    ];
    for (const text of cases) {
      const { profile } = saved(text);
      expect(url.format(profile), text).toBe(text);
      for (const scheme of url.schemes) {
        const again = saved(url.format(profile)!.replace(/^sqlserver:/, `${scheme}:`)).profile;
        expect(again, `${scheme}: ${text}`).toEqual(profile);
      }
    }
  });

  it("round-trips an integrated login through the form's Import box, not through a link", () => {
    const text = "sqlserver://h:1433?encrypt=strict&trustServerCertificate=true&auth=integrated";
    const { profile } = saved(text, parseConnectionString);
    expect(profile.options.driverParams).toMatchObject({ auth: "integrated" });
    expect(url.format(profile)).toBe(text);
    expect(saved(text).profile.options.driverParams?.auth).toBeUndefined();
  });

  it("never holds the password, and has none without a host", () => {
    const { profile, password } = saved("sqlserver://sa:secret@h/db?encrypt=true");
    expect(password).toBe("secret");
    expect(url.format(profile)).toBe("sqlserver://sa@h/db?encrypt=true");
    expect(url.format({ ...profile, host: null })).toBeNull();
  });

  it("keeps verify-ca as checked encryption, which the backend treats as verify-full", () => {
    const { profile } = saved("sqlserver://h?encrypt=true");
    expect(url.format({ ...profile, ssl: "verify-ca" })).toBe("sqlserver://h?encrypt=true");
  });
});
