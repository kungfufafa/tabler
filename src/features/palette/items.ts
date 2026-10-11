import {
  ArrowUpRight,
  Check,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  CirclePlay,
  CircleSlash2,
  Copy,
  Eraser,
  FileDown,
  FolderOpen,
  Funnel,
  History,
  ListTree,
  PanelLeft,
  PanelRight,
  Play,
  Plug,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Settings,
  Square,
  SquareChevronRight,
  SquareX,
  TextAlignStart,
  Trash2,
  Undo2,
  X,
  type LucideIcon,
} from "lucide-react";
import type { ConnectionProfile, Driver, TableKind, TableSummary } from "@/lib/ipc";
import type { ShortcutDef } from "@/lib/keys/keymap";
import type { Tab, TabKind } from "@/stores/tabs";
import type { ActionId } from "./actions";
import { fuzzyMatch, isWordStart, normalizeQuery, type MatchRange } from "./fuzzy";

export type PaletteGroupId = "tables" | "tabs" | "connections" | "actions";
export type PaletteSectionId = "recent" | PaletteGroupId;

export const GROUP_ORDER: readonly PaletteGroupId[] = ["tables", "tabs", "connections", "actions"];

export const SECTION_HEADINGS: Record<PaletteSectionId, string> = {
  recent: "Recent",
  tables: "Tables",
  tabs: "Open tabs",
  connections: "Connections",
  actions: "Actions",
};

/** Large schemas can have thousands of tables; only this many are rendered per query. */
export const TABLE_LIMIT = 200;
/** Recently used items shown on top while the query is empty. */
export const RECENT_LIMIT = 5;

export type PaletteIcon =
  | { type: "table"; kind: TableKind }
  | { type: "tab"; kind: TabKind }
  | { type: "connection"; color: string | null; driver: Driver }
  | { type: "action"; icon: LucideIcon };

export interface PaletteItem {
  /** Stable unique id: the cmdk value and the MRU key. */
  key: string;
  group: PaletteGroupId;
  label: string;
  /** Muted text after the label; searched too, with a lower weight than the label. */
  detail?: string;
  /** Extra search terms that are not displayed. */
  keywords?: string[];
  /**
   * Hidden terms that match only when the whole query equals one of them (e.g. "view" lists the
   * views). Shared by many items, so a partial query must not pull them all in.
   */
  tags?: string[];
  /** Right-aligned muted text (row estimate, connection name, …). */
  meta?: string;
  /** Keyboard shortcut rendered with <Kbd>. */
  combo?: string;
  icon: PaletteIcon;
  /** Shows the "unsaved changes" dot (table tabs with staged edits). */
  dirty?: boolean;
  /** The item reflects the current state (active tab, active connection, current theme). */
  current?: boolean;
  /** May return a promise; rejections surface as a toast. */
  run: () => unknown;
  /** What ↩ does, shown in the footer ("Open", "Connect", "Run", …). */
  verb: string;
  /** ⌘↩ alternative, e.g. open a table's structure instead of its data. */
  runAlt?: () => void;
  altLabel?: string;
  /**
   * Never recorded as recently used nor shown under "Recent" (destructive commands such as
   * Disconnect must not become the default ↩ target of an empty query).
   */
  noRecent?: boolean;
}

export interface RankedItem {
  item: PaletteItem;
  /** Highlight ranges within `item.label`. */
  ranges: MatchRange[];
}

export interface PaletteSection {
  id: PaletteSectionId;
  heading: string;
  items: RankedItem[];
  /** Matches that were cut off by the section limit. */
  more: number;
}

// ---- matching & ranking ------------------------------------------------------

/** Detail/keyword hits rank below label hits and highlight nothing. */
const SECONDARY_WEIGHT = 0.5;
const SECONDARY_PENALTY = 10;
/** A tag hit scores like a plain prefix match of the same length. */
const SCORE_PER_TAG_CHAR = 2;

/**
 * Hidden or secondary text only counts when every matched run starts a word ("dark" in "dark
 * mode", "lp" in "Local Postgres"), so short queries do not pull in noise through keywords.
 */
