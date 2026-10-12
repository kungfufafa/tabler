import { describe, expect, it } from "vitest";
import { statementTexts } from "@/features/editor/scanner";
import { newObjectTemplate, supportsNewObject } from "./templates";

describe("new object templates", () => {
  it("selects the placeholder name", () => {
    for (const dialect of ["postgres", "mysql", "sqlite"] as const)
      for (const kind of ["view", "materializedView", "function", "procedure"] as const) {
        if (!supportsNewObject(dialect, kind)) continue;
        const t = newObjectTemplate(dialect, kind, dialect === "mysql" ? "shop" : "public");
        expect(t.sql.slice(t.from, t.to)).toMatch(/^new_[a-z_]+$/);
        expect(t.title).toMatch(/^New /);
      }
  });

  it("qualifies with the schema where the dialect needs it", () => {
    expect(newObjectTemplate("postgres", "view", "app data").sql).toMatch(
      /^CREATE VIEW "app data"\.new_view AS/,
    );
    expect(newObjectTemplate("mysql", "view", "shop").sql).toMatch(/^CREATE VIEW new_view AS/);
    expect(newObjectTemplate("sqlite", "view", "main").sql).toMatch(/^CREATE VIEW new_view AS/);
    expect(newObjectTemplate("sqlite", "view", "aux").sql).toMatch(/^CREATE VIEW "aux"\.new_view/);
  });

  it("offers materialized views on PostgreSQL and routines on PG / MySQL only", () => {
    expect(supportsNewObject("mysql", "materializedView")).toBe(false);
    expect(supportsNewObject("postgres", "materializedView")).toBe(true);
    expect(supportsNewObject("sqlite", "function")).toBe(false);
    expect(supportsNewObject("mysql", "procedure")).toBe(true);
  });

  it("keeps a routine body in one statement for the editor", () => {
    const fn = newObjectTemplate("mysql", "function", "shop").sql;
    expect(statementTexts(fn, "mysql")).toHaveLength(1);
    const pg = newObjectTemplate("postgres", "function", "public").sql;
    expect(statementTexts(pg, "postgres")).toHaveLength(1);
    // What opening a MySQL routine shows: DROP … IF EXISTS, then the CREATE with its body.
    const opened =
      "DROP PROCEDURE IF EXISTS `archive`;\n\nCREATE DEFINER=`root`@`localhost` PROCEDURE `archive`(IN days INT)\nBEGIN\n  DELETE FROM posts WHERE created_at < NOW() - INTERVAL days DAY;\n  IF days > 30 THEN SELECT 'old'; END IF;\nEND;\n";
    expect(statementTexts(opened, "mysql")).toHaveLength(2);
  });
});
