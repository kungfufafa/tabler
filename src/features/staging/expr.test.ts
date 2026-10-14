import { describe, expect, it } from "vitest";
import type { StagedChange } from "@/lib/ipc";
import { displayValue } from "@/lib/values";
import {
  EXPR_TEXT,
  clockWithPrecision,
  exprOf,
  exprText,
  exprValue,
  parseServerExpr,
  withServerExprs,
} from "./expr";

describe("server expressions in staged cells", () => {
  it("round-trips an expression through its staged value, which reads as the keyword", () => {
    for (const [expr, text] of Object.entries(EXPR_TEXT)) {
      const v = exprValue(expr as keyof typeof EXPR_TEXT);
      expect(exprOf(v)).toBe(expr);
      expect(displayValue(v).text).toBe(text);
    }
    expect(displayValue(exprValue("now")).text).toBe("NOW()");
  });

  it("keeps a typed precision: it reads in the cell and survives the staged value", () => {
    for (const [expr, text] of [
      ["now(6)", "NOW(6)"],
      ["now(0)", "NOW(0)"],
      ["currentTimestamp(3)", "CURRENT_TIMESTAMP(3)"],
      ["currentTime(6)", "CURRENT_TIME(6)"],
      ["localTimestamp(1)", "LOCALTIMESTAMP(1)"],
    ] as const) {
      expect(exprText(expr)).toBe(text);
      const v = exprValue(expr);
      expect(exprOf(v), expr).toBe(expr);
      expect(displayValue(v).text).toBe(text);
    }
    // The precision-less ones are another expression: MySQL's CURRENT_TIMESTAMP has 0 digits.
    expect(exprOf(exprValue("currentTimestamp"))).toBe("currentTimestamp");
    expect(exprValue("currentTimestamp")).not.toEqual(exprValue("currentTimestamp(6)"));
  });

  it("never mistakes a fetched value for an expression", () => {
    expect(exprOf({ t: "other", v: { ty: "inet", text: "NOW()" } })).toBeNull();
    expect(exprOf({ t: "text", v: "NOW()" })).toBeNull();
    expect(exprOf({ t: "other", v: { ty: "\u0000expr", text: "DROP TABLE x" } })).toBeNull();
    // Only the text exprValue writes (not a spelling that merely parses).
    for (const text of ["now()", "NOW(7)", "NOW( 6 )", "CURRENT_TIMESTAMP()", "now(6)"])
      expect(exprOf({ t: "other", v: { ty: "\u0000expr", text } }), text).toBeNull();
    expect(exprOf(undefined)).toBeNull();
  });

  it("parses only the whitelisted keywords, in any case and spacing", () => {
    expect(parseServerExpr("NOW()")).toBe("now");
    expect(parseServerExpr(" now( ) ")).toBe("now");
    expect(parseServerExpr("CURRENT_TIMESTAMP")).toBe("currentTimestamp");
    expect(parseServerExpr("current_timestamp()")).toBe("currentTimestamp");
    expect(parseServerExpr("CURRENT_DATE")).toBe("currentDate");
    expect(parseServerExpr("current_time")).toBe("currentTime");
    for (const bad of [
      "NOW",
      "now() + interval '1 day'",
      "2024-01-01",
      "(select 1)",
      "current_date()",
      "current_date(3)",
      "now(-1)",
      "now(1.5)",
      "now(x)",
      "now(6",
      "",
    ])
      expect(parseServerExpr(bad), bad).toBeNull();
  });

  it("carries the fractional-seconds precision of NOW(n) and the other clocks", () => {
    expect(parseServerExpr("NOW(6)")).toBe("now(6)");
    expect(parseServerExpr(" now( 3 ) ")).toBe("now(3)");
    expect(parseServerExpr("now(0)")).toBe("now(0)");
    expect(parseServerExpr("CURRENT_TIMESTAMP(6)")).toBe("currentTimestamp(6)");
    expect(parseServerExpr("current_timestamp(3)")).toBe("currentTimestamp(3)");
    expect(parseServerExpr("current_time(2)")).toBe("currentTime(2)");
    expect(parseServerExpr("localtimestamp(0)")).toBe("localTimestamp(0)");
    expect(parseServerExpr("LOCALTIMESTAMP(06)")).toBe("localTimestamp(6)");
    // More than 6 digits is no expression: it stays text, which the server rejects as a date.
    for (const bad of ["now(7)", "now(10)", "current_timestamp(9)", "localtimestamp(99)"])
      expect(parseServerExpr(bad), bad).toBeNull();
    expect(clockWithPrecision("now", 6)).toBe("now(6)");
    expect(clockWithPrecision("now", 7)).toBeNull();
    expect(clockWithPrecision("currentTime", -1)).toBeNull();
    expect(clockWithPrecision("currentTime", 1.5)).toBeNull();
  });

  it("moves expression cells out of the values into the payload's exprs", () => {
    const update: StagedChange = {
      op: "update",
      rowId: "r",
      key: { kind: "rowid", value: 1 },
      changes: { name: { t: "text", v: "x" }, seen: exprValue("now") },
    };
    expect(withServerExprs(update)).toEqual({
      op: "update",
      rowId: "r",
      key: { kind: "rowid", value: 1 },
      changes: { name: { t: "text", v: "x" } },
      exprs: { seen: "now" },
    });
    const insert: StagedChange = {
      op: "insert",
      rowId: "n",
      values: { born: exprValue("currentDate"), id: { t: "default" } },
    };
    expect(withServerExprs(insert)).toEqual({
      op: "insert",
      rowId: "n",
      values: { id: { t: "default" } },
      exprs: { born: "currentDate" },
    });
    // A typed precision travels as the wire string the backend reads ("now(6)").
    const fsp: StagedChange = {
      op: "update",
      rowId: "f",
      key: { kind: "rowid", value: 1 },
      changes: { seen: exprValue("currentTimestamp(6)"), at: exprValue("now(3)") },
    };
    expect(withServerExprs(fsp)).toMatchObject({
      changes: {},
      exprs: { seen: "currentTimestamp(6)", at: "now(3)" },
    });
    // Nothing to move: the change is sent as it is (no `exprs` key at all).
    const plain: StagedChange = { op: "insert", rowId: "p", values: { a: { t: "int", v: 1 } } };
    expect(withServerExprs(plain)).toBe(plain);
    const del: StagedChange = { op: "delete", rowId: "d", key: { kind: "rowid", value: 2 } };
    expect(withServerExprs(del)).toBe(del);
  });
});
