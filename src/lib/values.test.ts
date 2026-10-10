import { describe, expect, it } from "vitest";
import { displayValue, pgArrayLiteral, valuesEqual, valueText } from "./values";

describe("values", () => {
  it("renders every kind on one line", () => {
    expect(displayValue({ t: "null" })).toEqual({ text: "NULL", cls: "null" });
    expect(displayValue({ t: "bigInt", v: "9007199254740993" }).text).toBe("9007199254740993");
    expect(
      displayValue({ t: "bytes", v: { base64: "AA==", len: 2048, truncated: false } }).text,
    ).toBe("<blob 2.0 KB>");
    expect(displayValue({ t: "json", v: '{\n  "a": 1\n}' }).text).toBe('{ "a": 1 }');
    expect(displayValue({ t: "text", v: "a\nb" }).text).toBe("a↵b");
    expect(displayValue({ t: "array", v: [{ t: "text", v: "x y" }, { t: "null" }] }).text).toBe(
      '{"x y",NULL}',
    );
  });
  it("compares structurally", () => {
    expect(valuesEqual({ t: "int", v: 1 }, { t: "int", v: 1 })).toBe(true);
    expect(valuesEqual({ t: "int", v: 1 }, { t: "bigInt", v: "1" })).toBe(false);
    expect(valuesEqual({ t: "null" }, { t: "null" })).toBe(true);
  });
  it("produces parameter text", () => {
    expect(valueText({ t: "bool", v: false })).toBe("false");
    expect(valueText({ t: "null" })).toBeNull();
    expect(
      pgArrayLiteral([
        { t: "text", v: 'a"b' },
        { t: "int", v: 2 },
      ]),
    ).toBe('{"a\\"b","2"}');
  });
});
