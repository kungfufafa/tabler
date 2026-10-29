import { describe, expect, it } from "vitest";
import { fakeLane } from "@/features/drivers/__fixtures__/fakeLane";
import type { ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import {
  connectionDetail,
  connectionFormSchema,
  DEFAULT_FORM,
  formKind,
  formToProfileInput,
  passwordModesFor,
  profileToForm,
  type ConnectionFormValues,
} from "./schema";

// The connection form's values for a lane's database (F9b): what its descriptor requires, the
// lane form's driver params, and the common fields read by the descriptor.

const ui = fakeLane("mongodb", "mongodb").kinds!.mongodb!;

const issues = (values: Partial<ConnectionFormValues>) => {
  const r = connectionFormSchema.safeParse({ ...DEFAULT_FORM, name: "n", ...values });
  return r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
};

const docs: ConnectionProfile = {
  ...SEED_PROFILES[0]!,
  id: "p-docs",
  name: "Docs",
  driver: "mongodb",
  host: "db.example",
  port: 27018,
  user: null,
  database: null,
  options: { ...SEED_PROFILES[0]!.options, driverParams: { authSource: "audit" } },
};

describe("a lane database's form values", () => {
  it("name the database by driver and flavor", () => {
    expect(formKind({ driver: "mongodb", mysqlVersion: "default", pgFlavor: null })).toBe(
      "mongodb",
    );
    expect(formKind({ driver: "postgres", mysqlVersion: "default", pgFlavor: "redshift" })).toBe(
      "redshift",
    );
    expect(formKind({ driver: "mysql", mysqlVersion: "mariadb", pgFlavor: null })).toBe("mariadb");
  });

  it("require what the descriptor requires, in its words", () => {
    // Redis needs a host only (no user, unlike the core drivers).
    expect(issues({ driver: "redis", host: "", user: "" })).toEqual(["host: Host is required"]);
    expect(issues({ driver: "redis", host: "cache", user: "" })).toEqual([]);
    // DynamoDB names its database "Region"; D1 needs its account id as a driver param.
    expect(issues({ driver: "dynamodb", database: "" })).toEqual(["database: Region is required"]);
    expect(issues({ driver: "d1", database: "db" })).toEqual([
      "driverParams.accountId: accountId is required",
    ]);
    expect(issues({ driver: "d1", database: "db", driverParams: { accountId: "abc" } })).toEqual(
      [],
    );
    expect(issues({ driver: "duckdb", sqlitePath: "" })).toEqual([
      "sqlitePath: Choose a database file",
    ]);
    // Command mode names the driver's secret.
    expect(issues({ driver: "snowflake", user: "u", passwordMode: "command" })).toEqual([
      "driverParams.account: account is required",
      "passwordCommand: Enter the command that prints the password or token",
    ]);
  });

  it("read and write the driver params through the lane form", () => {
    const values = profileToForm(docs, ui);
    expect(values).toMatchObject({ driver: "mongodb", database: "audit", host: "db.example" });
    expect(values.driverParams).toEqual({ authSource: "audit" });
    const input = formToProfileInput({ ...values, database: "admin" }, docs, docs.id, ui);
    expect(input.profile).toMatchObject({
      driver: "mongodb",
      host: "db.example",
      port: 27018,
      user: null,
      database: null,
      options: { driverParams: { authSource: "admin" }, pgFlavor: null },
    });
  });

  it("keep the params typed after Driver ▸ another lane database of the family", () => {
    const libsql: ConnectionProfile = {
      ...docs,
      id: "p-libsql",
      driver: "libsql",
      options: { ...docs.options, driverParams: { authTokenHeader: "x" } },
    };
    // Driver ▸ D1 starts D1's form with empty params; what is typed there is D1's own.
    const values = {
      ...profileToForm(libsql),
      driver: "d1" as const,
      driverParams: { accountId: "abc" },
      database: "db",
    };
    expect(connectionFormSchema.safeParse(values).success).toBe(true);
    const input = formToProfileInput(values, libsql, libsql.id, null);
    expect(input.profile.driver).toBe("d1");
    expect(input.profile.options.driverParams).toEqual({ accountId: "abc" });
  });

  it("follow the descriptor for the common fields", () => {
    const duck = formToProfileInput(
      {
        ...DEFAULT_FORM,
        name: "d",
        driver: "duckdb",
        sqlitePath: "/data/app.duckdb",
        host: "ignored",
        sshEnabled: true,
        sshHost: "box",
        passwordMode: "command",
        passwordCommand: "op read x",
      },
      null,
      "id-1",
    );
    // A file, no SSH, and no Command mode.
    expect(duck.profile).toMatchObject({ host: null, sqlitePath: "/data/app.duckdb", ssh: null });
    expect(duck.profile.options.passwordCommand).toBeNull();
    expect(passwordModesFor("duckdb")).toEqual(["keychain", "ask", "none"]);
    expect(passwordModesFor("d1")).toEqual(["keychain", "ask", "command"]);
  });

  it("are summed up by the lane in the connection list, else like the core ones", () => {
    expect(connectionDetail(docs, ui)).toBe("db.example:27018");
    expect(connectionDetail(docs)).toBe("db.example : 27018");
    expect(connectionDetail({ ...docs, driver: "duckdb", sqlitePath: "/d.duckdb" })).toBe(
      "/d.duckdb",
    );
  });
});
