import { useEffect, useLayoutEffect, useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import { useSystemDark } from "@/hooks/useTheme";
import { setRowHeight } from "@/features/datagrid/types";
import { usePrefsStore } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { applyAppearanceStyle, computeAppearanceStyle } from "./appearanceVars";
import { applyThemeName, resolveSelection } from "./selection";
import { flushThemes, loadThemes, THEMES_LOCAL_KEY, useThemes } from "./themesStore";

/**
 * Applies Settings ▸ Appearance to the window: the theme's name (and a custom theme's colours),
 * the fonts and colour overrides as custom properties on <html>, and the data table's row height.
 * Loads the custom themes, saves them on exit and follows the changes other windows make. Mount once.
 */
export function AppearanceHost() {
  const themePref = useUiStore((s) => s.themePref);
  const customs = useThemes((s) => s.themes);
  const systemDark = useSystemDark();
  const prefs = usePrefsStore(
    useShallow((s) => ({
      editorFontFamily: s.prefs.editorFontFamily,
      editorLineHeight: s.prefs.editorLineHeight,
      gridFontFamily: s.prefs.gridFontFamily,
      gridFontSize: s.prefs.gridFontSize,
      gridLineHeight: s.prefs.gridLineHeight,
      gridRowPadding: s.prefs.gridRowPadding,
      sidebarFontSize: s.prefs.sidebarFontSize,
      sidebarFontFamily: s.prefs.sidebarFontFamily,
      colorOverrides: s.prefs.colorOverrides,
      gridNoElastic: s.prefs.gridNoElastic,
    })),
  );

  const selection = useMemo(
    () => resolveSelection(themePref, systemDark, customs),
    [themePref, systemDark, customs],
  );
  useLayoutEffect(() => applyThemeName(selection), [selection]);

  const style = useMemo(() => computeAppearanceStyle(prefs, selection.key), [prefs, selection.key]);
  useLayoutEffect(() => {
    applyAppearanceStyle(style);
    setRowHeight(style.rowHeight);
  }, [style]);

  useEffect(() => {
    void loadThemes();
    const flush = () => void flushThemes();
    const onStorage = (e: StorageEvent) => {
      if (e.key !== THEMES_LOCAL_KEY || e.newValue === null) return;
      try {
        useThemes.getState().applyExternal(JSON.parse(e.newValue));
      } catch {
        // A half-written value: the next change brings a good one.
      }
    };
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", flush);
    window.addEventListener("storage", onStorage);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", flush);
      window.removeEventListener("storage", onStorage);
    };
  }, []);
  return null;
}
