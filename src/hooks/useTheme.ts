import { useEffect, useSyncExternalStore } from "react";
import { applyTheme, resolveTheme, type ResolvedTheme } from "@/lib/theme";
import { useUiStore } from "@/stores/ui";

const query =
  typeof window !== "undefined" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

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
  const systemDark = useSystemDark();
  return resolveTheme(pref, systemDark);
}

/** Applies the resolved theme to <html>. Mount once near the root. */
export function useTheme(): ResolvedTheme {
  const theme = useResolvedTheme();
  useEffect(() => applyTheme(theme), [theme]);
  return theme;
}
