import { afterEach, describe, expect, it } from "vitest";
import { parseCellText } from "@/features/datagrid/cellText";
import type { GridColumn } from "@/features/datagrid/types";
import type { Value } from "@/lib/ipc";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import {
  clearedPlaceholder,
  fieldText,
  isChoiceColumn,
  matchesField,
  parseFieldText,
  placeholderOf,
  viewText,
} from "./fields";

const col = (kind: GridColumn["kind"], extra: Partial<GridColumn> = {}): GridColumn => ({
  id: "c",
  name: "c",
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
  ...extra,
});

describe("Details field helpers", () => {
  it("starts boxes from the editor text, JSON re-indented by whitespace only", () => {
    expect(fieldText({ t: "int", v: 5 })).toBe("5");
    expect(fieldText({ t: "null" })).toBe("");
    expect(fieldText({ t: "default" })).toBe("");
    expect(fieldText({ t: "bool", v: false })).toBe("false");
    expect(fieldText({ t: "json", v: '{"n":9007199254740993,"p":1.10}' })).toBe(
      '{\n  "n": 9007199254740993,\n  "p": 1.10\n}',
    );
    // Invalid JSON is shown verbatim.
    expect(fieldText({ t: "json", v: "{oops" })).toBe("{oops");
    expect(viewText({ t: "default" })).toBe("DEFAULT");
  });

  it("names what an empty box holds", () => {
    expect(placeholderOf({ t: "null" })).toBe("NULL");
    expect(placeholderOf({ t: "default" })).toBe("DEFAULT");
    expect(placeholderOf({ t: "text", v: "" })).toBe("EMPTY");
    expect(placeholderOf({ t: "text", v: "x" })).toBeUndefined();
    expect(clearedPlaceholder(col("text"))).toBe("EMPTY");
    expect(clearedPlaceholder(col("int"))).toBe("NULL");
    expect(clearedPlaceholder(col("int", { nullable: false }))).toBeUndefined();
  });

  it("parses field text with the inline editor's rules", () => {
    expect(parseFieldText(col("int"), "", { t: "int", v: 1 })).toEqual({
      ok: true,
      value: { t: "null" },
    });
    expect(parseFieldText(col("int", { nullable: false }), "", { t: "int", v: 1 }).ok).toBe(false);
    expect(parseFieldText(col("text"), "", { t: "text", v: "a" })).toEqual({
      ok: true,
      value: { t: "text", v: "" },
    });
    expect(parseFieldText(col("int"), "12345678901234567890", { t: "null" })).toEqual({
      ok: true,
      value: { t: "bigInt", v: "12345678901234567890" },
    });
    const mood = col("enum", { enumValues: ["happy", "sad"] });
    expect(parseFieldText(mood, "sad", { t: "null" })).toEqual({
      ok: true,
      value: { t: "enum", v: "sad" },
    });
    expect(parseFieldText(mood, "meh", { t: "null" })).toEqual({
      ok: false,
      error: "Pick one of: happy, sad",
    });
  });

  it("knows picker columns and matches names", () => {
    expect(isChoiceColumn(col("bool"))).toBe(true);
    expect(isChoiceColumn(col("enum", { enumValues: ["a"] }))).toBe(true);
    expect(isChoiceColumn(col("enum", { enumValues: [] }))).toBe(false);
    expect(isChoiceColumn(col("text"))).toBe(false);
    expect(matchesField({ name: "created_at" }, " AT ")).toBe(true);
    expect(matchesField({ name: "created_at" }, "x")).toBe(false);
    expect(matchesField({ name: "created_at" }, "")).toBe(true);
  });
});

