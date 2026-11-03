import type { CSSProperties } from "react";
import { hexLuminance, isHexColor, type AccentPreset } from "@/lib/theme";

const COLORS: Record<AccentPreset, string> = {
  red: "#d44545",
  orange: "#d97722",
  yellow: "#c99a14",
  green: "#269b58",
  teal: "#219aa2",
  blue: "#3978ce",
  purple: "#9054cb",
  gray: "#626873",
};

function mix(hex: string, base: number, amount: number): string {
  return (
    "#" +
    [1, 3, 5]
      .map((i) =>
        Math.round(parseInt(hex.slice(i, i + 2), 16) * amount + base * (1 - amount))
          .toString(16)
          .padStart(2, "0"),
      )
      .join("")
  );
}

/** Choose the higher WCAG contrast: black or white always gives at least 4.58:1. */
export function badgeForeground(background: string): string {
  const luminance = hexLuminance(background);
  return (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) ? "#000000" : "#ffffff";
}

/** Opaque backgrounds keep contrast independent of custom themes and selected/hover surfaces. */
export function badgeContrastStyle(color: string | null | undefined): CSSProperties {
  const preset = COLORS[(color ?? "blue") as AccentPreset] ?? COLORS.blue;
  const accent = isHexColor(color) ? color : preset;
  const light = mix(accent, 255, 0.18);
  const dark = mix(accent, 0, 0.35);
  return {
    "--db-badge-bg": light,
    "--db-badge-fg": badgeForeground(light),
    "--db-badge-dark-bg": dark,
    "--db-badge-dark-fg": badgeForeground(dark),
  } as CSSProperties;
}
