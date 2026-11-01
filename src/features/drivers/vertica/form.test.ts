import { describe, expect, it } from "vitest";
import {
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import { DESCRIPTORS } from "@/features/drivers/catalog";
import lane from ".";
import { nameProblem, toVerticaParams, verticaParamsOf } from "./params";

// The Vertica form: what it saves (only the params that differ from the backend's defaults),
// what it reads back, and its checks next to the schema's, in the words of `entry::validate`.

const ui = lane.kinds!.vertica!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "vertica",
  host: "vertica.example.com",
  user: "dbadmin",
  database: "VMart",
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

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

describe("the Vertica form", () => {
  it("starts on port 5433 with a password login, the shared rows and libpq's SSL modes", () => {
    expect(ui.rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
    expect(ui.defaults!()).toEqual({
      port: "5433",
      driverParams: { auth: "password", clientLabel: "", workload: "", manualCommit: false },
    });
    // The descriptor's TLS style names the modes, as `connect::Dial` reads all five.
    expect(ui.sslModes).toBeUndefined();
    expect(DESCRIPTORS.vertica.tls).toBe("libpq");
    expect(DESCRIPTORS.vertica.defaultPort).toBe(5433);
    expect(ui.url!.schemes).toEqual(DESCRIPTORS.vertica.urlSchemes);
  });

  it("saves only the params that differ from the defaults, and reads them back", () => {
    expect(saved(form()).options.driverParams).toBeNull();
    const p = saved(
      form({
        ssl: "verify-full",
        passwordMode: "command",
        passwordCommand: "oidc-token vmart",
        driverParams: params({
          auth: "oauth",
          clientLabel: " reports ",
          workload: "analytics",
          manualCommit: true,
        }),
      }),
    );
    expect(p.options.driverParams).toEqual({
      auth: "oauth",
      clientLabel: "reports",
      workload: "analytics",
      manualCommit: true,
    });
    expect(p.port).toBe(5433);
    const back = { ...profileToForm(p), ...ui.fromProfile!(p) };
    expect(back.driverParams).toEqual({
      auth: "oauth",
      clientLabel: "reports",
      workload: "analytics",
      manualCommit: true,
    });
    expect(saved({ ...form(), ...back, password: "" }).options.driverParams).toEqual(
      p.options.driverParams,
    );
  });

  it("reads the params leniently: the wrong type counts as unset", () => {
    expect(verticaParamsOf({ auth: "kerberos", clientLabel: 3, manualCommit: "yes" })).toEqual({
      auth: "password",
      clientLabel: "",
      workload: "",
      manualCommit: false,
    });
    expect(toVerticaParams(verticaParamsOf(null))).toBeNull();
  });

  it("checks the label, the workload and an OAuth login as the backend does", () => {
    expect(issues(form())).toEqual([]);
    expect(nameProblem("w".repeat(128))).toBeNull();
    // Code points, as Rust counts `chars()`: 128 emoji are 256 UTF-16 units and still fine.
    expect(nameProblem("\u{1F600}".repeat(128))).toBeNull();
    expect(
      issues(
        form({ driverParams: params({ clientLabel: "a\u0007b", workload: "w".repeat(129) }) }),
      ),
    ).toEqual([
      "driverParams.clientLabel: Up to 128 characters, no line breaks",
      "driverParams.workload: Up to 128 characters, no line breaks",
    ]);
    const oauth = (v: Partial<ConnectionFormValues>) =>
      issues(form({ driverParams: params({ auth: "oauth" }), ...v }));
    expect(oauth({ ssl: "require" })).toEqual([]);
    expect(oauth({ ssl: "verify-ca" })).toEqual([]);
    for (const ssl of ["disable", "prefer"] as const)
      expect(oauth({ ssl })).toEqual([
        "ssl: An OAuth token is sent only over TLS: choose Require or stricter",
      ]);
    expect(oauth({ ssl: "require", passwordMode: "none" })).toEqual([
      "passwordMode: An OAuth token is the password: store it, ask for it or run a command",
    ]);
  });

  it("sums a profile up as user@host:port/database", () => {
    expect(ui.summary!(saved(form()))).toBe("dbadmin@vertica.example.com:5433/VMart");
    expect(
      ui.summary!(
        saved(form({ ssl: "require", port: "", driverParams: params({ auth: "oauth" }) })),
      ),
    ).toBe("dbadmin@vertica.example.com/VMart (OAuth)");
  });
});
