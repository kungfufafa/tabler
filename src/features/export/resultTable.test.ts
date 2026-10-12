import { describe, expect, it } from "vitest";
import type { ResultColumn } from "@/lib/ipc";
import { resultTable } from "./resultTable";

const col = (name: string, table: string | null): ResultColumn => ({
  name,
  dataType: "int",
  kind: "int",
  nullable: true,
  table,
});

describe("resultTable", () => {
  it("names the table the result was selected from", () => {
    const cols = [col("id", "users"), col("email", "users")];
    expect(resultTable(cols, "select id, email from users")).toBe("users");
    expect(resultTable(cols, "SELECT id, email\nFROM `tabler_dev`.`users` WHERE id > 1")).toBe(
      "users",
    );
    expect(resultTable(cols, "select * from posts p join users on users.id = p.user_id")).toBe(
      "users",
    );
  });

  it("refuses MySQL table aliases and derived tables (the server reports those names)", () => {
    expect(
      resultTable([col("id", "u"), col("email", "u")], "SELECT u.id, u.email FROM users u"),
    ).toBe(null);
    expect(resultTable([col("id", "u")], "SELECT u.id FROM users AS u")).toBeNull();
    expect(
      resultTable([col("id", "t")], "SELECT t.id FROM (SELECT id FROM users) AS t"),
    ).toBeNull();
  });

  it("needs every column from that one table", () => {
    expect(
      resultTable([col("id", "users"), col("title", "posts")], "select * from users join posts"),
    ).toBeNull();
    // A computed column has no table to be inserted into.
    expect(
      resultTable([col("id", "users"), col("n", null)], "select id, 1 + 1 as n from users"),
    ).toBeNull();
    expect(resultTable([col("id", null)], "select 1")).toBeNull();
    expect(resultTable([], "select 1")).toBeNull();
    expect(resultTable([col("id", "users")], null)).toBeNull();
  });

  it("ignores names in comments and strings", () => {
    expect(resultTable([col("id", "u")], "select u.id from users u -- from u")).toBeNull();
    expect(resultTable([col("id", "u")], "select u.id /* from u */ from users u")).toBeNull();
    expect(
      resultTable([col("id", "u")], "select u.id from users u where u.note = 'from u'"),
    ).toBeNull();
  });
});
