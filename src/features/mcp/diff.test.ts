import { describe, expect, it } from "vitest";
import { diffLines } from "./diff";

describe("diffLines", () => {
  it("shows a new file as added lines", () => {
    expect(diffLines("", "{\n}\n")).toEqual([
      { kind: "add", text: "{" },
      { kind: "add", text: "}" },
    ]);
  });

  it("marks inserted lines between unchanged ones", () => {
    const before = '{\n  "a": 1\n}\n';
    const after = '{\n  "a": 1,\n  "b": 2\n}\n';
    expect(diffLines(before, after)).toEqual([
      { kind: "same", text: "{" },
      { kind: "del", text: '  "a": 1' },
      { kind: "add", text: '  "a": 1,' },
      { kind: "add", text: '  "b": 2' },
      { kind: "same", text: "}" },
    ]);
  });

  it("folds unchanged lines far from a change", () => {
    const before = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    const after = before.replace("line 10", "LINE 10");
    const out = diffLines(before, after, 2);
    expect(out[0]).toEqual({ kind: "skip", count: 8 });
    expect(out.filter((l) => l.kind === "same")).toHaveLength(4);
    expect(out.at(-1)).toEqual({ kind: "skip", count: 7 });
    expect(out).toContainEqual({ kind: "del", text: "line 10" });
    expect(out).toContainEqual({ kind: "add", text: "LINE 10" });
  });

  it("treats CRLF like LF and reports no change for equal text", () => {
    expect(diffLines("a\r\nb\r\n", "a\nb\n")).toEqual([{ kind: "skip", count: 2 }]);
    expect(diffLines("same", "same", 0)).toEqual([{ kind: "skip", count: 1 }]);
  });
});
