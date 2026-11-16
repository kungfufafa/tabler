import { useEffect } from "react";

/** Where the webview's own menu (Cut / Copy / Paste) stays useful. */
const EDITABLE = "input, textarea, [contenteditable]:not([contenteditable='false']), .cm-editor";

/**
 * A right-click on a blank part of the window shows the webview's browser menu (Back, Refresh,
 * Print…), whose Refresh reloads the whole app. Native apps like TablePlus only show their own
 * menus, so the browser menu is suppressed everywhere except in text fields and editors. App
 * context menus (Radix) are unaffected. Dev builds keep it for the devtools' Inspect.
 */
export function attachNativeContextMenuSuppression(
  target: Pick<Window, "addEventListener" | "removeEventListener">,
): () => void {
  const onContextMenu = (e: Event) => {
    const el = e.target;
    if (el instanceof Element && el.closest(EDITABLE)) return;
    e.preventDefault();
  };
  target.addEventListener("contextmenu", onContextMenu);
  return () => target.removeEventListener("contextmenu", onContextMenu);
}

export function useSuppressNativeContextMenu(): void {
  useEffect(() => {
    if (import.meta.env.DEV) return;
    return attachNativeContextMenuSuppression(window);
  }, []);
}
