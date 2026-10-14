import { describe, expect, it } from "vitest";
import { joinSql } from "./joinSql";

describe("Generate JOIN", () => {
  it("joins the table holding the key to the one it references, per dialect", () => {
    const fk = {
      table: { schema: "public", name: "posts" },
      column: "user_id",
      references: { schema: "public", name: "users", column: "id" },
    };
    expect(joinSql("postgres", fk)).toBe(
      'SELECT *\nFROM "public"."posts"\nJOIN "public"."users" ON "posts"."user_id" = "users"."id"\n',
    );
    expect(
      joinSql("mysql", {
        table: { schema: null, name: "posts" },
        column: "user_id",
        references: { schema: "other", name: "users", column: "id" },
      }),
    ).toBe("SELECT *\nFROM `posts`\nJOIN `other`.`users` ON `posts`.`user_id` = `users`.`id`\n");
    expect(
      joinSql("sqlite", {
        table: { schema: "main", name: "a" },
        column: "b_id",
        references: { schema: "main", name: "b", column: "id" },
      }),
    ).toContain('JOIN "main"."b" ON "a"."b_id" = "b"."id"');
  });

  it("aliases the referenced side of a self reference", () => {
    expect(
      joinSql("postgres", {
        table: { schema: "public", name: "employees" },
        column: "manager_id",
        references: { schema: "public", name: "employees", column: "id" },
      }),
    ).toBe(
      'SELECT *\nFROM "public"."employees"\nJOIN "public"."employees" AS "employees_ref" ON "employees"."manager_id" = "employees_ref"."id"\n',
    );
  });

  it("quotes names that need it", () => {
    expect(
      joinSql("postgres", {
        table: { schema: null, name: 'we"ird' },
        column: "my col",
        references: { schema: null, name: "T", column: "Id" },
      }),
    ).toBe('SELECT *\nFROM "we""ird"\nJOIN "T" ON "we""ird"."my col" = "T"."Id"\n');
  });
});
