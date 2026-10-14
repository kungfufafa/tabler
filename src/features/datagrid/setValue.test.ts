import { describe, expect, it } from "vitest";
import { exprOf } from "@/features/staging/expr";
import { emptyBecomesNull, nowValue } from "./setValue";

describe("nowValue", () => {
  it("stages the server's NOW() for each date / time kind", () => {
    for (const kind of ["date", "time", "timestamp", "timestampTz"] as const) {
      const v = nowValue(kind);
      expect(exprOf(v ?? { t: "null" }), kind).toBe("now");
      // It reads as the keyword in the grid.
      expect(v?.t === "other" && v.v.text).toBe("NOW()");
    }
  });
  it("is null for other kinds", () => {
    expect(nowValue("text")).toBeNull();
    expect(nowValue("int")).toBeNull();
  });
});

describe("emptyBecomesNull", () => {
  const col = (kind: Parameters<typeof emptyBecomesNull>[0]["kind"], nullable = true) => ({
    kind,
    nullable,
  });
  it("converts empty numeric, UUID, JSON and array input in nullable columns", () => {
    for (const kind of ["int", "bigInt", "decimal", "float", "uuid", "json", "array"] as const) {
      expect(emptyBecomesNull(col(kind), "", true), kind).toBe(true);
      expect(emptyBecomesNull(col(kind), "  ", true), kind).toBe(true);
    }
  });
  it("leaves text kinds, NOT NULL columns, the text NULL and a disabled preference alone", () => {
    expect(emptyBecomesNull(col("text"), "", true)).toBe(false);
    expect(emptyBecomesNull(col("date"), "", true)).toBe(false);
    expect(emptyBecomesNull(col("int", false), "", true)).toBe(false);
    expect(emptyBecomesNull(col("int"), "NULL", true)).toBe(false);
    expect(emptyBecomesNull(col("int"), "", false)).toBe(false);
  });
});
