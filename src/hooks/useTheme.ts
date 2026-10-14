import { useEffect, useSyncExternalStore } from "react";
import { useThemes } from "@/features/appearance/themesStore";
import {
  applyTheme,
  nativeThemeFor,
  resolveTheme,
  setNativeTheme,
  type ResolvedTheme,
} from "@/lib/theme";
import { useUiStore } from "@/stores/ui";

// jsdom (the tests) has no matchMedia.
const query =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;

function subscribe(cb: () => void) {
  query?.addEventListener("change", cb);
  return () => query?.removeEventListener("change", cb);
}

export function useSystemDark(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => query?.matches ?? false,
    () => false,
  );
}

export function useResolvedTheme(): ResolvedTheme {
  const pref = useUiStore((s) => s.themePref);
  // A custom theme says whether it is a light or a dark one.
  const customs = useThemes((s) => s.themes);
  const systemDark = useSystemDark();
  return resolveTheme(pref, systemDark, (id) => customs.find((t) => t.id === id)?.base);
}

/** The native appearance the theme choice asks for: null = the system's (see `nativeThemeFor`). */
export function useNativeTheme(): ResolvedTheme | null {
  const pref = useUiStore((s) => s.themePref);
  const customs = useThemes((s) => s.themes);
  return nativeThemeFor(pref, (id) => customs.find((t) => t.id === id)?.base);
}

/**
 * Applies the resolved theme to <html> and to the native app chrome (menu bar, dialogs, title
 * bar). Mount once near the root; every window mounts its own.
 */
export function useTheme(): ResolvedTheme {
  const theme = useResolvedTheme();
  const native = useNativeTheme();
  useEffect(() => applyTheme(theme), [theme]);
  useEffect(() => {
    void setNativeTheme(native);
  }, [native]);
  return theme;
}
