import { describe, expect, it } from "vitest";
import {
  colorDistance,
  colorToHex,
  colorToOpaqueHex,
  composite,
  contrastRatio,
  isThemeColor,
  luminance,
  parseColor,
  toHex,
} from "./color";

describe("parseColor", () => {
  it("reads #rgb, #rgba, #rrggbb and #rrggbbaa", () => {
    expect(parseColor("#fff")).toEqual({ r: 1, g: 1, b: 1, a: 1 });
    expect(parseColor("#000f")).toEqual({ r: 0, g: 0, b: 0, a: 1 });
    expect(parseColor("#ff8000")?.g).toBeCloseTo(128 / 255, 5);
    expect(parseColor("#11223344")).toEqual({ r: 17 / 255, g: 34 / 255, b: 51 / 255, a: 68 / 255 });
    expect(parseColor("  #ABCDEF ")).not.toBeNull();
  });

  it("reads oklch() with numbers, percentages and an alpha", () => {
    expect(toHex(parseColor("oklch(1 0 0)")!)).toBe("#ffffff");
    expect(toHex(parseColor("oklch(0 0 0)")!)).toBe("#000000");
    expect(toHex(parseColor("oklch(100% 0 0)")!)).toBe("#ffffff");
    // sRGB red in OKLCH.
    expect(toHex(parseColor("oklch(0.62796 0.25768 29.234)")!)).toBe("#ff0000");
    expect(parseColor("oklch(0.5 0.1 120deg / 0.4)")?.a).toBeCloseTo(0.4, 5);
    expect(parseColor("oklch(0.5 0.1 120 / 40%)")?.a).toBeCloseTo(0.4, 5);
  });

  it("maps an out-of-gamut oklch() into sRGB instead of clipping channels apart", () => {
    const c = parseColor("oklch(0.7 0.4 150)")!;
    for (const ch of [c.r, c.g, c.b]) {
      expect(ch).toBeGreaterThanOrEqual(0);
      expect(ch).toBeLessThanOrEqual(1);
    }
  });

  it.each([
    "",
    "red",
    "#12",
    "#12345",
    "#1234567",
    "#gggggg",
    "rgb(1,2,3)",
    "oklch(0.5 0.1)",
    "oklch(0.5 0.1 120 / )",
    "url(javascript:alert(1))",
    "#fff; background: red",
    `#${"a".repeat(80)}`,
  ])("rejects %j", (value) => {
    expect(parseColor(value)).toBeNull();
    expect(isThemeColor(value)).toBe(false);
  });

  it("isThemeColor is false for non-strings", () => {
    expect(isThemeColor(12)).toBe(false);
    expect(isThemeColor(null)).toBe(false);
  });
});

describe("hex output", () => {
  it("writes #rrggbb, or #rrggbbaa for a translucent colour", () => {
    expect(toHex({ r: 1, g: 0.5, b: 0, a: 1 })).toBe("#ff8000");
    expect(toHex({ r: 0, g: 0, b: 0, a: 0.5 })).toBe("#00000080");
  });

  it("colorToHex converts oklch and falls back for junk", () => {
    expect(colorToHex("oklch(1 0 0)")).toBe("#ffffff");
    expect(colorToHex("nope", "#123456")).toBe("#123456");
  });

  it("colorToOpaqueHex drops the alpha (what a colour input holds)", () => {
    expect(colorToOpaqueHex("#11223344")).toBe("#112233");
    expect(colorToOpaqueHex("oklch(0.5 0.1 120 / 0.4)")).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe("contrast", () => {
  const black = parseColor("#000")!;
  const white = parseColor("#fff")!;

  it("is 21 for black on white and 1 for equal colours", () => {
    expect(contrastRatio(black, white)).toBeCloseTo(21, 5);
    expect(contrastRatio(white, black)).toBeCloseTo(21, 5);
    expect(contrastRatio(white, white)).toBeCloseTo(1, 5);
  });

  it("matches a known pair (#767676 on white is 4.54)", () => {
    expect(contrastRatio(parseColor("#767676")!, white)).toBeCloseTo(4.54, 2);
  });

  it("composites a translucent colour over an opaque one", () => {
    const half = composite({ r: 0, g: 0, b: 0, a: 0.5 }, white);
    expect(half.r).toBeCloseTo(0.5, 5);
    expect(half.a).toBe(1);
    // A translucent text colour is blended with its background before measuring.
    expect(contrastRatio({ ...black, a: 0.5 }, white)).toBeCloseTo(contrastRatio(half, white), 5);
  });

  it("luminance of white is 1", () => {
    expect(luminance(white)).toBeCloseTo(1, 5);
  });

  it("colorDistance is 0 for equal colours and grows with difference", () => {
    expect(colorDistance(white, white)).toBe(0);
    expect(colorDistance(white, black)).toBeGreaterThan(90);
    expect(colorDistance(parseColor("#808080")!, parseColor("#828282")!)).toBeLessThan(2);
  });
});