function strictMatch(query: string, text: string): number | null {
  const m = fuzzyMatch(query, text);
  if (!m || !m.ranges.every(([start]) => isWordStart(text, start))) return null;
  return m.score;
}

export function matchItem(
  query: string,
  item: PaletteItem,
): { score: number; ranges: MatchRange[] } | null {
  const label = fuzzyMatch(query, item.label);
  if (label) return label;
  let best: number | null = null;
  for (const text of [item.detail, ...(item.keywords ?? [])]) {
    if (!text) continue;
    const score = strictMatch(query, text);
    if (score !== null && (best === null || score > best)) best = score;
  }
  if (best === null) {
    const q = normalizeQuery(query);
    if (!item.tags?.some((tag) => normalizeQuery(tag) === q)) return null;
    best = q.length * SCORE_PER_TAG_CHAR;
  }
  return { score: best * SECONDARY_WEIGHT - SECONDARY_PENALTY, ranges: [] };
}

/**
 * Filters and orders one group. With an empty query the natural order is kept; otherwise matches
 * are sorted by score, ties broken by recency (MRU) and then by the natural order.
 */
export function rankItems(
  items: readonly PaletteItem[],
  query: string,
  mru: readonly string[] = [],
  limit: number = Infinity,
): { items: RankedItem[]; more: number } {
  if (normalizeQuery(query) === "") {
    const shown = items.slice(0, limit).map((item) => ({ item, ranges: [] }));
    return { items: shown, more: Math.max(0, items.length - shown.length) };
  }
  const recency = new Map(mru.map((k, i) => [k, i]));
  const matched: Array<RankedItem & { score: number; index: number }> = [];
  items.forEach((item, index) => {
    const m = matchItem(query, item);
    if (m) matched.push({ item, ranges: m.ranges, score: m.score, index });
  });
  matched.sort(
    (a, b) =>
      b.score - a.score ||
      (recency.get(a.item.key) ?? Infinity) - (recency.get(b.item.key) ?? Infinity) ||
      a.index - b.index,
  );
  const shown = matched.slice(0, limit).map(({ item, ranges }) => ({ item, ranges }));
  return { items: shown, more: matched.length - shown.length };
}

export interface BuildSectionsOptions {
  mru?: readonly string[];
  limits?: Partial<Record<PaletteGroupId, number>>;
  recentLimit?: number;
}

/**
 * Builds the visible sections in the fixed group order. While the query is empty, recently used
 * items are pulled out of their groups into a leading "Recent" section.
 */
export function buildSections(
  groups: Partial<Record<PaletteGroupId, readonly PaletteItem[]>>,
  query: string,
  {
    mru = [],
    limits = { tables: TABLE_LIMIT },
    recentLimit = RECENT_LIMIT,
  }: BuildSectionsOptions = {},
): PaletteSection[] {
  const sections: PaletteSection[] = [];
  const empty = normalizeQuery(query) === "";
  const recentKeys = new Set<string>();

  if (empty && recentLimit > 0 && mru.length > 0) {
    const byKey = new Map<string, PaletteItem>();
    for (const g of GROUP_ORDER) for (const item of groups[g] ?? []) byKey.set(item.key, item);
    const recent: RankedItem[] = [];
    for (const key of mru) {
      const item = byKey.get(key);
      if (!item || item.noRecent) continue;
      recent.push({ item, ranges: [] });
      recentKeys.add(key);
      if (recent.length >= recentLimit) break;
    }
    if (recent.length > 0)
      sections.push({ id: "recent", heading: SECTION_HEADINGS.recent, items: recent, more: 0 });
  }

  for (const g of GROUP_ORDER) {
    const source = groups[g] ?? [];
    const items = recentKeys.size > 0 ? source.filter((i) => !recentKeys.has(i.key)) : source;
    const ranked = rankItems(items, query, mru, limits[g] ?? Infinity);
    if (ranked.items.length === 0) continue;
    sections.push({ id: g, heading: SECTION_HEADINGS[g], ...ranked });
  }
  return sections;
}

