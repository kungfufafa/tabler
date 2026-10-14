import { create } from "zustand";
import { isMatchStrategy, type MatchStrategy } from "@/features/objects/search";

/** Which pane the left sidebar shows: the schema items or the query history. */
export type SidebarPane = "items" | "history";

/** Collapsible sections of the items tree (also their header labels). */
export type SidebarSection =
  "Recent" | "Pinned" | "Functions" | "Tables" | "Views" | "Materialized views" | "Sequences";

/** localStorage key of the visible pane (`"items"` / `"history"`). */
export const PANE_STORAGE_KEY = "tabler.sidebar.pane";
/** localStorage key of the collapsed sections (a list of `SidebarSection`s). */
export const COLLAPSED_STORAGE_KEY = "tabler.sidebar.collapsed";
/**
 * localStorage key of the sidebar's view options. Version 2: Recent and the row counts became
 * opt-in (TablePlus lists names only). The first build saved every option whenever one changed, so
 * a stored `true` there cannot tell a choice from the old default: see `migrateLegacyOptions`.
 */
export const OPTIONS_STORAGE_KEY = "tabler.sidebar.options.v2";
/** Where the first build kept the options (Recent and row counts on by default). */
export const LEGACY_OPTIONS_STORAGE_KEY = "tabler.sidebar.options";
/** localStorage key of the item search's Matching Strategy (search menu, `MatchStrategy`). */
export const MATCH_STORAGE_KEY = "tabler.sidebar.match";

export interface SidebarOptions {
  /** List the recently opened tables in a "Recent" section. */
  showRecent: boolean;
  /** Show each table's estimated row count on the right of its row. */
  showRowCounts: boolean;
  /** List functions and procedures in a "Functions" section (TablePlus: on by default). */
  showFunctions: boolean;
  /** PostgreSQL: list the schema's sequences in a "Sequences" section. */
  showSequences: boolean;
  /** PostgreSQL: offer information_schema, pg_catalog and pg_toast in the schema picker. */
  showSystemSchemas: boolean;
  /** The history pane lists every connection's statements, not just the active one's. */
  historyAllConnections: boolean;
  /** Table / view rows expand to list their columns (TablePlus b670, off by default). */
  showColumns: boolean;
}

export const DEFAULT_SIDEBAR_OPTIONS: SidebarOptions = {
  // Off: the section grew on every open and moved the list under the pointer (⚙ turns it on).
  showRecent: false,
  showRowCounts: false,
  showFunctions: true,
  showSequences: false,
  showSystemSchemas: false,
  historyAllConnections: false,
  showColumns: false,
};

const SECTIONS: readonly SidebarSection[] = [
  "Recent",
  "Pinned",
  "Functions",
  "Tables",
  "Views",
  "Materialized views",
  "Sequences",
];

export const EMPTY_COLLAPSED: ReadonlySet<SidebarSection> = new Set();

function readJson(key: string): unknown {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? null : JSON.parse(raw);
  } catch {
    // Unavailable storage (private window, blocked site data) or a malformed value.
    return null;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Not persisted; the in-memory state still works for this session.
  }
}

export function parsePane(value: unknown): SidebarPane {
  return value === "history" ? "history" : "items";
}

export function parseCollapsed(value: unknown): ReadonlySet<SidebarSection> {
  if (!Array.isArray(value)) return EMPTY_COLLAPSED;
  const sections = value.filter((s): s is SidebarSection => SECTIONS.includes(s as SidebarSection));
  return sections.length > 0 ? new Set(sections) : EMPTY_COLLAPSED;
}

export function parseOptions(value: unknown): SidebarOptions {
  const out = { ...DEFAULT_SIDEBAR_OPTIONS };
  if (!value || typeof value !== "object" || Array.isArray(value)) return out;
  for (const key of Object.keys(out) as Array<keyof SidebarOptions>) {
    const v = (value as Record<string, unknown>)[key];
    if (typeof v === "boolean") out[key] = v;
  }
  return out;
}

/**
 * Options saved by the first build, read once when nothing is stored under the current key. Only
 * the history scope carries over: its default did not change, while a stored Recent / row-counts
 * `true` was most likely just the old default, so those start from the new (off) defaults.
 */
export function migrateLegacyOptions(value: unknown): SidebarOptions {
  const legacy = parseOptions(value);
  return { ...DEFAULT_SIDEBAR_OPTIONS, historyAllConnections: legacy.historyAllConnections };
}

function readOptions(): SidebarOptions {
  const current = readJson(OPTIONS_STORAGE_KEY);
  if (current !== null) return parseOptions(current);
  const legacy = readJson(LEGACY_OPTIONS_STORAGE_KEY);
  if (legacy === null) return { ...DEFAULT_SIDEBAR_OPTIONS };
  const options = migrateLegacyOptions(legacy);
  writeJson(OPTIONS_STORAGE_KEY, options);
  try {
    localStorage.removeItem(LEGACY_OPTIONS_STORAGE_KEY);
  } catch {
    // Left behind; the current key wins from now on.
  }
  return options;
}

interface SidebarPrefsState {
  pane: SidebarPane;
  collapsed: ReadonlySet<SidebarSection>;
  options: SidebarOptions;
  /** How the item search matches plain terms (search menu ▸ Matching Strategy). */
  matchStrategy: MatchStrategy;
}

export function parseMatchStrategy(value: unknown): MatchStrategy {
  return isMatchStrategy(value) ? value : "fuzzy";
}

function readStored(): SidebarPrefsState {
  return {
    pane: parsePane(readJson(PANE_STORAGE_KEY)),
    collapsed: parseCollapsed(readJson(COLLAPSED_STORAGE_KEY)),
    options: readOptions(),
    matchStrategy: parseMatchStrategy(readJson(MATCH_STORAGE_KEY)),
  };
}

export const useSidebarPrefs = create<SidebarPrefsState>()(() => readStored());

export function setSidebarPane(pane: SidebarPane): void {
  if (useSidebarPrefs.getState().pane === pane) return;
  useSidebarPrefs.setState({ pane });
  writeJson(PANE_STORAGE_KEY, pane);
}

/** Collapses an expanded section of the items tree, or expands a collapsed one. */
export function toggleSection(section: SidebarSection): void {
  const next = new Set(useSidebarPrefs.getState().collapsed);
  if (next.has(section)) next.delete(section);
  else next.add(section);
  useSidebarPrefs.setState({ collapsed: next });
  writeJson(COLLAPSED_STORAGE_KEY, [...next]);
}

export function setSectionCollapsed(section: SidebarSection, collapsed: boolean): void {
  if (useSidebarPrefs.getState().collapsed.has(section) !== collapsed) toggleSection(section);
}

export function setSidebarOption<K extends keyof SidebarOptions>(
  key: K,
  value: SidebarOptions[K],
): void {
  const current = useSidebarPrefs.getState().options;
  if (current[key] === value) return;
  const options = { ...current, [key]: value };
  useSidebarPrefs.setState({ options });
  writeJson(OPTIONS_STORAGE_KEY, options);
}

export function setMatchStrategy(strategy: MatchStrategy): void {
  if (useSidebarPrefs.getState().matchStrategy === strategy) return;
  useSidebarPrefs.setState({ matchStrategy: strategy });
  writeJson(MATCH_STORAGE_KEY, strategy);
}

/** Test helper: reads the stored preferences again. */
export function reloadSidebarPrefs(): void {
  useSidebarPrefs.setState(readStored());
}
