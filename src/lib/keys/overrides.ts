import type { ActionId } from "@/features/palette/actions";
import type { Scope, ShortcutDef } from "./keymap";

/**
 * The user's changes to the keymap (Settings ▸ Shortcuts), kept in the `keymapOverrides`
 * preference: action id → one combo per built-in binding of that action, in KEYMAP order; `""`
 * removes that binding. Positional, so a remapped binding keeps its own label, scope and flags
 * (⌘T and ⌘E both open a query tab, but only ⌘T works inside the editor). Actions without an entry
 * keep their defaults.
 */
export type KeymapOverrides = Readonly<Record<string, readonly string[]>>;

export const EMPTY_OVERRIDES: KeymapOverrides = Object.freeze({});

/**
 * Bindings that cannot be remapped, as in TablePlus (⌘R reload, ⌘0 toggle the list — #3802). ⌘R
 * was Refresh (`view.refresh`) until it became Reload Workspace; both spellings stay fixed.
 */
export const FIXED_BINDINGS: ReadonlyArray<{ id: ActionId; keys: string }> = [
  { id: "workspace.reload", keys: "Mod+R" },
  { id: "view.refresh", keys: "Mod+R" },
  { id: "ui.toggle-sidebar", keys: "Mod+0" },
];

/**
 * Brings overrides saved by an older keymap up to date (same object when nothing changed):
 * `view.refresh` had two slots (the fixed ⌘R, then ⌥⌘R) before ⌘R moved to `workspace.reload`, so
 * its saved ⌘R slot is dropped and the ⌥⌘R one becomes its only slot.
 */
export function migrateKeymapOverrides(overrides: KeymapOverrides): KeymapOverrides {
  const refresh = overrides["view.refresh"];
  if (!refresh || refresh.length !== 2 || refresh[0] !== "Mod+R") return overrides;
  const next: Record<string, readonly string[]> = { ...overrides };
  if (refresh[1] === "Mod+Alt+R") delete next["view.refresh"];
  else next["view.refresh"] = [refresh[1]!];
  return next;
}

/** Combos no other binding may take: the fixed ones plus Quit (⌘Q, handled by the native menu). */
export const RESERVED_COMBOS: ReadonlySet<string> = new Set([
  ...FIXED_BINDINGS.map((b) => b.keys),
  "Mod+Q",
]);

export function isFixedBinding(def: Pick<ShortcutDef, "id" | "keys">): boolean {
  return FIXED_BINDINGS.some((b) => b.id === def.id && b.keys === def.keys);
}

const KEY = String.raw`(?:[A-Z0-9]|F(?:[1-9]|1[0-9]|2[0-4])|Enter|Escape|Backspace|Delete|Tab|Space|Up|Down|Left|Right|Home|End|PageUp|PageDown|[,./\\[\]\-=\x60;'])`;
const COMBO_RE = new RegExp(String.raw`^(?:Mod\+)?(?:Ctrl\+)?(?:Alt\+)?(?:Shift\+)?${KEY}$`);

/** A combo spelled like KEYMAP's (`Mod+Shift+T`, modifiers in the order Mod, Ctrl, Alt, Shift). */
export function isValidCombo(combo: unknown): combo is string {
  return typeof combo === "string" && COMBO_RE.test(combo);
}

/** True for a value the `keymapOverrides` preference accepts. */
export function isKeymapOverrides(raw: unknown): raw is KeymapOverrides {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return false;
  return Object.values(raw as Record<string, unknown>).every(
    (v) => Array.isArray(v) && v.every((k) => k === "" || isValidCombo(k)),
  );
}

/** The built-in bindings of `id`, in KEYMAP order (their index is the override slot). */
export function defaultSlots(defaults: readonly ShortcutDef[], id: string): ShortcutDef[] {
  return defaults.filter((d) => d.id === id);
}

/** The combo bound to every slot of `id` once the overrides apply ("" = removed). */
export function slotKeys(
  defaults: readonly ShortcutDef[],
  overrides: KeymapOverrides,
  id: string,
): string[] {
  const own = overrides[id];
  return defaultSlots(defaults, id).map((def, i) => {
    if (isFixedBinding(def)) return def.keys;
    const k = own?.[i];
    return k === "" || isValidCombo(k) ? k : def.keys;
  });
}

/**
 * The effective keymap: the defaults with the overrides applied, in KEYMAP order (the first match
 * in a scope wins, so a remapped binding keeps its precedence). Removed bindings are left out;
 * fixed bindings and invalid entries keep their default.
 */
export function resolveKeymap(
  defaults: readonly ShortcutDef[],
  overrides: KeymapOverrides,
): ShortcutDef[] {
  if (Object.keys(overrides).length === 0) return [...defaults];
  const keysById = new Map<string, string[]>();
  const seen = new Map<string, number>();
  const out: ShortcutDef[] = [];
  for (const def of defaults) {
    const slot = seen.get(def.id) ?? 0;
    seen.set(def.id, slot + 1);
    if (!(def.id in overrides)) {
      out.push(def);
      continue;
    }
    let keys = keysById.get(def.id);
    if (!keys) {
      keys = slotKeys(defaults, overrides, def.id);
      keysById.set(def.id, keys);
    }
    const k = keys[slot] ?? def.keys;
    if (k === "") continue;
    out.push(k === def.keys ? def : { ...def, keys: k });
  }
  return out;
}

