import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import {
  csvDefaults,
  csvField,
  csvText,
  decimalClashes,
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

  it("copies with the Space quote: every value between spaces, a space inside doubled", () => {
    const space = { ...o, quote: "space" } as const;
    expect(csvField("ab", space)).toBe(" ab ");
    expect(csvField("a b", space)).toBe(" a  b ");
    expect(csvField("", space)).toBe("  ");
    expect(csvField(null, space)).toBe("");
    // Quotes and delimiters inside are plain text; only spaces are doubled.
    expect(csvField('say "hi", ok', space)).toBe(' say  "hi",  ok ');
    const rows = [[{ t: "int", v: 1 } as const, { t: "text", v: "a b" } as const]];
    expect(formatCsv(["id", "name"], rows, space)).toBe(" id , name \n 1 , a  b ");
    expect(formatCsv(null, rows, space)).toBe(" 1 , a  b ");
  });

  it("writes a custom decimal separator, replacing the first point of numbers only", () => {
    const hash = { ...o, delimiter: ";", decimal: "#" } as const;
    expect(csvText({ t: "decimal", v: "12.50" }, hash)).toBe("12#50");
    expect(csvText({ t: "float", v: 3.25 }, hash)).toBe("3#25");
    expect(csvText({ t: "float", v: 1.5e-7 }, hash)).toBe("1#5e-7");
    expect(csvText({ t: "text", v: "1.5" }, hash)).toBe("1.5");
    // `$` is a character like another one, not a replacement pattern.
    expect(csvText({ t: "decimal", v: "1.5" }, { ...hash, decimal: "$" })).toBe("1$5");
    expect(csvText({ t: "decimal", v: "1.5" }, { ...hash, decimal: "&" })).toBe("1&5");
  });

  it("reads a custom decimal separator from the preferences, unless it is the delimiter", () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ";", csvDecimal: "#", csvQuote: "space" });
    expect(csvDefaults()).toEqual({
      delimiter: ";",
      quote: "space",
      lineBreak: "\n",
      decimal: "#",
    });
    // The same character as the delimiter cannot be told apart: points stay.
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ";", csvDecimal: ";" });
    expect(csvDefaults().decimal).toBe(".");
    // `,` with a comma delimiter is the classic European choice: numbers get quoted.
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ",", csvDecimal: "," });
    expect(csvDefaults().decimal).toBe(",");
    expect(decimalClashes(";", ";")).toBe(true);
    expect(decimalClashes(",", ",")).toBe(false);
    expect(decimalClashes("#", ";")).toBe(false);
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
    // The Space quote of the CSV default options (and of the last import) starts the wizard too.
    expect(importQuote('"', "space")).toBe(" ");
    expect(importQuote(" ", "ifNeeded")).toBe(" ");
    expect(importQuote("'", "space")).toBe("'");
  });
});