/** Flattened item order as rendered, used to resolve the selected key. */
export function flattenSections(sections: readonly PaletteSection[]): PaletteItem[] {
  return sections.flatMap((s) => s.items.map((r) => r.item));
}

// ---- builders ---------------------------------------------------------------

/** Encodes user-controlled parts so keys never carry whitespace (cmdk trims values). */
const enc = (s: string | null | undefined) => encodeURIComponent(s ?? "");

export function tableItemKey(profileId: string, table: { schema: string | null; name: string }) {
  return `table:${enc(profileId)}:${enc(table.schema)}:${enc(table.name)}`;
}

function formatEstimate(n: number | null): string | undefined {
  if (n === null || n < 0) return undefined;
  const text = new Intl.NumberFormat(undefined, {
    notation: n >= 10_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(n);
  return n > 1000 ? `~${text}` : text;
}

/**
 * Typing a kind lists the relations of that kind ("view", "materialized view"). Plain tables get
 * no tag: every relation in the Tables group is a table, and "tab"/"table" must reach the tab
 * actions instead of listing the whole schema.
 */
const KIND_TAGS: Record<TableKind, string[]> = {
  table: [],
  partitioned: ["partitioned", "partitioned table"],
  foreignTable: ["foreign", "foreign table"],
  view: ["view"],
  systemView: ["view", "system view"],
  materializedView: ["view", "materialized view", "matview"],
};

export function tableItems(
  tables: readonly TableSummary[],
  ctx: { profileId: string; open: (table: TableSummary, kind: "table" | "structure") => void },
): PaletteItem[] {
  return tables.map((t) => ({
    key: tableItemKey(ctx.profileId, t),
    group: "tables",
    label: t.name,
    tags: KIND_TAGS[t.kind],
    meta: formatEstimate(t.rowEstimate),
    icon: { type: "table", kind: t.kind },
    run: () => ctx.open(t, "table"),
    verb: "Open",
    runAlt: () => ctx.open(t, "structure"),
    altLabel: "Structure",
  }));
}

const TAB_DETAIL: Record<TabKind, string> = { table: "Data", structure: "Structure", query: "" };

export function tabItems(
  tabs: readonly Tab[],
  ctx: {
    activeId: string | null;
    isDirty?: (tab: Tab) => boolean;
    /** Connection name for the right-hand meta; omitted when only one connection is open. */
    connectionName?: (connectionId: string) => string | null;
    activate: (tab: Tab) => void;
  },
): PaletteItem[] {
  return tabs.map((tab) => {
    const where = tab.kind === "query" ? "" : (tab.table.schema ?? "");
    const detail = [TAB_DETAIL[tab.kind], where].filter(Boolean).join(" · ");
    return {
      key: `tab:${tab.id}`,
      group: "tabs",
      label: tab.title,
      detail: detail || undefined,
      keywords: tab.kind === "query" ? ["query sql"] : where ? [`${where}.${tab.title}`] : [],
      meta: ctx.connectionName?.(tab.connectionId) ?? undefined,
      icon: { type: "tab", kind: tab.kind },
      dirty: ctx.isDirty?.(tab) ?? false,
      current: tab.id === ctx.activeId,
      run: () => ctx.activate(tab),
      verb: "Switch to tab",
    };
  });
}

export function connectionItems(
  profiles: readonly ConnectionProfile[],
  ctx: {
    /** profileId → open connectionId */
    openByProfile: ReadonlyMap<string, string>;
    /** Profiles with a connect in flight. */
    connecting?: ReadonlySet<string>;
    activeConnectionId: string | null;
    summary: (p: ConnectionProfile) => string;
    connect: (p: ConnectionProfile) => void;
    switchTo: (connectionId: string) => void;
  },
): PaletteItem[] {
  return profiles.map((p) => {
    const connectionId = ctx.openByProfile.get(p.id) ?? null;
    const active = connectionId !== null && connectionId === ctx.activeConnectionId;
    const pending = connectionId === null && (ctx.connecting?.has(p.id) ?? false);
    const base = {
      key: `conn:${enc(p.id)}`,
      group: "connections",
      label: p.name,
      detail: ctx.summary(p) || undefined,
      keywords: [p.driver, p.group ?? "", p.database ?? "", p.host ?? ""].filter(Boolean),
      icon: { type: "connection", color: p.color, driver: p.driver },
    } satisfies Partial<PaletteItem>;
    if (connectionId)
      return {
        ...base,
        meta: active ? undefined : "Connected",
        current: active,
        run: () => ctx.switchTo(connectionId),
        verb: "Switch",
      };
    // Already on its way: a second connect would open a duplicate session.
    if (pending) return { ...base, meta: "Connecting…", run: () => {}, verb: "Connecting…" };
    return { ...base, run: () => ctx.connect(p), verb: "Connect" };
  });
}

const ACTION_ICONS: Partial<Record<ActionId, LucideIcon>> = {
  "tab.new-query": Plus,
  "tab.close": X,
  "tab.close-all": SquareX,
  "tab.reopen": RotateCcw,
  "tab.next": ChevronRight,
  "tab.prev": ChevronLeft,
  "settings.open": Settings,
  "ui.toggle-sidebar": PanelLeft,
  "ui.toggle-inspector": PanelRight,
  "ui.launcher": Plug,
  "sidebar.focus-search": Search,
  "view.refresh": RefreshCw,
  "grid.commit": Check,
  "grid.discard": Eraser,
  "grid.filter": Funnel,
  "grid.new-row": Plus,
  "grid.delete-rows": Trash2,
  "grid.duplicate-row": Copy,
  "grid.set-null": CircleSlash2,
  "grid.set-default": CircleDot,
  "grid.open-fk": ArrowUpRight,
  "grid.undo": Undo2,
  "query.run": Play,
  "query.run-current": CirclePlay,
  "query.cancel": Square,
  "query.explain": ListTree,
  "query.format": TextAlignStart,
  "query.history": History,
  "query.save": Save,
  "query.open": FolderOpen,
  "export.open": FileDown,
};

export function actionIcon(id: ActionId): LucideIcon {
  return ACTION_ICONS[id] ?? SquareChevronRight;
}

/** The palette itself and the per-index tab jumps (covered by "Open tabs") are not listed. */
export function isListedAction(id: ActionId): boolean {
  return id !== "palette.open" && !id.startsWith("tab.goto-");
}

export interface ActionContext {
  hasConnection: boolean;
  tabCount: number;
  closedCount: number;
}

/**
 * The tab actions' handlers are registered for the app's lifetime; hide them while they would do
 * nothing (no tab to close, nothing to reopen, no connection for a new query).
 */
export function actionApplies(id: ActionId, ctx: ActionContext): boolean {
  switch (id) {
    case "tab.new-query":
      return ctx.hasConnection;
    // Without a tab, export.open only reports "Nothing to export here".
    case "tab.close":
    case "tab.close-all":
    case "export.open":
      return ctx.tabCount > 0;
    case "tab.next":
    case "tab.prev":
      return ctx.tabCount > 1;
    case "tab.reopen":
      return ctx.closedCount > 0;
    default:
      return true;
  }
}

/**
 * One item per keymap action that currently has a handler. Actions bound to several combos are
 * listed once, with their first (primary) shortcut.
 */
export function keymapActionItems(
  keymap: readonly ShortcutDef[],
  has: (id: ActionId) => boolean,
  run: (id: ActionId) => unknown,
): PaletteItem[] {
  const seen = new Set<ActionId>();
  const items: PaletteItem[] = [];
  for (const def of keymap) {
    if (seen.has(def.id)) continue;
    seen.add(def.id);
    if (!isListedAction(def.id) || !has(def.id)) continue;
    items.push({
      key: `action:${def.id}`,
      group: "actions",
      label: def.label,
      keywords: [def.id.replace(/[.-]/g, " ")],
      combo: def.keys,
      icon: { type: "action", icon: actionIcon(def.id) },
      run: () => run(def.id),
      verb: "Run",
    });
  }
  return items;
}
