import { describe, expect, it } from "vitest";
import { constantDefault, defaultKind, defaultPresets } from "./defaults";

describe("column_default presets", () => {
  it("MySQL: EMPTY, NULL, AUTO_INCREMENT, CURRENT_TIMESTAMP, NOW(), Advanced…", () => {
    expect(defaultPresets("mysql", "varchar(20)").map((p) => p.label)).toEqual([
      "EMPTY",
      "NULL",
      "AUTO_INCREMENT",
      "CURRENT_TIMESTAMP",
      "NOW()",
      "Advanced…",
    ]);
    const [empty, nul, ai] = defaultPresets("mysql", "int");
    expect(empty).toMatchObject({ kind: "set", expr: "''" });
    expect(nul).toMatchObject({ kind: "set", expr: null });
    expect(ai).toMatchObject({ kind: "autoIncrement" });
  });

  it("PostgreSQL: the kinds Sequence, Expression, Constant (with type-specific shortcuts)", () => {
    expect(defaultPresets("postgres", "text").map((p) => p.kind)).toEqual([
      "autoIncrement",
      "expression",
      "constant",
      "set",
    ]);
    expect(defaultPresets("postgres", "text")[0]?.label).toBe("Sequence");
    expect(defaultPresets("postgres", "timestamptz").map((p) => p.label)).toContain("now()");
    expect(defaultPresets("postgres", "uuid").map((p) => p.label)).toContain("gen_random_uuid()");
  });

  it("quotes constants unless they are numbers / booleans for such columns", () => {
    expect(constantDefault("postgres", "integer", " 42 ")).toBe("42");
    expect(constantDefault("postgres", "text", "42")).toBe("'42'");
    expect(constantDefault("postgres", "text", "it's")).toBe("'it''s'");
    expect(constantDefault("mysql", "varchar(10)", "it's")).toBe("'it\\'s'");
    expect(constantDefault("postgres", "boolean", "TRUE")).toBe("true");
    expect(constantDefault("postgres", "numeric(10,2)", "1.5")).toBe("1.5");
    expect(constantDefault("postgres", "text", "null")).toBe("NULL");
  });

  it("classifies PostgreSQL defaults", () => {
    expect(defaultKind("nextval('t_id_seq'::regclass)")).toBe("sequence");
    expect(defaultKind("'viewer'::user_role")).toBe("constant");
    expect(defaultKind("0")).toBe("constant");
    expect(defaultKind("now()")).toBe("expression");
    expect(defaultKind(null)).toBeNull();
  });
});
