import { Monitor, Moon, Palette, Sun, type LucideIcon } from "lucide-react";
import { BUILTIN_THEMES } from "@/features/appearance/builtinThemes";
import type { CustomTheme } from "@/features/appearance/customTheme";
import type { ThemePref } from "@/stores/ui";
import type { PaletteItem } from "./items";

const SCHEME_KEYWORDS = "appearance color scheme toggle theme";

/** The icon of a theme: the sun / moon of its base, the palette for the user's own. */
function iconFor(base: "light" | "dark", custom: boolean): LucideIcon {
  if (custom) return Palette;
  return base === "dark" ? Moon : Sun;
}

/**
 * The palette's theme commands: one per theme of Settings ▸ Appearance ▸ Theme, so every theme
 * can be applied from the keyboard: Auto, Light, Dark, Solarized Light, Solarized Dark and each
 * custom theme (id-based, labelled with the theme's name). The one in use is marked `current`.
 */
export function themeCommandItems(ctx: {
  themePref: ThemePref;
  customThemes?: readonly CustomTheme[];
  setTheme: (pref: ThemePref) => void;
}): PaletteItem[] {
  const customs = ctx.customThemes ?? [];
  // A choice whose custom theme is gone is what Auto shows (the window falls back to it).
  const dangling =
    ctx.themePref.startsWith("custom:") && !customs.some((t) => `custom:${t.id}` === ctx.themePref);
  const item = (
    pref: ThemePref,
    name: string,
    icon: LucideIcon,
    keywords: string,
    current: boolean,
  ): PaletteItem => ({
    key: `cmd:theme.${pref}`,
    group: "actions",
    label: `Theme: ${name}`,
    keywords: [SCHEME_KEYWORDS, keywords],
    icon: { type: "action", icon },
    current,
    run: () => ctx.setTheme(pref),
    verb: "Apply",
  });
  const mode = (base: "light" | "dark") => (base === "dark" ? "dark mode" : "light mode");
  return [
    item(
      "system",
      "Auto",
      Monitor,
      "system automatic follows macos appearance",
      ctx.themePref === "system" || dangling,
    ),
    ...BUILTIN_THEMES.map((t) =>
      item(t.id, t.label, iconFor(t.base, false), mode(t.base), ctx.themePref === t.id),
    ),
    ...customs.map((t) =>
      item(
        `custom:${t.id}`,
        t.name,
        iconFor(t.base, true),
        `custom ${mode(t.base)}`,
        ctx.themePref === `custom:${t.id}`,
      ),
    ),
  ];
}
