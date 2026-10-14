import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import {
  csvDefaults,
  csvField,
  csvText,
  DEFAULT_CSV_OPTIONS,
  formatCsv,
  importDelimiter,
  importQuote,
} from "./csv";

const o = DEFAULT_CSV_OPTIONS;

describe("CSV default options", () => {
  afterEach(() => resetPrefsForTests());

  it("reads the preferences (Settings ▸ General ▸ CSV default options)", () => {
    expect(csvDefaults()).toEqual(DEFAULT_CSV_OPTIONS);
    resetPrefsForTests({
      ...DEFAULT_PREFS,
      csvDelimiter: "\t",
      csvQuote: "single",
      csvLineBreak: "\r\n",
      csvDecimal: ",",
    });
    expect(csvDefaults()).toEqual({
      delimiter: "\t",
      quote: "single",
      lineBreak: "\r\n",
      decimal: ",",
    });
  });

  it("quotes only what needs it, or everything with the chosen quote", () => {
    expect(csvField("plain", o)).toBe("plain");
    expect(csvField('say "hi"', o)).toBe('"say ""hi"""');
    expect(csvField("a,b", o)).toBe('"a,b"');
    expect(csvField("a;b", o)).toBe("a;b");
    expect(csvField("a;b", { ...o, delimiter: ";" })).toBe('"a;b"');
    expect(csvField("two\nlines", o)).toBe('"two\nlines"');
    expect(csvField("\\.", o)).toBe('"\\."');
    expect(csvField(null, { ...o, quote: "double" })).toBe("");
    expect(csvField("x", { ...o, quote: "double" })).toBe('"x"');
    expect(csvField("it's", { ...o, quote: "single" })).toBe("'it''s'");
  });

  it("writes decimals with the chosen separator, other values as they are", () => {
    const comma = { ...o, decimal: "," } as const;
    expect(csvText({ t: "decimal", v: "12.50" }, comma)).toBe("12,50");
    expect(csvText({ t: "float", v: 0.5 }, comma)).toBe("0,5");
    expect(csvText({ t: "text", v: "1.5" }, comma)).toBe("1.5");
    expect(csvText({ t: "int", v: 3 }, comma)).toBe("3");
    expect(csvText({ t: "null" }, comma)).toBeNull();
  });

  it("joins fields and lines with the chosen delimiter and line break", () => {
    const rows = [[{ t: "int", v: 1 } as const, { t: "text", v: "a" } as const]];
    expect(formatCsv(["id", "name"], rows, { ...o, delimiter: "\t", lineBreak: "\r" })).toBe(
      "id\tname\r1\ta",
    );
    expect(formatCsv(null, [], o)).toBe("");
  });

  it("decides the CSV import's starting delimiter and quote", () => {
    // A delimiter that splits the file stays; one that does not is not trusted.
    expect(importDelimiter(",", 5, ";")).toBe(",");
    expect(importDelimiter(",", 1, ";")).toBe(";");
    expect(importDelimiter(",", 0, "\t")).toBe("\t");
    expect(importQuote('"', "ifNeeded")).toBe('"');
    expect(importQuote('"', "double")).toBe('"');
    expect(importQuote('"', "single")).toBe("'");
    expect(importQuote("'", "ifNeeded")).toBe("'");
  });
});
