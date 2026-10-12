import { describe, expect, it } from "vitest";
import type { Value, ValueKind } from "@/lib/ipc";
import { choiceKeyOf, choicesFor, editorFor, NULL_CHOICE } from "./registry";

const col = (
  kind: ValueKind,
  extra: { enumValues?: string[] | null; nullable?: boolean } = {},
) => ({
  kind,
  enumValues: extra.enumValues ?? null,
  nullable: extra.nullable ?? true,
});
const NULL: Value = { t: "null" };

describe("editorFor", () => {
  it("picks an editor per kind", () => {
    expect(editorFor(col("text"), { t: "text", v: "a" })).toBe("text");
    expect(editorFor(col("int"), { t: "int", v: 1 })).toBe("text");
    expect(editorFor(col("bigInt"), NULL)).toBe("text");
    expect(editorFor(col("decimal"), NULL)).toBe("text");
    expect(editorFor(col("float"), NULL)).toBe("text");
    expect(editorFor(col("uuid"), NULL)).toBe("text");
    expect(editorFor(col("time"), NULL)).toBe("text");
    expect(editorFor(col("interval"), NULL)).toBe("text");
    expect(editorFor(col("array"), NULL)).toBe("text");
    expect(editorFor(col("other"), NULL)).toBe("text");
    expect(editorFor(col("bool"), { t: "bool", v: true })).toBe("choice");
    expect(editorFor(col("enum", { enumValues: ["a", "b"] }), NULL)).toBe("choice");
    expect(editorFor(col("enum"), { t: "enum", v: "x" })).toBe("text");
    expect(editorFor(col("json"), { t: "json", v: "{}" })).toBe("json");
    expect(editorFor(col("date"), NULL)).toBe("date");
    expect(editorFor(col("timestamp"), NULL)).toBe("date");
    expect(editorFor(col("timestampTz"), NULL)).toBe("date");
    expect(editorFor(col("bytes"), NULL)).toBeNull();
  });

  it("uses the multi-line editor whenever the text has a line break", () => {
    expect(editorFor(col("text"), { t: "text", v: "a\nb" })).toBe("multiline");
    expect(editorFor(col("text"), { t: "text", v: "a\r\nb" })).toBe("multiline");
    expect(editorFor(col("text"), { t: "text", v: "a\nb" }, "typed")).toBe("text");
    expect(editorFor(col("enum"), { t: "enum", v: "x\ny" })).toBe("multiline");
  });
});

describe("choices", () => {
  it("offers NULL only on nullable columns", () => {
    expect(choicesFor(col("bool")).map((c) => c.label)).toEqual(["true", "false", "NULL"]);
    expect(choicesFor(col("bool", { nullable: false })).map((c) => c.label)).toEqual([
      "true",
      "false",
    ]);
    expect(choicesFor(col("enum", { enumValues: ["x", "y"], nullable: false }))).toEqual([
      { key: "x", label: "x" },
      { key: "y", label: "y" },
    ]);
  });

  it("maps values to choice keys", () => {
    const choices = choicesFor(col("bool"));
    expect(choiceKeyOf({ t: "bool", v: false }, choices)).toBe("false");
    expect(choiceKeyOf(NULL, choices)).toBe(NULL_CHOICE);
    expect(choiceKeyOf({ t: "default" }, choices)).toBeNull();
    expect(choiceKeyOf(NULL, choicesFor(col("bool", { nullable: false })))).toBeNull();
  });
});
