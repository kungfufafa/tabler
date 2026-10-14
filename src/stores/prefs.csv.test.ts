import { describe, expect, it } from "vitest";
import { CSV_QUOTE_PREFS, DEFAULT_PREFS, isCsvDecimal, sanitizePrefs } from "./prefs";

describe("CSV default options in the preferences", () => {
  it("knows the Space quote", () => {
    expect(CSV_QUOTE_PREFS).toEqual(["double", "single", "space", "ifNeeded"]);
    expect(sanitizePrefs({ csvQuote: "space" }).csvQuote).toBe("space");
    expect(sanitizePrefs({ csvQuote: "tab" }).csvQuote).toBe(DEFAULT_PREFS.csvQuote);
  });

  it("accepts a custom decimal separator: one printable character, no digit or quote", () => {
    for (const ok of [".", ",", "#", ";", "-", " ", "x"])
      expect(isCsvDecimal(ok), JSON.stringify(ok)).toBe(true);
    for (const bad of ["", "..", "5", '"', "'", "\t", "\n", "é", 5, null, undefined])
      expect(isCsvDecimal(bad), JSON.stringify(bad)).toBe(false);
    expect(sanitizePrefs({ csvDecimal: "#" }).csvDecimal).toBe("#");
    expect(sanitizePrefs({ csvDecimal: "7" }).csvDecimal).toBe(DEFAULT_PREFS.csvDecimal);
    expect(sanitizePrefs({ csvDecimal: "ab" }).csvDecimal).toBe(DEFAULT_PREFS.csvDecimal);
  });
});
