import { describe, expect, it } from "vitest";
import type { UserState } from "@/lib/ipc";
import {
  draftOf,
  draftProblems,
  dropChange,
  isDirty,
  MYSQL_DB_PRIVILEGES,
  newDraft,
  newUserState,
  PG_TABLE_PRIVILEGES,
  privilegeDatabases,
  setPrivilege,
  setPrivileges,
  setTablePrivilege,
  tablePrivilegeState,
  toChange,
  userKey,
  userLabel,
} from "./users";

const role: UserState = {
  ...newUserState("postgres"),
  name: "app",
  tablePrivileges: [{ object: "public", privileges: ["SELECT"] }],
  partialTablePrivileges: [{ object: "public", privileges: ["INSERT"] }],
};

describe("user drafts", () => {
  it("names accounts", () => {
    expect(userKey({ name: "app", host: "%" })).toBe("app@%");
    expect(userLabel({ name: "app", host: "%" })).toBe("'app'@'%'");
    expect(userLabel({ name: "app", host: null })).toBe("app");
  });

  it("starts new accounts per driver", () => {
    expect(newUserState("mysql")).toMatchObject({ host: "%", attributes: null });
    expect(newUserState("mysql").limits).toEqual({
      maxQueriesPerHour: 0,
      maxUpdatesPerHour: 0,
      maxConnectionsPerHour: 0,
      maxUserConnections: 0,
    });
    expect(newUserState("postgres")).toMatchObject({ host: null, limits: null });
    expect(newUserState("postgres").attributes).toMatchObject({ login: true, connectionLimit: -1 });
  });

  it("toggles privileges in canonical order and drops empty entries", () => {
    let list = setPrivilege([], "shop", "UPDATE", true, MYSQL_DB_PRIVILEGES);
    list = setPrivilege(list, "shop", "SELECT", true, MYSQL_DB_PRIVILEGES);
    list = setPrivilege(list, "app", "SELECT", true, MYSQL_DB_PRIVILEGES);
    expect(list).toEqual([
      { object: "app", privileges: ["SELECT"] },
      { object: "shop", privileges: ["SELECT", "UPDATE"] },
    ]);
    list = setPrivileges(list, "app", [], MYSQL_DB_PRIVILEGES);
    expect(list.map((p) => p.object)).toEqual(["shop"]);
  });

  it("moves table privileges between all, some and none", () => {
    expect(tablePrivilegeState(role, "public", "SELECT")).toBe("all");
    expect(tablePrivilegeState(role, "public", "INSERT")).toBe("some");
    expect(tablePrivilegeState(role, "public", "DELETE")).toBe("none");
    const all = setTablePrivilege(role, "public", "INSERT", "all");
    expect(tablePrivilegeState(all, "public", "INSERT")).toBe("all");
    expect(all.partialTablePrivileges).toEqual([]);
    const none = setTablePrivilege(role, "public", "SELECT", "none");
    expect(tablePrivilegeState(none, "public", "SELECT")).toBe("none");
    expect(PG_TABLE_PRIVILEGES).toHaveLength(4);
  });

  it("knows when there is something to apply", () => {
    const draft = draftOf(role);
    expect(isDirty(draft)).toBe(false);
    // Order and empty entries do not count as changes.
    const reordered = {
      ...role,
      schemaPrivileges: [{ object: "x", privileges: [] }],
      tablePrivileges: [{ object: "public", privileges: ["SELECT"] }],
    };
    expect(isDirty({ ...draft, state: reordered })).toBe(false);
    expect(isDirty({ ...draft, password: "secret" })).toBe(true);
    expect(isDirty({ ...draft, state: { ...role, name: "app2" } })).toBe(true);
    expect(isDirty(newDraft("postgres"))).toBe(true);
  });

  it("validates and builds the change", () => {
    const mysql = newDraft("mysql");
    expect(draftProblems(mysql, "mysql")).toEqual(["Enter a user name"]);
    const named = {
      ...mysql,
      state: { ...mysql.state, name: " app ", host: " " },
      password: "pw",
    };
    expect(draftProblems(named, "mysql")).toEqual([]);
    expect(toChange(named)).toEqual({
      original: null,
      edited: { ...mysql.state, name: "app", host: "%" },
      password: "pw",
    });
    const bad = {
      ...draftOf(role),
      state: { ...role, attributes: { ...role.attributes!, connectionLimit: -3 } },
    };
    expect(draftProblems(bad, "postgres")).toEqual([
      "Connection limit is a whole number, -1 = no limit",
    ]);
    expect(dropChange(role)).toEqual({ original: role, edited: null, password: null });
    const limits = {
      ...mysql,
      state: {
        ...mysql.state,
        name: "a",
        limits: { ...mysql.state.limits!, maxUserConnections: 1.5 },
      },
    };
    expect(draftProblems(limits, "mysql")).toEqual([
      "Resource limits are whole numbers, 0 = unlimited",
    ]);
  });

  it("lists the server's databases plus any with grants", () => {
    const state = { ...role, databasePrivileges: [{ object: "old_%", privileges: ["CONNECT"] }] };
    expect(privilegeDatabases(["shop", "app"], state)).toEqual(["app", "old_%", "shop"]);
  });
});
