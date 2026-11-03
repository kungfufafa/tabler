import { describe, expect, it } from "vitest";
import { ACCENT_PRESETS, hexLuminance } from "@/lib/theme";
import { badgeContrastStyle } from "./badgeContrast";

describe("database badge contrast", () => {
  it("exceeds 4.5:1 in both themes for every preset and custom RGB samples", () => {
    const colors: string[] = [...ACCENT_PRESETS];
    for (let r = 0; r <= 255; r += 17)
      for (let g = 0; g <= 255; g += 17)
        for (let b = 0; b <= 255; b += 17)
          colors.push("#" + [r, g, b].map((v) => v.toString(16).padStart(2, "0")).join(""));
    for (const color of [...colors, null, undefined, "invalid"]) {
      const style = badgeContrastStyle(color) as Record<string, string>;
      for (const prefix of ["--db-badge", "--db-badge-dark"]) {
        const bg = hexLuminance(style[`${prefix}-bg`]!);
        const fg = hexLuminance(style[`${prefix}-fg`]!);
        expect((Math.max(bg, fg) + 0.05) / (Math.min(bg, fg) + 0.05)).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});
