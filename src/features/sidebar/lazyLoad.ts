import { create } from "zustand";
import { useProfiles } from "@/features/connections/useConnections";
import type { ConnectionProfile, Driver } from "@/lib/ipc";
import type { OpenConnection } from "@/stores/connections";
import { usePrefs } from "@/stores/prefs";

/**
 * "Lazy Load Schema Items" (TablePlus spec §1.3, §2.3, §11.1): the sidebar lists the schemas
 * (PostgreSQL) or databases (MySQL) as nodes and loads a node's items only once it is expanded;
 * Open Anything then searches only the loaded ones.
 *
 * Whether a connection lazy-loads: the sidebar's own toggle (⚙ / "+" ▸ Settings, the search
 * menu), kept per saved connection; else the connection form's option (`options.lazyLoad`, URL
 * `lazyload`); else the "Lazy load schema items" preference (Settings ▸ General, off). A sidebar
 * toggle remembers the form's value it was made against, so changing the form later wins again.
 */

/** localStorage key of the sidebar toggles (profile id → `LazyOverride`). */
export const LAZY_STORAGE_KEY = "tabler.sidebar.lazyLoad";

export interface LazyOverride {
  value: boolean;
  /** The profile's own option when the toggle was set (it applies while that is unchanged). */
  base: boolean | null;
}

/** PostgreSQL (schema nodes) and MySQL (database nodes); SQLite files always load whole. */
export function supportsLazyLoad(driver: Driver): boolean {
  return driver === "postgres" || driver === "mysql";
}

/** The connection form's option, when it has one (`options.lazyLoad`). */
export function profileLazyLoad(profile: Pick<ConnectionProfile, "options"> | null | undefined) {
  const value = (profile?.options as Record<string, unknown> | undefined)?.lazyLoad;
  return typeof value === "boolean" ? value : null;
}

export function resolveLazyLoad(
  override: LazyOverride | undefined,
  profileOption: boolean | null,
  prefDefault: boolean,
): boolean {
  if (override !== undefined && override.base === profileOption) return override.value;
  return profileOption ?? prefDefault;
}

/** Expanded and loaded nodes of one connection's database (`lazyKey`). */
export interface LazyTreeState {
  /** Nodes showing their items. */
  expanded: readonly string[];
  /** Nodes whose items were loaded (expanded once, or "Load all schemas"): what is searched. */
  loaded: readonly string[];
}

export const EMPTY_LAZY_TREE: LazyTreeState = Object.freeze({
  expanded: Object.freeze([]) as readonly string[],
  loaded: Object.freeze([]) as readonly string[],
});

/** The tree state belongs to a connection and the database it is on. */
export function lazyKey(connectionId: string, database: string | null): string {
  return `${connectionId}|${database ?? ""}`;
}

interface LazyState {
  overrides: Readonly<Record<string, LazyOverride>>;
  trees: Readonly<Record<string, LazyTreeState>>;
}

function readOverrides(): Record<string, LazyOverride> {
  try {
    const raw = localStorage.getItem(LAZY_STORAGE_KEY);
    const value: unknown = raw === null ? null : JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const out: Record<string, LazyOverride> = {};
    for (const [id, o] of Object.entries(value as Record<string, unknown>)) {
      if (!o || typeof o !== "object") continue;
      const { value: v, base } = o as { value?: unknown; base?: unknown };
      if (typeof v === "boolean" && (base === null || typeof base === "boolean"))
        out[id] = { value: v, base };
    }
    return out;
  } catch {
    // Unavailable storage or a malformed value: no toggles.
    return {};
  }
}

export const useLazyStore = create<LazyState>()(() => ({ overrides: readOverrides(), trees: {} }));

/** Whether the connection's sidebar lazy-loads (see the module comment). */
export function useLazyLoad(connection: OpenConnection | null): boolean {
  const profiles = useProfiles();
  const prefDefault = usePrefs((p) => p.sidebarLazyLoad);
  const profileId = connection?.info.profileId ?? "";
  const override = useLazyStore((s) => s.overrides[profileId]);
  if (!connection || !supportsLazyLoad(connection.info.driver)) return false;
  const profile = profiles.data?.find((p) => p.id === profileId);
  return resolveLazyLoad(override, profileLazyLoad(profile), prefDefault);
}

/** The sidebar's toggle for one saved connection (`profileOption`: its form's current value). */
export function setLazyLoad(profileId: string, value: boolean, profileOption: boolean | null) {
  const overrides = {
    ...useLazyStore.getState().overrides,
    [profileId]: { value, base: profileOption },
  };
  useLazyStore.setState({ overrides });
  try {
    localStorage.setItem(LAZY_STORAGE_KEY, JSON.stringify(overrides));
  } catch {
    // Kept for this session only.
  }
}

/** The connection form's lazy-load option of a profile (the base of a sidebar toggle). */
export function useProfileLazyOption(profileId: string | null): boolean | null {
  const profiles = useProfiles();
  return profileLazyLoad(profiles.data?.find((p) => p.id === profileId));
}

export function useLazyTree(key: string): LazyTreeState {
  return useLazyStore((s) => s.trees[key] ?? EMPTY_LAZY_TREE);
}

export function lazyTreeOf(key: string): LazyTreeState {
  return useLazyStore.getState().trees[key] ?? EMPTY_LAZY_TREE;
}

function updateTree(key: string, update: (tree: LazyTreeState) => LazyTreeState): void {
  useLazyStore.setState((s) => {
    const current = s.trees[key] ?? EMPTY_LAZY_TREE;
    const next = update(current);
    return next === current ? s : { trees: { ...s.trees, [key]: next } };
  });
}

const withItem = (list: readonly string[], item: string) =>
  list.includes(item) ? list : [...list, item];

/** First look at a connection's tree: `schemas` (the current one) start expanded. */
export function initLazyTree(key: string, schemas: readonly string[]): void {
  if (useLazyStore.getState().trees[key] !== undefined) return;
  updateTree(key, () => ({ expanded: [...schemas], loaded: [...schemas] }));
}

/** Expands a collapsed node (loading its items) or collapses an expanded one. */
export function toggleLazyNode(key: string, schema: string, open?: boolean): void {
  updateTree(key, (tree) => {
    const isOpen = tree.expanded.includes(schema);
    if (open === isOpen) return tree;
    if (isOpen) return { ...tree, expanded: tree.expanded.filter((s) => s !== schema) };
    return { expanded: [...tree.expanded, schema], loaded: withItem(tree.loaded, schema) };
  });
}

/** Expands (and loads) a node: an item of it was opened from Open Anything. */
export function revealLazyNode(key: string, schema: string): void {
  toggleLazyNode(key, schema, true);
}

/** "Load all schemas": every node's items are loaded (and searched), none is expanded. */
export function loadLazyNodes(key: string, schemas: readonly string[]): void {
  updateTree(key, (tree) => {
    const missing = schemas.filter((s) => !tree.loaded.includes(s));
    return missing.length === 0 ? tree : { ...tree, loaded: [...tree.loaded, ...missing] };
  });
}

/** Collapses every node of the tree ("Collapse All"). */
export function collapseLazyNodes(key: string): void {
  updateTree(key, (tree) => (tree.expanded.length === 0 ? tree : { ...tree, expanded: [] }));
}

/** Test helper: forgets the trees and reads the stored toggles again. */
export function resetLazyLoad(): void {
  useLazyStore.setState({ overrides: readOverrides(), trees: {} });
}
