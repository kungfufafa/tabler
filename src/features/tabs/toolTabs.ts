import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";
import { useUiStore } from "@/stores/ui";
import { useTabsStore, type ToolTab } from "@/stores/tabs";

/**
 * Tool tabs: a feature's own tab kind (Item Overview, metrics boards, plugins…) without a new
 * `Tab` variant. A feature registers its component once (at module load) and opens tabs with
 * `openToolTab`; the tab bar, Open Anything, session restore and the close guards treat them like
 * any other tab.
 */

export interface ToolTabProps<P = unknown> {
  tab: ToolTab;
  /** The tab's params, as `restore` / the opener gave them. */
  params: P;
  /** False while the tab sits in the unfocused group of a split workspace. */
  focused: boolean;
}

export interface ToolTabDefinition<P = unknown> {
  component: ComponentType<ToolTabProps<P>>;
  icon: LucideIcon;
  /** The tab's default title for its params. */
  title: (params: P) => string;
  /**
   * Saved with the session and reopened at startup (with "Reopen closed workspaces"). A function
   * decides per tab from its params, for tools whose tabs cannot all come back (an untitled
   * SQL Viewer has nothing to reopen).
   */
  persist?: boolean | ((params: P) => boolean);
  /**
   * Checks restored (or handed-over) params; null leaves the tab out. Default: taken as they are.
   */
  restore?: (raw: unknown) => P | null;
}

const registry = new Map<string, ToolTabDefinition<never>>();

/** Registers (or replaces) a tool. Returns the unregister function. */
export function registerToolTab<P>(tool: string, definition: ToolTabDefinition<P>): () => void {
  const entry = definition as unknown as ToolTabDefinition<never>;
  registry.set(tool, entry);
  return () => {
    if (registry.get(tool) === entry) registry.delete(tool);
  };
}

export function getToolTab(tool: string): ToolTabDefinition<unknown> | null {
  return (registry.get(tool) as ToolTabDefinition<unknown> | undefined) ?? null;
}

/**
 * Whether a tool tab (of `tool`, with these params) is kept in the saved session. Without `params`
 * a per-tab `persist` function is asked with `null`.
 */
export function persistsToolTab(tool: string, params: unknown = null): boolean {
  const persist = getToolTab(tool)?.persist;
  if (typeof persist !== "function") return persist === true;
  try {
    return persist(params) === true;
  } catch {
    // A broken predicate leaves its tab out; it must not keep the session from being saved.
    return false;
  }
}

/** A tool tab's params as its definition accepts them (null: the tab cannot be rebuilt). */
export function restoreToolParams(tool: string, raw: unknown): unknown {
  const def = getToolTab(tool);
  if (!def) return null;
  return def.restore ? def.restore(raw) : (raw ?? null);
}

/** Same params (structurally): which tab `reuse` finds. */
export function sameParams(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  try {
    return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
  } catch {
    return false;
  }
}

export interface OpenToolTabOptions {
  /** Default: the definition's `title(params)`. */
  title?: string;
  /**
   * An open tab of the tool on the same connection is shown instead of a new one: `true` (the
   * default) when its params are the same, "tool" whatever its params (they are replaced), false
   * never.
   */
  reuse?: boolean | "tool";
}

/** The open tool tab `openToolTab` would reuse. */
export function findToolTab(
  tabs: readonly { kind: string }[],
  connectionId: string,
  tool: string,
  params: unknown,
  reuse: boolean | "tool" = true,
): ToolTab | null {
  if (reuse === false) return null;
  for (const t of tabs as readonly ToolTab[]) {
    if (t.kind !== "tool" || t.connectionId !== connectionId || t.tool !== tool) continue;
    if (reuse === "tool" || sameParams(t.params, params)) return t;
  }
  return null;
}

/**
 * Opens (or shows) a tool tab on `connectionId` and brings the workspace to the front. Returns the
 * tab's id.
 */
export function openToolTab<P>(
  connectionId: string,
  tool: string,
  params: P,
  options: OpenToolTabOptions = {},
): string {
  const store = useTabsStore.getState();
  const def = getToolTab(tool);
  const title = options.title ?? def?.title(params as never) ?? tool;
  const existing = findToolTab(store.tabs, connectionId, tool, params, options.reuse);
  useUiStore.getState().setView("workspace");
  if (existing) {
    if (!sameParams(existing.params, params) || existing.title !== title)
      useTabsStore.setState((s) => ({
        tabs: s.tabs.map((t) => (t.id === existing.id ? { ...existing, params, title } : t)),
      }));
    store.activate(existing.id);
    return existing.id;
  }
  const id = crypto.randomUUID();
  store.open({ id, kind: "tool", connectionId, tool, title, params });
  return id;
}

/** Test-only: forget every registration. */
export function resetToolTabsForTests(): void {
  registry.clear();
}
