import { describe, expect, it } from "vitest";
import type { GridColumn } from "@/features/datagrid/types";
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
