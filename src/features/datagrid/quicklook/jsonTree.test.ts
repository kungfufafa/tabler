import { describe, expect, it } from "vitest";
import { nodeSummary, parseJsonTree } from "./jsonTree";

describe("parseJsonTree", () => {
  it("keeps numbers as written, key order and duplicate keys", () => {
    const tree = parseJsonTree(
      '{"b": 9007199254740993, "a": [1.10, true, null, "x\\ny"], "b": -2e3}',
    );
    expect(tree).toEqual({
      type: "object",
      entries: [
        { key: "b", value: { type: "number", raw: "9007199254740993" } },
        {
          key: "a",
          value: {
            type: "array",
            items: [
              { type: "number", raw: "1.10" },
              { type: "boolean", value: true },
              { type: "null" },
              { type: "string", value: "x\ny" },
            ],
          },
        },
        { key: "b", value: { type: "number", raw: "-2e3" } },
      ],
    });
  });

  it("reads scalars and empty containers", () => {
    expect(parseJsonTree(' "a\\"b" ')).toEqual({ type: "string", value: 'a"b' });
    expect(parseJsonTree("[]")).toEqual({ type: "array", items: [] });
    expect(parseJsonTree("{ }")).toEqual({ type: "object", entries: [] });
    expect(parseJsonTree("0")).toEqual({ type: "number", raw: "0" });
  });

  it("throws on invalid JSON", () => {
    for (const bad of ["", "{", "[1,]", '{"a" 1}', "tru", "1 2", "-", '"open'])
      expect(() => parseJsonTree(bad), bad).toThrow(SyntaxError);
  });

  it("summarizes nodes", () => {
    expect(nodeSummary(parseJsonTree('{"a":1}'))).toBe("{1 key}");
    expect(nodeSummary(parseJsonTree("[1,2]"))).toBe("[2 items]");
    expect(nodeSummary(parseJsonTree('"hi"'))).toBe('"hi"');
    expect(nodeSummary(parseJsonTree("1.50"))).toBe("1.50");
  });
});
