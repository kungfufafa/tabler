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

// The ClickHouse form: what it saves (only the params that differ from the backend's defaults,
// roles and settings as lists, Prefer as verify-full), what it reads back, and its checks next to
// the schema's, in `entry::validate`'s words.

const ui = lane.kinds!.clickhouse!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "clickhouse",
  host: "ch.example.com",
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

describe("the ClickHouse form", () => {
  it("starts on HTTP port 8123 as `default`, with the backend's defaults", () => {
    expect(ui.rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
    expect(ui.defaults!()).toEqual({
      port: "8123",
      user: "default",
      ssl: "disable",
      driverParams: { certificateAuth: false, roles: "", settings: "", sessionTimeoutSecs: "" },
    });
    // `entry::certificate_check`: no plain HTTP to fall back to, so no Prefer.
    expect(ui.sslModes).toBe(SSL_MODES);
    expect(SSL_MODES.map(([m]) => m)).toEqual(["disable", "require", "verify-ca", "verify-full"]);
  });

  it("saves a profile with no params, and reads it back", () => {
    const input = formToProfileInput(form({ database: "shop" }), null, "id-1", ui);
    expect(input.profile).toMatchObject({
      driver: "clickhouse",
      host: "ch.example.com",
      port: 8123,
      user: "default",
      database: "shop",
      ssl: "disable",
    });
    expect(input.profile.options.driverParams).toBeNull();
    expect(input.password).toBe("pw");
    const back = profileToForm(input.profile, ui);
    expect(back).toMatchObject({
      host: "ch.example.com",
      port: "8123",
      user: "default",
      database: "shop",
      driverParams: ui.defaults!().driverParams,
    });
    expect(connectionDetail(input.profile, ui)).toBe("ch.example.com : 8123 : default : shop");
  });

  it("saves every param as `ClickhouseParams` holds it, and reads them back", () => {
    const all = params({
      certificateAuth: true,
      roles: " analyst , ,reporting ",
      settings: "max_execution_time = 60\n\n  max_threads=4  \njoin_use_nulls = 1",
      sessionTimeoutSecs: "600",
    });
    const files = { sslCertPath: "/c.pem", sslKeyPath: "/k.pem" };
    const p = formToProfileInput(
      form({ ssl: "verify-full", port: "8443", driverParams: all, ...files }),
      null,
      "id",
      ui,
    ).profile;
    expect(p.options.driverParams).toEqual({
      certificateAuth: true,
      roles: ["analyst", "reporting"],
      settings: [
        { name: "max_execution_time", value: "60" },
        { name: "max_threads", value: "4" },
        { name: "join_use_nulls", value: "1" },
      ],
      sessionTimeoutSecs: 600,
    });
    expect(profileToForm(p, ui).driverParams).toEqual({
      certificateAuth: true,
      roles: "analyst, reporting",
      settings: "max_execution_time = 60\nmax_threads = 4\njoin_use_nulls = 1",
      sessionTimeoutSecs: "600",
    });
    expect(connectionDetail(p, ui)).toBe(
      "ch.example.com : 8443 : default (certificate) : analyst, reporting",
    );
  });

  it("saves Prefer as verify-full, the check the backend makes of it", () => {
    const p = formToProfileInput(form({ ssl: "prefer" }), null, "id", ui).profile;
    expect(p.ssl).toBe("verify-full");
    const plain = formToProfileInput(form({ ssl: "require" }), null, "id", ui).profile;
    expect(plain.ssl).toBe("require");
  });

  it("reads a profile's params leniently: wrong types count as unset", () => {
    const p = formToProfileInput(form(), null, "id", ui).profile;
    const odd = {
      ...p,
      options: {
        ...p.options,
        driverParams: {
          certificateAuth: "yes",
          roles: ["a", 3, "b"],
          settings: [{ name: "x", value: "1" }, { name: 2 }, "y=2"],
          sessionTimeoutSecs: 90,
        },
      },
    };
    expect(profileToForm(odd, ui).driverParams).toEqual(
      params({ roles: "a, b", settings: "x = 1", sessionTimeoutSecs: "90" }),
    );
  });
});

describe("the form's checks", () => {
  it("need a host (the descriptor) and nothing else of a plain profile", () => {
    expect(issues(form())).toEqual([]);
    expect(issues(form({ host: "" }))).toEqual(["host: Host is required"]);
    // An empty user is `default` to the backend.
    expect(issues(form({ user: "" }))).toEqual([]);
  });

  it("refuse settings as `request::validate_settings` does", () => {
    const with_ = (settings: string) => issues(form({ driverParams: params({ settings }) }));
    expect(with_("max_threads = 8\nmax_memory_usage = 10000000000")).toEqual([]);
    expect(with_("max-threads = 8")).toEqual([
      "driverParams.settings: “max-threads” is not a ClickHouse setting name",
    ]);
    for (const reserved of ["readonly", "Dialect", "param_x", "result_overflow_mode", "profile"])
      expect(with_(`${reserved} = 1`), reserved).toEqual([
        `driverParams.settings: “${reserved}” is set by Tabler or changes how statements are read, so it cannot be a connection setting`,
      ]);
    expect(with_("max_threads 8")).toEqual([
      "driverParams.settings: Line 1: write each setting as name = value",
    ]);
    expect(with_("\n= 8")).toEqual([
      "driverParams.settings: Line 2: write each setting as name = value",
    ]);
  });

  it("check the session timeout and role names as `entry::validate` does", () => {
    for (const bad of ["0", "3601", "abc", "1.5"])
      expect(issues(form({ driverParams: params({ sessionTimeoutSecs: bad }) })), bad).toEqual([
        "driverParams.sessionTimeoutSecs: Seconds, 1 to 3600",
      ]);
    expect(issues(form({ driverParams: params({ sessionTimeoutSecs: "3600" }) }))).toEqual([]);
    expect(issues(form({ driverParams: params({ roles: "a, b\tc" }) }))).toEqual([
      "driverParams.roles: “b\tc” is not a role name",
    ]);
  });

  it("want HTTPS and both files for a certificate sign-in", () => {
    const cert = params({ certificateAuth: true });
    expect(issues(form({ driverParams: cert }))).toEqual([
      "driverParams.certificateAuth: Signing in with a certificate needs HTTPS (turn SSL on)",
    ]);
    expect(issues(form({ ssl: "verify-full", driverParams: cert }))).toEqual([
      "driverParams.certificateAuth: Signing in with a certificate needs the client certificate and key files",
    ]);
    expect(
      issues(
        form({
          ssl: "verify-full",
          driverParams: cert,
          sslCertPath: "/c.pem",
          sslKeyPath: "/k.pem",
        }),
      ),
    ).toEqual([]);
  });

  it("want both files of a client certificate while TLS is on", () => {
    expect(issues(form({ ssl: "require", sslCertPath: "/c.pem" }))).toEqual([
      "sslKeyPath: Choose the private key of the certificate (Key…)",
    ]);
    expect(issues(form({ ssl: "disable", sslCertPath: "/c.pem" }))).toEqual([]);
  });

  it("want the command of a Command password", () => {
    expect(issues(form({ passwordMode: "command" }))).toEqual([
      "passwordCommand: Enter the command that prints the password",
    ]);
  });
});
