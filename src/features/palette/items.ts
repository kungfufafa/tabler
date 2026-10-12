import {
  AArrowDown,
  AArrowUp,
  Activity,
  ArrowLeft,
  ArrowRight,
  ArrowUpRight,
  BookmarkPlus,
  Cable,
  Check,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  CircleDashed,
  CircleDot,
  CircleMinus,
  CirclePlay,
  CircleSlash2,
  ClipboardCopy,
  ClipboardPaste,
  Columns2,
  Columns3Cog,
  Copy,
  CopyPlus,
  Database,
  DatabaseZap,
  Eraser,
  Eye,
  FileCode,
  FileDown,
  FileSearch,
  FoldVertical,
  FolderOpen,
  Funnel,
  History,
  LayoutGrid,
  ListTree,
  ListX,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelRightOpen,
  PencilLine,
  Play,
  Plug,
  PlugZap,
  Plus,
  Redo2,
  RefreshCw,
  RotateCcw,
  Save,
  SaveAll,
  ScrollText,
  Search,
  Settings,
  Sigma,
  Square,
  SquareArrowLeft,
  SquareArrowRight,
  SquareChevronRight,
  SquareFunction,
  SquareX,
  Table2,
  TableProperties,
  TextAlignStart,
  Trash2,
  Undo2,
  Unplug,
  UserCog,
  Workflow,
  X,
  type LucideIcon,
} from "lucide-react";
import type {
  ConnectionProfile,
  DatabaseInfo,
  Driver,
  RoutineKind,
  RoutineSummary,
  SavedQuery,
  SchemaInfo,
  TableKind,
  TableSummary,
} from "@/lib/ipc";
import type { ShortcutDef } from "@/lib/keys/keymap";
import type { Tab, TabKind } from "@/stores/tabs";
import type { ActionId } from "./actions";
import {
  fuzzyMatch,
  isWordStart,
  normalizeQuery,
  STRICT_TIER_RANK,
  strictMatch,
  type MatchRange,
} from "./fuzzy";

export type PaletteGroupId =
  "tables" | "routines" | "tabs" | "queries" | "schemas" | "databases" | "connections" | "actions";
/**
 * Grouped sections (⌘K, ⇧⌘K) use their group's id; Open Anything (⌘P) shows one flat list of
 * database objects, or of commands once the query starts with `>`.
 */
export type PaletteSectionId = PaletteGroupId | "objects" | "commands";

/** Group order: of the narrowed lists' sections, and the last tie-break of Open Anything. */
export const GROUP_ORDER: readonly PaletteGroupId[] = [
  "tables",
  "routines",
  "tabs",
  "queries",
  "schemas",
  "databases",
  "connections",
  "actions",
];

/** What Open Anything lists without the `>` prefix. */
export const OBJECT_GROUPS: readonly PaletteGroupId[] = [
  "tables",
  "routines",
  "tabs",
  "queries",
  "schemas",
  "databases",
  "connections",
];

export const SECTION_HEADINGS: Record<PaletteGroupId, string> = {
  tables: "Tables",
  routines: "Functions",
  tabs: "Open tabs",
  queries: "Queries",
  schemas: "Schemas",
  databases: "Databases",
  connections: "Connections",
  actions: "Actions",
};

/** Large schemas can have thousands of tables; only this many are rendered per query. */
export const TABLE_LIMIT = 200;
/** Recently used items listed first while the query is empty. */
export const RECENT_LIMIT = 5;
/** Open Anything lists commands (keymap actions, palette commands) after this prefix. */
export const COMMAND_PREFIX = ">";

export type PaletteIcon =
  | { type: "table"; kind: TableKind }
  | { type: "tab"; kind: TabKind }
  | { type: "connection"; color: string | null; driver: Driver }
  | { type: "database" }
  | { type: "schema" }
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
  /** What the object is ("table", "view", "schema", …), shown on the right in Open Anything. */
  kind?: string;
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
  /** ⌘↩ / ⌘-click alternative, e.g. open a table in a new tab instead of the preview tab. */
  runAlt?: () => void;
  altLabel?: string;
  /**
   * Never recorded as recently used nor listed first as recent (destructive commands such as
   * Disconnect, database switches) so it never becomes the default ↩ target of an empty query.
   */
  noRecent?: boolean;
  /** Right-click menu of the row (⌘K database list). */
  menu?: PaletteMenuEntry[];
}