/** Drops `id` from the overrides when its slots are all back to the defaults. */
function withSlots(
  defaults: readonly ShortcutDef[],
  overrides: KeymapOverrides,
  id: string,
  keys: string[],
): KeymapOverrides {
  const builtIn = defaultSlots(defaults, id).map((d) => d.keys);
  const next: Record<string, readonly string[]> = { ...overrides };
  if (keys.every((k, i) => k === builtIn[i])) delete next[id];
  else next[id] = keys;
  return next;
}

/** Binds slot `slot` of `id` to `keys` ("" removes it). Fixed bindings never change. */
export function setBinding(
  defaults: readonly ShortcutDef[],
  overrides: KeymapOverrides,
  id: string,
  slot: number,
  keys: string,
): KeymapOverrides {
  const slots = defaultSlots(defaults, id);
  const def = slots[slot];
  if (!def || isFixedBinding(def) || (keys !== "" && !isValidCombo(keys))) return overrides;
  const current = slotKeys(defaults, overrides, id);
  if (current[slot] === keys) return overrides;
  current[slot] = keys;
  return withSlots(defaults, overrides, id, current);
}

/** Puts the given slots of `id` (default: all of them) back to their built-in combos. */
export function resetBindings(
  defaults: readonly ShortcutDef[],
  overrides: KeymapOverrides,
  id: string,
  slots?: readonly number[],
): KeymapOverrides {
  if (!(id in overrides)) return overrides;
  const builtIn = defaultSlots(defaults, id).map((d) => d.keys);
  const current = slotKeys(defaults, overrides, id);
  const keys = current.map((k, i) => (!slots || slots.includes(i) ? builtIn[i]! : k));
  return withSlots(defaults, overrides, id, keys);
}

/**
 * Other actions bound to `keys` in `scope` (the same action twice is not a conflict). Keys in
 * different scopes never clash: the focused scope's binding wins (⌘E is Explain in the editor).
 */
export function conflictsWith(
  keymap: readonly ShortcutDef[],
  scope: Scope,
  keys: string,
  id: string,
): ShortcutDef[] {
  const out: ShortcutDef[] = [];
  for (const d of keymap) {
    if (d.scope !== scope || d.keys !== keys || d.id === id) continue;
    if (!out.some((o) => o.id === d.id)) out.push(d);
  }
  return out;
}

export type CaptureResult =
  | { type: "cancel" }
  | { type: "clear" }
  /** Only modifiers are held so far. */
  | { type: "pending" }
  | { type: "combo"; combo: string };

const MODIFIER_KEYS = new Set(["Meta", "Control", "Alt", "Shift", "CapsLock", "Fn", "OS"]);

/**
 * What a keydown means while Settings ▸ Shortcuts waits for a new shortcut: ⎋ cancels, ⌫ (or ⌦)
 * clears the binding, a modifier alone keeps waiting, anything else is the combo (`combo`, from
 * `comboFromEvent`).
 */
export function captureResult(
  e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  combo: string,
): CaptureResult {
  if (MODIFIER_KEYS.has(e.key)) return { type: "pending" };
  const plain = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
  if (plain && e.key === "Escape") return { type: "cancel" };
  if (plain && (e.key === "Backspace" || e.key === "Delete")) return { type: "clear" };
  return { type: "combo", combo };
}

/**
 * Why `combo` cannot be bound to `def` (null when it can): reserved combos, keys the shortcut
 * parser does not know, and plain keys (they type text) unless the binding was built that way
 * (Space toggles the row detail in the grid).
 */
export function comboProblem(
  combo: string,
  def: Pick<ShortcutDef, "id" | "keys">,
): "reserved" | "invalid" | "needs-modifier" | null {
  if (RESERVED_COMBOS.has(combo) && !(def.keys === combo && isFixedBinding(def))) return "reserved";
  if (!isValidCombo(combo)) return "invalid";
  const parts = combo.split("+");
  const key = parts[parts.length - 1]!;
  const modified = parts.some((p) => p === "Mod" || p === "Ctrl" || p === "Alt");
  if (!modified && !/^F\d+$/.test(key) && combo !== def.keys) return "needs-modifier";
  return null;
}

/** "Mod+Shift+T" → "CmdOrCtrl+Shift+T": the accelerator syntax of the native menu (Tauri). */
export function comboToAccelerator(combo: string): string {
  return combo
    .split("+")
    .map((p) => (p === "Mod" ? "CmdOrCtrl" : p))
    .join("+");
}

/**
 * The native menu's accelerator for each of `ids`: the action's primary (first) combo in the
 * effective keymap, null when it has none left.
 */
export function menuAccelerators(
  keymap: readonly ShortcutDef[],
  ids: readonly string[],
): Record<string, string | null> {
  const out: Record<string, string | null> = {};
  for (const id of ids) {
    const keys = keymap.find((d) => d.id === id)?.keys;
    out[id] = keys ? comboToAccelerator(keys) : null;
  }
  return out;
}
