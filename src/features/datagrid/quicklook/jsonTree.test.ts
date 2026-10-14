import { describe, expect, it } from "vitest";
import { nodeSummary, parseJsonTree, sortJsonKeys } from "./jsonTree";

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

describe("sortJsonKeys", () => {
  it("orders object entries by key, recursively; arrays keep their order", () => {
    // b then a (a holding d then c): a (c, d) then b.
    expect(sortJsonKeys('{"b":1,"a":{"d":2,"c":3}}')).toBe('{"a":{"c":3,"d":2},"b":1}');
    expect(sortJsonKeys('{"z":[{"y":1,"x":2},3,"a",{"q":null,"p":[{"n":1,"m":2}]}],"a":0}')).toBe(
      '{"a":0,"z":[{"x":2,"y":1},3,"a",{"p":[{"m":2,"n":1}],"q":null}]}',
    );
    expect(sortJsonKeys("[3,1,2]")).toBe("[3,1,2]");
    // The tree of the sorted text lists the keys in order.
    const tree = parseJsonTree(sortJsonKeys('{"b":1,"a":2}'));
    expect(tree.type === "object" && tree.entries.map((e) => e.key)).toEqual(["a", "b"]);
  });

  it("keeps duplicate keys (stable), big numbers, escapes and trailing zeros verbatim", () => {
    const text =
      '{"b": 9007199254740993, "a": 1.10, "b": -2e3, "a": "\\u00e9", "c": 12345678901234567890}';
    expect(sortJsonKeys(text)).toBe(
      '{"a": 1.10, "a": "\\u00e9", "b": 9007199254740993, "b": -2e3, "c": 12345678901234567890}',
    );
  });

  it("keeps the layout: the whitespace between entries stays where it is", () => {
    const text = '{\n  "b": [\n    1,\n    2\n  ],\n  "a": {\n    "d": 4,\n    "c": 3\n  }\n}\n';
    expect(sortJsonKeys(text)).toBe(
      '{\n  "a": {\n    "c": 3,\n    "d": 4\n  },\n  "b": [\n    1,\n    2\n  ]\n}\n',
    );
    expect(sortJsonKeys('  { "b" : 1 , "a" : 2 }  ')).toBe('  { "a" : 2 , "b" : 1 }  ');
  });

  it("compares by code unit and leaves scalars, empty containers and sorted text alone", () => {
    expect(sortJsonKeys('{"b":1,"B":2,"a":3,"é":4,"Z":5}')).toBe('{"B":2,"Z":5,"a":3,"b":1,"é":4}');
    for (const same of ['"x"', "12", "true", "null", "{}", "[]", '{ "a": {}, "b": [ ] }'])
      expect(sortJsonKeys(same)).toBe(same);
    // Keys that look alike after decoding sort by their decoded form.
    expect(sortJsonKeys('{"\\u0062":1,"a":2}')).toBe('{"a":2,"\\u0062":1}');
  });

  it("throws on invalid JSON like the parser", () => {
    expect(() => sortJsonKeys('{"a": ')).toThrow(SyntaxError);
  });
});
