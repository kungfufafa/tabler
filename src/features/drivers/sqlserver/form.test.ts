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
import { SSL_MODES } from "./form";
import { instanceHint } from "./params";

// The SQL Server form: what it saves (only the params that differ from the backend's defaults,
// no user for logins that send none), what it reads back, and its checks next to the schema's.

const ui = lane.kinds!.sqlserver!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "sqlserver",
  host: "db.example.com",
  user: "sa",
  password: "pw",
  ...values,
});

const params = (p: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...p });

/** The schema's issues and the lane's, as `path: message`. */
function issues(values: ConnectionFormValues): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui.validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

describe("the SQL Server form", () => {
  it("starts on port 1433, a SQL Server login and checked encryption", () => {
    expect(ui.rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
    expect(ui.defaults!()).toEqual({
      port: "1433",
      ssl: "verify-full",
      driverParams: {
        auth: "sqlServer",
        instance: "",
        strict: false,
        hostnameInCertificate: "",
        readOnlyIntent: false,
        applicationName: "",
        multiSubnetFailover: false,
        packetSize: "",
      },
    });
    // The backend's `encryption`: no verify-ca, which checks the name as verify-full does.
    expect(ui.sslModes).toBe(SSL_MODES);
    expect(SSL_MODES.map(([m]) => m)).toEqual(["disable", "prefer", "require", "verify-full"]);
  });

  it("saves a SQL Server login with no params, and reads it back", () => {
    const input = formToProfileInput(form({ database: "shop" }), null, "id-1", ui);
    expect(input.profile).toMatchObject({
      driver: "sqlserver",
      host: "db.example.com",
      port: 1433,
      user: "sa",
      database: "shop",
      ssl: "verify-full",
    });
    expect(input.profile.options.driverParams).toBeNull();
    expect(input.password).toBe("pw");
    const back = profileToForm(input.profile, ui);
    expect(back).toMatchObject({
      host: "db.example.com",
      port: "1433",
      user: "sa",
      database: "shop",
      driverParams: ui.defaults!().driverParams,
    });
    expect(connectionDetail(input.profile, ui)).toBe("db.example.com : 1433 : sa : shop");
  });

  it("saves every param that differs from the defaults, trimmed, and reads them back", () => {
    const all = params({
      auth: "windows",
      instance: " SQLEXPRESS ",
      strict: true,
      hostnameInCertificate: "db.corp ",
      readOnlyIntent: true,
      applicationName: " Reports",
      multiSubnetFailover: true,
      packetSize: "8192",
    });
    const p = formToProfileInput(
      form({ port: "", user: "CORP\\ann", driverParams: all }),
      null,
      "id",
      ui,
    ).profile;
    expect(p.user).toBe("CORP\\ann");
    expect(p.port).toBeNull();
    expect(p.options.driverParams).toEqual({
      auth: "windows",
      instance: "SQLEXPRESS",
      strict: true,
      hostnameInCertificate: "db.corp",
      readOnlyIntent: true,
      applicationName: "Reports",
      multiSubnetFailover: true,
      packetSize: 8192,
    });
    expect(profileToForm(p, ui).driverParams).toEqual({
      auth: "windows",
      instance: "SQLEXPRESS",
      strict: true,
      hostnameInCertificate: "db.corp",
      readOnlyIntent: true,
      applicationName: "Reports",
      multiSubnetFailover: true,
      packetSize: "8192",
    });
    expect(connectionDetail(p, ui)).toBe("db.example.com\\SQLEXPRESS : CORP\\ann");
  });

  it("sends no user for integrated and token logins", () => {
    for (const auth of ["integrated", "entraToken"] as const) {
      const p = formToProfileInput(
        form({ driverParams: params({ auth }) }),
        null,
        "id",
        ui,
      ).profile;
      expect(p.user, auth).toBeNull();
      expect(p.options.driverParams).toEqual({ auth });
    }
    const p = formToProfileInput(
      form({
        driverParams: params({ auth: "entraToken" }),
        passwordMode: "command",
        passwordCommand: "az account get-access-token",
      }),
      null,
      "id",
      ui,
    ).profile;
    expect(p.options.passwordCommand).toBe("az account get-access-token");
    expect(connectionDetail(p, ui)).toBe("db.example.com : 1433 : Entra ID");
  });

  it("reads a profile's params leniently: wrong types count as unset", () => {
    const p = formToProfileInput(form(), null, "id", ui).profile;
    const odd = {
      ...p,
      options: {
        ...p.options,
        driverParams: { auth: "kerberos", strict: "yes", packetSize: 4096, instance: 7 },
      },
    };
    expect(profileToForm(odd, ui).driverParams).toEqual(
      params({ auth: "sqlServer", strict: false, packetSize: "4096", instance: "" }),
    );
  });
});

describe("the form's checks", () => {
  it("need a host (the descriptor) and the user of a login that sends one", () => {
    expect(issues(form({ host: "" }))).toEqual(["host: Host is required"]);
    expect(issues(form({ user: "" }))).toEqual(["user: Enter the user of the SQL Server login"]);
    expect(issues(form({ user: "", driverParams: params({ auth: "windows" }) }))).toEqual([
      "user: Enter the Windows user as DOMAIN\\user",
    ]);
    expect(issues(form({ user: "", driverParams: params({ auth: "integrated" }) }))).toEqual([]);
    expect(issues(form({ user: "", driverParams: params({ auth: "entraToken" }) }))).toEqual([]);
  });

  it("need a token for an Entra ID login", () => {
    expect(
      issues(form({ passwordMode: "none", driverParams: params({ auth: "entraToken" }) })),
    ).toEqual([
      "passwordMode: An Entra ID token is the password: store it, ask for it or run a command",
    ]);
    expect(
      issues(form({ passwordMode: "command", driverParams: params({ auth: "entraToken" }) })),
    ).toEqual(["passwordCommand: Enter the command that prints the password"]);
  });

  it("check a named instance as the backend does, and where SQL Browser reaches it", () => {
    expect(issues(form({ port: "", driverParams: params({ instance: "SQL EXPRESS" }) }))).toEqual([
      "driverParams.instance: Up to 16 bytes of letters, digits, _, $ or #",
    ]);
    expect(
      issues(form({ port: "", driverParams: params({ instance: "A234567890123456X" }) })),
    ).toEqual(["driverParams.instance: Up to 16 bytes of letters, digits, _, $ or #"]);
    expect(issues(form({ port: "", driverParams: params({ instance: "Inst_$#1" }) }))).toEqual([]);
    // `str::len` counts bytes: 9 letters of 2 bytes each are too many.
    expect(issues(form({ port: "", driverParams: params({ instance: "ÄÄÄÄÄÄÄÄÄ" }) }))).toEqual([
      "driverParams.instance: Up to 16 bytes of letters, digits, _, $ or #",
    ]);
    expect(issues(form({ port: "", driverParams: params({ instance: "ÄÄÄÄÄÄÄÄ" }) }))).toEqual([]);
    // With a port or over SSH the backend ignores the instance: the field says so, the profile
    // saves (a pasted `host\SQLEXPRESS:1433` stays editable).
    expect(issues(form({ driverParams: params({ instance: "SQLEXPRESS" }) }))).toEqual([]);
    const ssh = { sshEnabled: true, sshHost: "bastion", driverParams: params({ instance: "X" }) };
    expect(issues(form({ port: "", ...ssh }))).toEqual([]);
    expect(issues(form(ssh))).toEqual([]);
    expect(instanceHint(false, "")).toMatch(/^Optional: a named instance/);
    expect(instanceHint(false, "1433")).toMatch(/^Ignored while a port is set/);
    expect(instanceHint(true, "")).toMatch(/^Ignored over SSH/);
  });

  it("check the packet size's range", () => {
    for (const bad of ["100", "40000", "8k"])
      expect(issues(form({ driverParams: params({ packetSize: bad }) })), bad).toEqual([
        "driverParams.packetSize: Bytes, 512 to 32767",
      ]);
    expect(issues(form({ driverParams: params({ packetSize: "512" }) }))).toEqual([]);
    expect(issues(form({ driverParams: params({ packetSize: "32767" }) }))).toEqual([]);
  });

  it("check that a client certificate comes with its key, as the backend's validate does", () => {
    expect(issues(form({ sslCertPath: "/c.pem" }))).toEqual([
      "sslKeyPath: Choose the private key of the certificate (Key…)",
    ]);
  });
});
