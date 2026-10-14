import { describe, expect, it } from "vitest";
import { detectLanguage, highlightRuns } from "./languages";

describe("value languages", () => {
  it("detects JSON and XML", () => {
    expect(detectLanguage("json", "jsonb", "")).toBe("json");
    expect(detectLanguage("other", "xml", "x")).toBe("xml");
    expect(detectLanguage("text", "text", '{"a": [1, 2]}')).toBe("json");
    expect(detectLanguage("text", "text", "<a href='x'>b</a>")).toBe("xml");
    expect(detectLanguage("text", "text", "{not json")).toBeNull();
    expect(detectLanguage("text", "text", "plain")).toBeNull();
  });

  it("colours JSON keys, strings, numbers and literals", () => {
    const runs = highlightRuns('{"a": "x", "n": 1.5, "b": true, "z": null}', "json");
    expect(runs.map((r) => r.text).join("")).toBe('{"a": "x", "n": 1.5, "b": true, "z": null}');
    const cls = (text: string) => runs.find((r) => r.text === text)?.className ?? "";
    expect(cls('"a"')).toContain("--syn-keyword");
    expect(cls('"x"')).toContain("--syn-string");
    expect(cls("1.5")).toContain("--syn-number");
    expect(cls("true")).toContain("--syn-number");
    expect(cls("null")).toContain("--syn-number");
  });

  it("colours XML tags, attributes and comments", () => {
    const text = '<?xml version="1.0"?><a id="1"><!-- c --><b>t &amp; u</b></a>';
    const runs = highlightRuns(text, "xml");
    expect(runs.map((r) => r.text).join("")).toBe(text);
    const cls = (t: string) => runs.find((r) => r.text === t)?.className ?? "";
    expect(cls("a")).toContain("font-semibold");
    expect(cls("id")).toContain("--syn-number");
    expect(cls('"1"')).toContain("--syn-string");
    expect(cls("<!-- c -->")).toContain("italic");
  });
});
