import { describe, expect, it } from "vitest";
import { jsonError, reformatJson } from "./json";

describe("json editor helpers", () => {
  it("formats and minifies without touching numbers or strings", () => {
    const raw = '{"big": 12345678901234567890, "s":"a, b: {c}" ,"e":{}, "l":[1,[ ]], "q":"x\\"y"}';
    expect(reformatJson(raw, 2)).toBe(
      [
        "{",
        '  "big": 12345678901234567890,',
        '  "s": "a, b: {c}",',
        '  "e": {},',
        '  "l": [',
        "    1,",
        "    []",
        "  ],",
        '  "q": "x\\"y"',
        "}",
      ].join("\n"),
    );
    expect(reformatJson(reformatJson(raw, 2), null)).toBe(
      '{"big":12345678901234567890,"s":"a, b: {c}","e":{},"l":[1,[]],"q":"x\\"y"}',
    );
    expect(reformatJson("  42 ", 2)).toBe("42");
  });

  it("reports errors with a position", () => {
    expect(jsonError('{"a": 1}')).toBeNull();
    expect(jsonError("   ")).toBe("Enter a JSON value");
    const err = jsonError('{\n  "a": 1,\n}');
    expect(err).toBeTruthy();
    expect(err).toMatch(/line \d+/);
  });
});