/** One entry of a row's right-click menu. */
export interface PaletteMenuEntry {
  label: string;
  run: () => unknown;
  destructive?: boolean;
  disabled?: boolean;
  /** Draws a separator above the entry. */
  separated?: boolean;
  /** Runs with the palette left open (Copy Name). */
  keepOpen?: boolean;
}

export interface RankedItem {
  item: PaletteItem;
  /** Highlight ranges within `item.label`. */
  ranges: MatchRange[];
}

export interface PaletteSection {
  id: PaletteSectionId;
  /** Group heading; Open Anything's flat lists have none. */
  heading?: string;
  items: RankedItem[];
  /** Matches that were cut off by the section limit. */
  more: number;
  /** What is still loading ("tables", "databases"…): a placeholder row follows the items. */
  loading?: string;
  /** Rows show their kind on the right instead of their detail (Open Anything's objects). */
  showKind?: boolean;
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
function wordStartScore(query: string, text: string): number | null {
  const m = fuzzyMatch(query, text);
  if (!m || !m.ranges.every(([start]) => isWordStart(text, start))) return null;
  return m.score;
}

/** Loose (subsequence) match of the narrowed lists (⌘K, ⇧⌘K). */
export function matchItem(
  query: string,
  item: PaletteItem,
): { score: number; ranges: MatchRange[] } | null {
  const label = fuzzyMatch(query, item.label);
  if (label) return label;
  let best: number | null = null;
  for (const text of [item.detail, ...(item.keywords ?? [])]) {
    if (!text) continue;
    const score = wordStartScore(query, text);
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

/** Label hits of a strict match rank by tier; these rank below every label hit. */
const SECONDARY_RANK = 0.5;
const TAG_RANK = 0.25;

/**
 * Strict match of Open Anything: the label must match by prefix, word starts or as a contiguous
 * substring (see `strictMatch`). With `secondary` (commands), detail and keyword hits that start
 * words count too, below every label hit. A whole-query tag hit ("view", "schema") ranks last.
 */
export function matchItemStrict(
  query: string,
  item: PaletteItem,
  { secondary = false }: { secondary?: boolean } = {},
): { rank: number; ranges: MatchRange[] } | null {
  const label = strictMatch(query, item.label);
  if (label) return { rank: STRICT_TIER_RANK[label.tier], ranges: label.ranges };
  if (secondary) {
    for (const text of [item.detail, ...(item.keywords ?? [])]) {
      if (!text) continue;
      const m = strictMatch(query, text);
      if (m && m.tier !== "substring") return { rank: SECONDARY_RANK, ranges: [] };
    }
  }
  const q = normalizeQuery(query);
  if (item.tags?.some((tag) => normalizeQuery(tag) === q)) return { rank: TAG_RANK, ranges: [] };
  return null;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const groupIndex = (g: PaletteGroupId) => GROUP_ORDER.indexOf(g);

/** Alphabetical by label, then by group (a table before its open tab), then natural order. */
function compareLabels(
  a: { item: PaletteItem; index: number },
  b: { item: PaletteItem; index: number },
): number {
  return (
    collator.compare(a.item.label, b.item.label) ||
    groupIndex(a.item.group) - groupIndex(b.item.group) ||
    a.index - b.index
  );
}

function capped(
  items: readonly RankedItem[],
  limit: number,
): { items: RankedItem[]; more: number } {
  const shown = items.slice(0, limit);
  return { items: shown, more: items.length - shown.length };
}

/**
 * One flat list for a query: strict matches ordered by tier (exact > prefix > word starts >
 * contiguous), ties broken by recency (MRU), then alphabetically.
 */
export function rankStrict(
  items: readonly PaletteItem[],
  query: string,
  {
    mru = [],
    limit = Infinity,
    secondary = false,
  }: { mru?: readonly string[]; limit?: number; secondary?: boolean } = {},
): { items: RankedItem[]; more: number } {
  const recency = new Map(mru.map((k, i) => [k, i]));
  const matched: Array<RankedItem & { rank: number; index: number }> = [];
  items.forEach((item, index) => {
    const m = matchItemStrict(query, item, { secondary });
    if (m) matched.push({ item, ranges: m.ranges, rank: m.rank, index });
  });
  matched.sort(
    (a, b) =>
      b.rank - a.rank ||
      (recency.get(a.item.key) ?? Infinity) - (recency.get(b.item.key) ?? Infinity) ||
      compareLabels(a, b),
  );
  return capped(
    matched.map(({ item, ranges }) => ({ item, ranges })),
    limit,
  );
}

/**
 * The list of an empty query: up to `recentLimit` recently used items first (MRU order), then the
 * rest alphabetically. `noRecent` items (Disconnect, database switches) come last, so they are
 * never the default ↩ target.
 */
export function browseOrder(
  items: readonly PaletteItem[],
  { mru = [], recentLimit = RECENT_LIMIT }: { mru?: readonly string[]; recentLimit?: number } = {},
): PaletteItem[] {
  const byKey = new Map(items.map((item) => [item.key, item]));
  const recent: PaletteItem[] = [];
  for (const key of mru) {
    if (recent.length >= recentLimit) break;
    const item = byKey.get(key);
    if (item && !item.noRecent) recent.push(item);
  }
  const taken = new Set(recent);
  const rest = items
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => !taken.has(item))
    .sort(
      (a, b) =>
        Number(a.item.noRecent ?? false) - Number(b.item.noRecent ?? false) || compareLabels(a, b),
    )
    .map(({ item }) => item);
  return [...recent, ...rest];
}

/** True when Open Anything's query asks for commands (`>` first, like VS Code). */
export function isCommandQuery(query: string): boolean {
  return query.trimStart().startsWith(COMMAND_PREFIX);
}

/** The part of a command query after the `>` prefix. */
export function commandQuery(query: string): string {
  return query.trimStart().slice(COMMAND_PREFIX.length);
}

/**
 * Inline completion: the part of `label` that follows `query` when the query is a
 * (case-insensitive) prefix of it; empty otherwise.
 */
export function completionOf(query: string, label: string): string {
  if (query === "" || query.length >= label.length || isCommandQuery(query)) return "";
  return label.toLowerCase().startsWith(query.toLowerCase()) ? label.slice(query.length) : "";
}

export interface OpenAnythingOptions {
  mru?: readonly string[];
  /** Groups listed while the query is empty: the active connection's tables (or connections). */
  browse?: readonly PaletteGroupId[];
  /** Groups still loading: a placeholder row follows the list while one of them is listed. */
  loading?: readonly PaletteGroupId[];
  limit?: number;
  recentLimit?: number;
}

/**
 * Open Anything (⌘P), like TablePlus: one flat list without headings.
 * - Empty query: the `browse` groups (the current schema's tables, views…), recently opened ones
 *   first, then alphabetically.
 * - A query: tables, open tabs, schemas, databases and connections in one strictly ranked list.
 * - `>` first: the commands (keymap actions and palette commands) matching the rest.
 */
export function buildOpenAnything(
  groups: Partial<Record<PaletteGroupId, readonly PaletteItem[]>>,
  query: string,
  {
    mru = [],
    browse = ["tables"],
    loading = [],
    limit = TABLE_LIMIT,
    recentLimit = RECENT_LIMIT,
  }: OpenAnythingOptions = {},
): PaletteSection[] {
  const gather = (ids: readonly PaletteGroupId[]) => ids.flatMap((g) => groups[g] ?? []);

  if (isCommandQuery(query)) {
    const rest = commandQuery(query);
    const actions = gather(["actions"]);
    const ranked =
      normalizeQuery(rest) === ""
        ? capped(
            browseOrder(actions, { mru, recentLimit }).map((item) => ({ item, ranges: [] })),
            Infinity,
          )
        : rankStrict(actions, rest, { mru, secondary: true });
    return ranked.items.length > 0 ? [{ id: "commands", ...ranked }] : [];
  }

  const empty = normalizeQuery(query) === "";
  const listed = empty ? browse : OBJECT_GROUPS;
  const pending = listed.find((g) => loading.includes(g));
  const ranked = empty
    ? capped(
        browseOrder(gather(listed), { mru, recentLimit }).map((item) => ({ item, ranges: [] })),
        limit,
      )
    : rankStrict(gather(listed), query, { mru, limit });
  if (ranked.items.length === 0 && pending === undefined) return [];
  return [
    {
      id: "objects",
      ...ranked,
      showKind: true,
      ...(pending !== undefined ? { loading: pending } : {}),
    },
  ];
}

export interface BuildSectionsOptions {
  mru?: readonly string[];
  limits?: Partial<Record<PaletteGroupId, number>>;
  /** Group order; defaults to `GROUP_ORDER`. */
  order?: readonly PaletteGroupId[];
  /** Groups still loading: an empty one becomes a `loading` section in its place. */
  loading?: readonly PaletteGroupId[];
}

/** Builds the headed sections of the narrowed lists (⌘K, ⇧⌘K) in the given group order. */
export function buildSections(
  groups: Partial<Record<PaletteGroupId, readonly PaletteItem[]>>,
  query: string,
  {
    mru = [],
    limits = { tables: TABLE_LIMIT },
    order = GROUP_ORDER,
    loading = [],
  }: BuildSectionsOptions = {},
): PaletteSection[] {
  const sections: PaletteSection[] = [];
  for (const g of order) {
    const ranked = rankItems(groups[g] ?? [], query, mru, limits[g] ?? Infinity);
    const heading = SECTION_HEADINGS[g];
    if (ranked.items.length > 0) sections.push({ id: g, heading, ...ranked });
    else if (loading.includes(g))
      sections.push({ id: g, heading, items: [], more: 0, loading: heading.toLowerCase() });
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

/** Typing a whole kind ("view", "materialized view", "table") lists the relations of that kind. */
const KIND_TAGS: Record<TableKind, string[]> = {
  table: ["table"],
  partitioned: ["table", "partitioned", "partitioned table"],
  foreignTable: ["table", "foreign", "foreign table"],
  view: ["view"],
  systemView: ["view", "system view"],
  materializedView: ["view", "materialized view", "matview"],
};

/** The kind shown on the right of a relation in Open Anything. */
export const TABLE_KIND_LABELS: Record<TableKind, string> = {
  table: "table",
  partitioned: "partitioned table",
  foreignTable: "foreign table",
  view: "view",
  systemView: "system view",
  materializedView: "materialized view",
};

/**
 * ↩ shows the table in the connection's preview tab, like a single click in the sidebar
 * (TablePlus Open Anything); ⌘↩ / ⌘-click opens it in a tab of its own. The structure is one
 * ⌃⌘] away once the table shows.
 */
export function tableItems(
  tables: readonly TableSummary[],
  ctx: { profileId: string; open: (table: TableSummary, options: { preview: boolean }) => void },
): PaletteItem[] {
  return tables.map((t) => ({
    key: tableItemKey(ctx.profileId, t),
    group: "tables",
    label: t.name,
    tags: KIND_TAGS[t.kind],
    meta: formatEstimate(t.rowEstimate),
    kind: TABLE_KIND_LABELS[t.kind],
    icon: { type: "table", kind: t.kind },
    run: () => ctx.open(t, { preview: true }),
    verb: "Open",
    runAlt: () => ctx.open(t, { preview: false }),
    altLabel: "New tab",
  }));
}

const ROUTINE_ICONS: Record<RoutineKind, LucideIcon> = {
  function: SquareFunction,
  procedure: Workflow,
  aggregate: Sigma,
  window: SquareFunction,
};

const ROUTINE_KIND_LABELS: Record<RoutineKind, string> = {
  function: "function",
  procedure: "procedure",
  aggregate: "aggregate function",
  window: "window function",
};

/**
 * The schema's functions and procedures (TablePlus Open Anything lists them next to the tables).
 * ↩ opens the definition in a new SQL tab, as a click in the sidebar's Functions section does.
 */
export function routineItems(
  routines: readonly RoutineSummary[],
  ctx: { profileId: string; open: (routine: RoutineSummary) => unknown },
): PaletteItem[] {
  return routines.map((r) => ({
    key: `routine:${enc(ctx.profileId)}:${enc(r.schema)}:${enc(r.name)}:${enc(r.identityArguments)}`,
    group: "routines",
    label: r.name,
    detail: r.identityArguments || undefined,
    tags: r.kind === "procedure" ? ["procedure", "routine"] : ["function", "routine"],
    kind: ROUTINE_KIND_LABELS[r.kind],
    icon: { type: "action", icon: ROUTINE_ICONS[r.kind] },
    run: () => ctx.open(r),
    verb: "Open",
  }));
}

/**
 * Saved queries of the Queries pane (TablePlus b388: Open Anything searches "Queries"). ↩ opens
 * the query in a new SQL tab; ⌘↩ opens and runs it. The binding keyword is searchable.
 */
export function savedQueryItems(
  queries: readonly SavedQuery[],
  ctx: { open: (query: SavedQuery) => unknown; run: (query: SavedQuery) => unknown },
): PaletteItem[] {
  return queries.map((q) => ({
    key: `query:${enc(q.id)}`,
    group: "queries",
    label: q.name,
    detail: q.keyword ?? undefined,
    keywords: q.keyword ? [q.keyword] : undefined,
    tags: ["query", "queries"],
    kind: "query",
    icon: { type: "action", icon: FileCode },
    run: () => ctx.open(q),
    verb: "Open",
    runAlt: () => void ctx.run(q),
    altLabel: "Run",
  }));
}

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
  return tabs.map((tab) => ({
    key: `tab:${tab.id}`,
    group: "tabs",
    label: tab.title,
    meta: ctx.connectionName?.(tab.connectionId) ?? undefined,
    kind: "tab",
    // The icon tells data, structure and query tabs apart.
    icon: { type: "tab", kind: tab.kind },
    dirty: ctx.isDirty?.(tab) ?? false,
    current: tab.id === ctx.activeId,
    run: () => ctx.activate(tab),
    verb: "Switch to tab",
  }));
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
      kind: "connection",
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

/** Typing the whole word lists every schema / database. */
const SCHEMA_TAGS = ["schema", "schemas", "switch schema"];
const DATABASE_TAGS = ["database", "databases", "db", "switch database"];

/** PostgreSQL schemas / SQLite attached databases of the active connection. */
export function schemaItems(
  schemas: readonly SchemaInfo[],
  ctx: {
    profileId: string;
    /** Database the schemas belong to (part of the MRU key). */
    database: string | null;
    current: string | null;
    select: (schema: string) => void;
    /** The row's right-click menu. */
    menu?: (schema: string) => PaletteMenuEntry[];
  },
): PaletteItem[] {
  return schemas.map((s) => ({
    key: `schema:${enc(ctx.profileId)}:${enc(ctx.database)}:${enc(s.name)}`,
    group: "schemas",
    label: s.name,
    tags: SCHEMA_TAGS,
    meta: s.isDefault ? "default" : undefined,
    kind: "schema",
    icon: { type: "schema" },
    current: s.name === ctx.current,
    run: () => ctx.select(s.name),
    verb: "Switch",
    ...(ctx.menu ? { menu: ctx.menu(s.name) } : {}),
  }));
}

/** Databases of the active connection's server (PostgreSQL, MySQL). */
export function databaseItems(
  databases: readonly DatabaseInfo[],
  ctx: {
    profileId: string;
    /** The connection's current database (`isCurrent` of the list may lag behind a switch). */
    current: string | null;
    /** Database a switch is on its way to. */
    switching?: string | null;
    switchTo: (database: string) => unknown;
    /** The row's right-click menu. */
    menu?: (database: string, current: boolean) => PaletteMenuEntry[];
  },
): PaletteItem[] {
  return databases.map((d) => {
    const current = ctx.current !== null ? d.name === ctx.current : d.isCurrent;
    return {
      key: `db:${enc(ctx.profileId)}:${enc(d.name)}`,
      group: "databases",
      label: d.name,
      tags: DATABASE_TAGS,
      meta: d.name === ctx.switching ? "Switching…" : undefined,
      kind: "database",
      icon: { type: "database" },
      current,
      run: () => ctx.switchTo(d.name),
      verb: "Switch",
      // Switching closes every tab of the connection: never the ↩ default of an empty query.
      noRecent: true,
      ...(ctx.menu ? { menu: ctx.menu(d.name, current) } : {}),
    };
  });
}

/**
 * The "+ New Database…" / "+ New Schema…" row at the end of the ⌘K list. Never a Recent default,
 * and listed for "new", "create" or "+".
 */
export function createItem(
  group: "databases" | "schemas",
  ctx: { profileId: string; run: () => unknown },
): PaletteItem {
  const what = group === "databases" ? "Database" : "Schema";
  return {
    key: `${group === "databases" ? "db-new" : "schema-new"}:${enc(ctx.profileId)}`,
    group,
    label: `New ${what}…`,
    keywords: [`create add + new ${what.toLowerCase()}`],
    icon: { type: "action", icon: Plus },
    run: ctx.run,
    verb: "Create",
    noRecent: true,
  };
}

const ACTION_ICONS: Partial<Record<ActionId, LucideIcon>> = {
  "tab.new-query": Plus,
  "tab.close": X,
  "tab.close-all": SquareX,
  "tab.reopen": RotateCcw,
  "tab.next": ChevronRight,
  "tab.prev": ChevronLeft,
  "palette.open": Search,
  "palette.databases": Database,
  "palette.connections": Cable,
  "settings.open": Settings,
  "ui.toggle-sidebar": PanelLeft,
  "ui.toggle-inspector": PanelRight,
  "ui.toggle-console": ScrollText,
  "ui.launcher": Plug,
  "sidebar.focus-search": Search,
  "view.refresh": RefreshCw,
  "view.data": Table2,
  "view.structure": TableProperties,
  "connection.reconnect": PlugZap,
  "grid.commit": Check,
  "grid.preview": FileSearch,
  "grid.discard": Eraser,
  "grid.filter": Funnel,
  "grid.columns": Columns3Cog,
  "grid.new-row": Plus,
  "grid.delete-rows": Trash2,
  "grid.duplicate-row": CopyPlus,
  "grid.set-null": CircleSlash2,
  "grid.set-empty": CircleDashed,
  "grid.set-default": CircleDot,
  "grid.open-fk": ArrowUpRight,
  "grid.undo": Undo2,
  "grid.redo": Redo2,
  "grid.toggle-detail": PanelRightOpen,
  "grid.copy": Copy,
  "grid.copy-as": ClipboardCopy,
  "grid.paste": ClipboardPaste,
  "query.run": Play,
  "query.run-current": CirclePlay,
  "query.cancel": Square,
  "query.explain": ListTree,
  "query.format": TextAlignStart,
  "query.minify": FoldVertical,
  "query.toggle-results": PanelBottom,
  "query.history": History,
  "query.save": Save,
  "query.open": FolderOpen,
  "export.open": FileDown,
  "connection.new": Plus,
  "admin.process-list": Activity,
  "admin.users": UserCog,
  "admin.new-database": DatabaseZap,
  "admin.drop-database": CircleMinus,
  "admin.new-schema": Plus,
  "admin.drop-schema": ListX,
  "connection.disconnect": Unplug,
  "grid.quick-look": Eye,
  "grid.set-value": PencilLine,
  "grid.prev-page": ChevronsLeft,
  "grid.next-page": ChevronsRight,
  "query.add-to-queries": BookmarkPlus,
  "query.save-selection": SaveAll,
  "query.font-bigger": AArrowUp,
  "query.font-smaller": AArrowDown,
  "query.split": Columns2,
  "workspace.show": LayoutGrid,
  "workspace.next": SquareArrowRight,
  "workspace.prev": SquareArrowLeft,
  "tab.back": ArrowLeft,
  "tab.forward": ArrowRight,
};

export function actionIcon(id: ActionId): LucideIcon {
  return ACTION_ICONS[id] ?? SquareChevronRight;
}

/**
 * The palette's own openers (⌘P, ⌘K, ⇧⌘K; their lists are the Databases / Connections groups) and
 * the per-index tab jumps (open tabs are listed as objects) are not listed.
 */
export function isListedAction(id: ActionId): boolean {
  // The filter bar's keys only mean something while one of its fields has the focus.
  return !id.startsWith("palette.") && !id.startsWith("tab.goto-") && !id.startsWith("filter.");
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