describe("parseFieldText ▸ Auto convert empty Numeric, UUID, JSON or Array to NULL", () => {
  afterEach(() => resetPrefsForTests());
  const off = () => resetPrefsForTests({ ...DEFAULT_PREFS, gridAutoNullEmpty: false });
  const AUTO = ["int", "bigInt", "decimal", "float", "uuid", "json", "array"] as const;

  it("is on by default: an emptied nullable field of those kinds is NULL", () => {
    for (const kind of AUTO) {
      expect(parseFieldText(col(kind), "", { t: "text", v: "x" }), kind).toEqual({
        ok: true,
        value: { t: "null" },
      });
      // A blank (spaces) box too; the literal text NULL is a value, not a NULL.
      expect(parseFieldText(col(kind), "  ", { t: "text", v: "x" }), kind).toEqual({
        ok: true,
        value: { t: "null" },
      });
    }
    expect(parseFieldText(col("int"), "NULL", { t: "null" }).ok).toBe(false);
    expect(parseFieldText(col("text"), "NULL", { t: "null" })).toEqual({
      ok: true,
      value: { t: "text", v: "NULL" },
    });
  });

  it("is honoured: with it off an emptied field raises the grid editor's own error", () => {
    off();
    for (const kind of AUTO) {
      const previous: Value = { t: "text", v: "x" };
      const field = parseFieldText(col(kind), "", previous);
      // Exactly what the grid's inline editor makes of the same empty text.
      expect(field, kind).toEqual(parseCellText(col(kind), "", previous));
      expect(field.ok && field.value.t === "null", kind).toBe(false);
    }
    expect(parseFieldText(col("int"), "", { t: "int", v: 1 })).toEqual({
      ok: false,
      error: "Enter a whole number",
    });
    expect(parseFieldText(col("bigInt"), "", { t: "int", v: 1 })).toEqual({
      ok: false,
      error: "Enter a whole number",
    });
    expect(parseFieldText(col("decimal"), "", { t: "int", v: 1 })).toEqual({
      ok: false,
      error: "Enter a number",
    });
    expect(parseFieldText(col("uuid"), "", { t: "null" })).toEqual({
      ok: false,
      error: "Enter a UUID",
    });
    const json = parseFieldText(col("json"), "", { t: "json", v: "{}" });
    expect(json.ok).toBe(false);
    expect(!json.ok && json.error).toMatch(/^Invalid JSON/);
    // The array parser takes any text (a literal the server checks), the empty one included.
    expect(parseFieldText(col("array"), "", { t: "null" })).toEqual({
      ok: true,
      value: { t: "other", v: { ty: "array", text: "" } },
    });
    // Typing a value works as before.
    expect(parseFieldText(col("int"), "7", { t: "null" })).toEqual({
      ok: true,
      value: { t: "int", v: 7 },
    });
  });

  it("keeps a NOT NULL column's own error and the kinds the preference does not cover", () => {
    for (const on of [true, false]) {
      if (!on) off();
      expect(parseFieldText(col("int", { nullable: false }), "", { t: "int", v: 1 })).toEqual({
        ok: false,
        error: "Enter a whole number",
      });
      // Dates, times, booleans, binary and the like: an emptied nullable field is NULL either way.
      for (const kind of ["date", "time", "timestamp", "bool", "bytes", "bit"] as const)
        expect(parseFieldText(col(kind), "", { t: "text", v: "x" }), `${kind} ${on}`).toEqual({
          ok: true,
          value: { t: "null" },
        });
      // Text-like columns keep the empty string.
      expect(parseFieldText(col("text"), "", { t: "text", v: "a" })).toEqual({
        ok: true,
        value: { t: "text", v: "" },
      });
    }
  });

  it("promises NULL for a cleared box only while it would be NULL", () => {
    expect(clearedPlaceholder(col("int"))).toBe("NULL");
    expect(clearedPlaceholder(col("uuid"))).toBe("NULL");
    off();
    for (const kind of AUTO) expect(clearedPlaceholder(col(kind)), kind).toBeUndefined();
    // Not covered by the preference: still NULL; text-like: EMPTY; NOT NULL: nothing.
    expect(clearedPlaceholder(col("date"))).toBe("NULL");
    expect(clearedPlaceholder(col("text"))).toBe("EMPTY");
    expect(clearedPlaceholder(col("int", { nullable: false }))).toBeUndefined();
  });
});
