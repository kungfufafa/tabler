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

// The Cassandra form: what it saves (only the params that differ from the backend's defaults,
// numbers as numbers, Prefer as Disable), what it reads back, and its checks next to the schema's.

const ui = lane.kinds!.cassandra!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "cassandra",
  host: "db.example.com",
  user: "cassandra",
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

describe("the Cassandra form", () => {
  it("starts on port 9042 without TLS and with the backend's defaults", () => {
    expect(ui.rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
    expect(ui.defaults!()).toEqual({
      port: "9042",
      user: "",
      ssl: "disable",
      driverParams: {
        consistency: "LOCAL_ONE",
        serialConsistency: "LOCAL_SERIAL",
        localDatacenter: "",
        requestTimeoutSecs: "",
        pageSize: "",
        compression: "none",
        singleNode: false,
      },
    });
    // `entry::dial`: no negotiation, so no Prefer.
    expect(ui.sslModes).toBe(SSL_MODES);
    expect(SSL_MODES.map(([m]) => m)).toEqual(["disable", "require", "verify-ca", "verify-full"]);
  });

  it("saves a profile with no params, and reads it back", () => {
    const input = formToProfileInput(form({ database: "shop" }), null, "id-1", ui);
    expect(input.profile).toMatchObject({
      driver: "cassandra",
      host: "db.example.com",
      port: 9042,
      user: "cassandra",
      database: "shop",
      ssl: "disable",
    });
    expect(input.profile.options.driverParams).toBeNull();
    expect(input.password).toBe("pw");
    const back = profileToForm(input.profile, ui);
    expect(back).toMatchObject({
      host: "db.example.com",
      port: "9042",
      user: "cassandra",
      database: "shop",
      driverParams: ui.defaults!().driverParams,
    });
    expect(connectionDetail(input.profile, ui)).toBe("db.example.com : 9042 : cassandra : shop");
  });

  it("saves every param that differs from the defaults, numbers as numbers, and reads them back", () => {
    const all = params({
      consistency: "LOCAL_QUORUM",
      serialConsistency: "SERIAL",
      localDatacenter: " dc1 ",
      requestTimeoutSecs: "5",
      pageSize: "500",
      compression: "lz4",
      singleNode: true,
    });
    const p = formToProfileInput(form({ port: "", driverParams: all }), null, "id", ui).profile;
    expect(p.port).toBeNull();
    expect(p.options.driverParams).toEqual({
      consistency: "LOCAL_QUORUM",
      serialConsistency: "SERIAL",
      localDatacenter: "dc1",
      requestTimeoutSecs: 5,
      pageSize: 500,
      compression: "lz4",
      singleNode: true,
    });
    expect(profileToForm(p, ui).driverParams).toEqual({
      consistency: "LOCAL_QUORUM",
      serialConsistency: "SERIAL",
      localDatacenter: "dc1",
      requestTimeoutSecs: "5",
      pageSize: "500",
      compression: "lz4",
      singleNode: true,
    });
    expect(connectionDetail(p, ui)).toBe("db.example.com : cassandra : LOCAL_QUORUM");
  });

  it("saves Prefer as Disable without client files, and keeps the files TLS uses", () => {
    const files = { sslCertPath: "/c.pem", sslKeyPath: "/k.pem", sslCaPath: "/ca.pem" };
    const prefer = formToProfileInput(form({ ssl: "prefer", ...files }), null, "id", ui).profile;
    expect(prefer.ssl).toBe("disable");
    expect(prefer.options.sslCertPath).toBeNull();
    expect(prefer.options.sslKeyPath).toBeNull();
    const tls = formToProfileInput(form({ ssl: "verify-full", ...files }), null, "id", ui).profile;
    expect(tls).toMatchObject({ ssl: "verify-full", sslCaPath: "/ca.pem" });
    expect(tls.options).toMatchObject({ sslCertPath: "/c.pem", sslKeyPath: "/k.pem" });
  });

  it("sends no user when the form has none (no authentication)", () => {
    const p = formToProfileInput(form({ user: "", password: "" }), null, "id", ui).profile;
    expect(p.user).toBeNull();
  });

  it("reads a profile's params leniently: wrong types count as unset", () => {
    const p = formToProfileInput(form(), null, "id", ui).profile;
    const odd = {
      ...p,
      options: {
        ...p.options,
        driverParams: { consistency: "SERIAL", pageSize: 250, singleNode: "yes", compression: 4 },
      },
    };
    expect(profileToForm(odd, ui).driverParams).toEqual(params({ pageSize: "250" }));
  });
});

describe("the form's checks", () => {
  it("need a host (the descriptor) and nothing else of a plain profile", () => {
    expect(issues(form())).toEqual([]);
    expect(issues(form({ host: "" }))).toEqual(["host: Host is required"]);
    expect(issues(form({ user: "" }))).toEqual([]);
  });

  it("check the request timeout and page size as `CassandraParams::check` does", () => {
    for (const bad of ["0", "3601", "abc", "1.5"])
      expect(issues(form({ driverParams: params({ requestTimeoutSecs: bad }) })), bad).toEqual([
        "driverParams.requestTimeoutSecs: Seconds, 1 to 3600",
      ]);
    for (const bad of ["0", "10001", "-1"])
      expect(issues(form({ driverParams: params({ pageSize: bad }) })), bad).toEqual([
        "driverParams.pageSize: Rows, 1 to 10,000",
      ]);
    expect(
      issues(form({ driverParams: params({ requestTimeoutSecs: "3600", pageSize: "10000" }) })),
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
