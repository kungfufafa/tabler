import { describe, expect, it } from "vitest";
import {
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { DatabaseKind } from "@/lib/ipc";
import lane from ".";

// The three tiles: PostgreSQL profiles with the engine in `pgFlavor` and no driver params, the
// tile's defaults, and the checks of what an engine cannot take next to the schema's.

const ui = (kind: DatabaseKind) => lane.kinds![kind]!;

const form = (
  kind: "cockroachdb" | "greenplum" | "redshift",
  values: Partial<ConnectionFormValues> = {},
): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui(kind).defaults?.(),
  name: "n",
  driver: "postgres",
  pgFlavor: kind,
  host: "db.example.com",
  user: "u",
  password: "pw",
  ...values,
});

function issues(kind: DatabaseKind, values: ConnectionFormValues): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui(kind).validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

describe("the PostgreSQL flavors' forms", () => {
  it("are the PostgreSQL form with each engine's defaults", () => {
    for (const kind of ["cockroachdb", "greenplum", "redshift"] as const) {
      expect(ui(kind).rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
      expect(ui(kind).sslModes?.map(([m]) => m)).toEqual([
        "disable",
        "prefer",
        "require",
        "verify-ca",
        "verify-full",
      ]);
    }
    expect(ui("cockroachdb").defaults!()).toEqual({ user: "root" });
    expect(ui("greenplum").defaults).toBeUndefined();
    expect(ui("redshift").defaults!()).toEqual({ ssl: "require" });
    expect(ui("redshift").Fields).toBeDefined();
    expect(ui("cockroachdb").Fields).toBeUndefined();
  });

  it("save a PostgreSQL profile with the engine and no params, and read it back", () => {
    for (const kind of ["cockroachdb", "greenplum", "redshift"] as const) {
      const input = formToProfileInput(
        form(kind, { port: "26257", database: "shop" }),
        null,
        "id-1",
        ui(kind),
      );
      expect(input.profile).toMatchObject({
        driver: "postgres",
        host: "db.example.com",
        port: 26257,
        user: "u",
        database: "shop",
      });
      expect(input.profile.options.pgFlavor).toBe(kind);
      expect(input.profile.options.driverParams).toBeNull();
      expect(input.password).toBe("pw");
      const back = profileToForm(input.profile, ui(kind));
      expect(back).toMatchObject({
        pgFlavor: kind,
        host: "db.example.com",
        port: "26257",
        database: "shop",
      });
      expect(issues(kind, back)).toEqual([]);
    }
  });

  it("refuse direct TLS on Greenplum and Redshift, not on CockroachDB", () => {
    const direct = { pgSslNegotiation: "direct" as const, ssl: "require" as const };
    expect(issues("greenplum", form("greenplum", direct))).toEqual([
      "pgSslNegotiation: Greenplum has no direct TLS negotiation (PostgreSQL 17's): use Postgres",
    ]);
    expect(issues("redshift", form("redshift", direct))).toEqual([
      "pgSslNegotiation: Redshift has no direct TLS negotiation (PostgreSQL 17's): use Postgres",
    ]);
    expect(issues("cockroachdb", form("cockroachdb", direct))).toEqual([]);
  });

  it("refuse a Redshift profile without a password", () => {
    expect(issues("redshift", form("redshift", { passwordMode: "none" }))).toEqual([
      "passwordMode: Redshift asks every user for a password: store it, ask for it or run a command (IAM)",
    ]);
    expect(
      issues("redshift", form("redshift", { passwordMode: "command", passwordCommand: "x" })),
    ).toEqual([]);
    expect(issues("greenplum", form("greenplum", { passwordMode: "none" }))).toEqual([]);
  });

  it("refuse a Redshift encoding other than UTF-8 unless read-only is on", () => {
    expect(issues("redshift", form("redshift", { encoding: "LATIN1" }))).toEqual([
      "encoding: Redshift speaks only UTF-8: leave Encoding empty, or turn read-only on",
    ]);
    for (const encoding of ["", "UTF8", " utf-8 ", "UNICODE"])
      expect(issues("redshift", form("redshift", { encoding })), encoding).toEqual([]);
    expect(issues("redshift", form("redshift", { encoding: "LATIN1", readOnly: true }))).toEqual(
      [],
    );
    expect(issues("greenplum", form("greenplum", { encoding: "LATIN1" }))).toEqual([]);
  });
});
