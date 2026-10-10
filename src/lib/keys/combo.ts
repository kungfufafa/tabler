/** Combo strings look like "Mod+Shift+Enter" (modifier order Mod, Ctrl, Alt, Shift). "Mod" is ⌘ on macOS, Ctrl elsewhere. */

const CODE_KEYS: Record<string, string> = {
  Enter: "Enter",
  NumpadEnter: "Enter",
  Escape: "Escape",
  Backspace: "Backspace",
  Delete: "Delete",
  Tab: "Tab",
  Space: "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backslash: "\\",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
  Semicolon: ";",
  Quote: "'",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
};

export const isMac: boolean =
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.platform ?? navigator.userAgent);

/** Normalizes a key from `code` (layout-stable, immune to ⌥-modified characters on macOS). */
export function keyFromEvent(e: Pick<KeyboardEvent, "code" | "key">): string {
  const code = e.code ?? "";
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F[0-9]{1,2}$/.test(code)) return code;
  const mapped = CODE_KEYS[code];
  if (mapped) return mapped;
  if (e.key.length === 1) return e.key.toUpperCase();
  return e.key;
}

export function comboFromEvent(
  e: Pick<KeyboardEvent, "code" | "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  mac: boolean = isMac,
): string {
  const parts: string[] = [];
  const mod = mac ? e.metaKey : e.ctrlKey;
  const ctrl = mac ? e.ctrlKey : false;
  if (mod) parts.push("Mod");
  if (ctrl) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  parts.push(keyFromEvent(e));
  return parts.join("+");
}

const MAC_GLYPHS: Record<string, string> = {
  Mod: "⌘",
  Ctrl: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Enter: "↩",
  Backspace: "⌫",
  Delete: "⌦",
  Escape: "⎋",
  Tab: "⇥",
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Space: "␣",
};

/** Human-readable form: "Mod+Shift+Enter" → "⌘⇧↩" on macOS, "Ctrl+Shift+Enter" elsewhere. */
export function formatCombo(combo: string, mac: boolean = isMac): string {
  const parts = combo.split("+");
  if (mac) return parts.map((p) => MAC_GLYPHS[p] ?? p).join("");
  return parts.map((p) => (p === "Mod" ? "Ctrl" : p)).join("+");
}
