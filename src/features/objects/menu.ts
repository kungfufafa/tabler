import {
  ChevronsDownUp,
  Code,
  Copy,
  CopyPlus,
  DatabaseZap,
  Download,
  Eraser,
  Eye,
  FileCode2,
  Folder,
  FolderMinus,
  FolderPen,
  FolderPlus,
  Info,
  Layers,
  PencilLine,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  SquareFunction,
  SquarePlus,
  SquareStack,
  SquareTerminal,
  TableProperties,
  Trash2,
  Upload,
  Waypoints,
  Workflow,
  type LucideIcon,
} from "lucide-react";
import { coreDialect } from "@/features/drivers/catalog";
import type { Driver } from "@/lib/ipc";
import {
  isMatchStrategy,
  MATCH_STRATEGIES,
  MATCH_STRATEGY_LABELS,
  type MatchStrategy,
} from "./search";
import { supportsNewObject, type NewObjectKind } from "./templates";

/**
 * Menus as data, so one model renders as a context menu (sidebar rows and headers) or a dropdown
 * (the sidebar "+" button), and tests can check labels, order and enabled state.
 */
export type MenuEntry =
  | {
      kind: "item";
      id: string;
      label: string;
      icon?: LucideIcon;
      /** Combo in KEYMAP spelling (`Alt+Backspace`). */
      shortcut?: string;
      disabled?: boolean;
      destructive?: boolean;
      onSelect: () => void;
    }
  | { kind: "sub"; id: string; label: string; icon?: LucideIcon; items: MenuEntry[] }
  | {
      kind: "check";
      id: string;
      label: string;
      checked: boolean;
      onChange: (checked: boolean) => void;
    }
  | {
      /** One choice of several (a radio group), e.g. Matching Strategy ▸ Fuzzy / Contains…. */
      kind: "radio";
      id: string;
      value: string;
      options: ReadonlyArray<{ value: string; label: string }>;
      onChange: (value: string) => void;
    }
  | { kind: "separator"; id: string }
  | { kind: "label"; id: string; label: string };

const sep = (id: string): MenuEntry => ({ kind: "separator", id });

/** What the "New ▸" submenu (and the "+" menu) can create. */
export type NewTarget = "table" | NewObjectKind | "schema" | "query" | "group";

export interface NewMenuHandlers {
  newItem: (what: NewTarget) => void;
}

/** "New ▸" entries for a driver: Table / View / Materialized View (PG) / Function / Procedure
 * (PG, MySQL) / Schema (PG), and Group… when the sidebar can keep groups (`group`). */
export function newEntries(
  driver: Driver,
  readOnly: boolean,
  on: NewMenuHandlers,
  suffix = "…",
  { group = false }: { group?: boolean } = {},
): MenuEntry[] {
  const out: MenuEntry[] = [
    {
      kind: "item",
      id: "new-table",
      label: `Table${suffix}`,
      icon: SquarePlus,
      disabled: readOnly,
      onSelect: () => on.newItem("table"),
    },
  ];
  const kinds: Array<[NewObjectKind, string, LucideIcon]> = [
    ["view", "View", Eye],
    ["materializedView", "Materialized View", Layers],
    ["function", "Function", SquareFunction],
    ["procedure", "Procedure", Workflow],
  ];
  for (const [kind, label, icon] of kinds)
    if (supportsNewObject(coreDialect(driver), kind))
      out.push({
        kind: "item",
        id: `new-${kind}`,
        label: `${label}${suffix}`,
        icon,
        onSelect: () => on.newItem(kind),
      });
  if (driver === "postgres")
    out.push({
      kind: "item",
      id: "new-schema",
      label: `Schema${suffix}`,
      icon: FolderPlus,
      disabled: readOnly,
      onSelect: () => on.newItem("schema"),
    });
  if (group)
    out.push({
      kind: "item",
      id: "new-group",
      label: `Group${suffix}`,
      icon: Folder,
      onSelect: () => on.newItem("group"),
    });
  return out;
}

/** "New ▸" (with Group… when `group`: TablePlus's New ▸ Group…, spec §2.3). */
export function newSubmenu(
  driver: Driver,
  readOnly: boolean,
  on: NewMenuHandlers,
  group = false,
): MenuEntry {
  return {
    kind: "sub",
    id: "new",
    label: "New",
    icon: Plus,
    items: newEntries(driver, readOnly, on, "…", { group }),
  };
}

// --- tables / views ------------------------------------------------------------------------------

export interface TableMenuOptions {
  driver: Driver;
  readOnly: boolean;
  /** How many items the menu acts on (the selection, or just the clicked row). */
  count: number;
  /** Tables (not views) among them. */
  truncatable: number;
  /** Items among them that can be dropped (not system views). */
  droppable: number;
  /** The clicked item is a table (not a view): Clone / Rename / Import / TRUNCATE script. */
  isTable: boolean;
  /** The clicked item can be renamed. */
  renamable: boolean;
  /** Every target is pinned already ("Unpin from Top"); null hides the pin entry. */
  allPinned: boolean | null;
  /** New ▸ offers Group… (the sidebar keeps groups for this connection). */
  groups?: boolean;
}

export interface TableMenuHandlers extends NewMenuHandlers {
  openInNewTab: () => void;
  openStructure: () => void;
  itemOverview: () => void;
  copyName: () => void;
  togglePin: () => void;
  exportItems: () => void;
  importCsv: () => void;
  importJson: () => void;
  importSql: () => void;
  clone: () => void;
  rename: () => void;
  truncate: () => void;
  drop: () => void;
  copyScript: (kind: "creation" | "drop" | "truncate" | "laravel") => void;
  /** Generate Diagram… of the targets. */
  generateDiagram: () => void;
}

const many = (n: number) => (n > 1 ? ` ${n} Items` : "");

/** Entries other features add at the end of the table / view menu (Plugins ▸). */
const tableMenuExtensions = new Set<(o: TableMenuOptions) => MenuEntry[]>();

/** Adds entries to every table / view menu. Returns the function that removes them. */
export function registerTableMenuExtension(extend: (o: TableMenuOptions) => MenuEntry[]) {
  tableMenuExtensions.add(extend);
  return () => void tableMenuExtensions.delete(extend);
}

/**
 * The table / view context menu, in TablePlus's order (spec §2.4). `groupItems` (sidebar groups,
 * `groupEntries`) follow the pin entry.
 */
export function tableMenu(
  o: TableMenuOptions,
  on: TableMenuHandlers,
  groupItems: readonly MenuEntry[] = [],
): MenuEntry[] {
  const entries: MenuEntry[] = [
    {
      kind: "item",
      id: "open-new-tab",
      label: "Open in New Tab",
      icon: SquareStack,
      onSelect: on.openInNewTab,
    },
    {
      kind: "item",
      id: "open-structure",
      label: "Open Structure",
      icon: TableProperties,
      onSelect: on.openStructure,
    },
    {
      kind: "item",
      id: "item-overview",
      label: "Item Overview",
      icon: Info,
      onSelect: on.itemOverview,
    },
    {
      kind: "item",
      id: "copy-name",
      label: o.count > 1 ? `Copy ${o.count} Names` : "Copy Name",
      icon: Copy,
      onSelect: on.copyName,
    },
  ];
  if (o.allPinned !== null)
    entries.push({
      kind: "item",
      id: "pin",
      label: o.allPinned ? "Unpin from Top" : "Pin to Top",
      icon: o.allPinned ? PinOff : Pin,
      onSelect: on.togglePin,
    });
  entries.push(...groupItems);
  entries.push(
    sep("s1"),
    {
      kind: "item",
      id: "export",
      label: `Export${many(o.count)}…`,
      icon: Download,
      onSelect: on.exportItems,
    },
    {
      kind: "sub",
      id: "import",
      label: "Import",
      icon: Upload,
      items: [
        {
          kind: "item",
          id: "import-csv",
          label: "From CSV…",
          disabled: o.readOnly || !o.isTable,
          onSelect: on.importCsv,
        },
        {
          kind: "item",
          id: "import-json",
          label: "From JSON…",
          disabled: o.readOnly || !o.isTable,
          onSelect: on.importJson,
        },
        {
          kind: "item",
          id: "import-sql",
          label: "From SQL Dump…",
          disabled: o.readOnly,
          onSelect: on.importSql,
        },
      ],
    },
    sep("s2"),
    newSubmenu(o.driver, o.readOnly, on, o.groups),
    {
      kind: "item",
      id: "clone",
      label: "Clone…",
      icon: CopyPlus,
      disabled: o.readOnly || !o.isTable || o.count > 1,
      onSelect: on.clone,
    },
    {
      kind: "item",
      id: "rename",
      label: "Rename…",
      icon: PencilLine,
      disabled: o.readOnly || !o.renamable || o.count > 1,
      onSelect: on.rename,
    },
    {
      kind: "item",
      id: "truncate",
      label: `Truncate${many(o.truncatable)}…`,
      icon: Eraser,
      shortcut: "Alt+Backspace",
      disabled: o.readOnly || o.truncatable === 0,
      onSelect: on.truncate,
    },
    {
      kind: "item",
      id: "delete",
      label: `Delete${many(o.droppable)}…`,
      icon: Trash2,
      shortcut: "Backspace",
      destructive: true,
      disabled: o.readOnly || o.droppable === 0,
      onSelect: on.drop,
    },
    sep("s3"),
    {
      kind: "sub",
      id: "copy-script",
      label: "Copy Script As",
      icon: FileCode2,
      items: [
        {
          kind: "item",
          id: "script-creation",
          label: "CREATION",
          onSelect: () => on.copyScript("creation"),
        },
        {
          kind: "item",
          id: "script-drop",
          label: "DROP",
          onSelect: () => on.copyScript("drop"),
        },
        {
          kind: "item",
          id: "script-truncate",
          label: "TRUNCATE",
          disabled: !o.isTable,
          onSelect: () => on.copyScript("truncate"),
        },
        {
          kind: "item",
          id: "script-laravel",
          label: "Laravel Migration",
          disabled: o.truncatable === 0,
          onSelect: () => on.copyScript("laravel"),
        },
      ],
    },
    {
      kind: "item",
      id: "generate-diagram",
      label: "Generate Diagram…",
      icon: Waypoints,
      onSelect: on.generateDiagram,
    },
  );
  for (const extend of tableMenuExtensions) entries.push(...extend(o));
  return entries;
}

// --- sidebar groups ------------------------------------------------------------------------------

export interface GroupMenuOptions {
  /** The manual groups (no filter rules) items can be added to. */
  manual: ReadonlyArray<{ id: string; name: string }>;
  /** The manual group the clicked row is listed in, if any. */
  current: { id: string; name: string } | null;
}

export interface GroupMenuHandlers {
  addToGroup: (groupId: string) => void;
  removeFromGroup: () => void;
  newGroup: () => void;
}

/** "Add to Group ▸ <groups> / New Group…" and "Remove from Group" of an item row. */
export function groupEntries(o: GroupMenuOptions, on: GroupMenuHandlers): MenuEntry[] {
  const targets = o.manual.filter((g) => g.id !== o.current?.id);
  const items: MenuEntry[] = targets.map((g) => ({
    kind: "item",
    id: `group-${g.id}`,
    label: g.name,
    icon: Folder,
    onSelect: () => on.addToGroup(g.id),
  }));
  if (items.length > 0) items.push(sep("group-sep"));
  items.push({
    kind: "item",
    id: "group-new",
    label: "New Group…",
    icon: FolderPlus,
    onSelect: on.newGroup,
  });
  const out: MenuEntry[] = [
    { kind: "sub", id: "add-to-group", label: "Add to Group", icon: Folder, items },
  ];
  if (o.current)
    out.push({
      kind: "item",
      id: "remove-from-group",
      label: `Remove from ${o.current.name}`,
      icon: FolderMinus,
      onSelect: on.removeFromGroup,
    });
  return out;
}

/** Right-click on a group's header: Edit Group…, Delete Group. */
export function groupHeaderMenu(on: { edit: () => void; remove: () => void }): MenuEntry[] {
  return [
    { kind: "item", id: "group-edit", label: "Edit Group…", icon: FolderPen, onSelect: on.edit },
    {
      kind: "item",
      id: "group-delete",
      label: "Delete Group",
      icon: Trash2,
      destructive: true,
      onSelect: on.remove,
    },
  ];
}

// --- routines / sequences ------------------------------------------------------------------------

export interface ObjectMenuOptions {
  driver: Driver;
  readOnly: boolean;
  count: number;
  /** How many of them Delete… acts on (not already pending a drop); `count` when left out. */
  droppable?: number;
  /** New ▸ offers Group…. */
  groups?: boolean;
}

export interface ObjectMenuHandlers extends NewMenuHandlers {
  open: () => void;
  copyName: () => void;
  copyScript: (kind: "creation" | "drop") => void;
  drop: () => void;
}

/**
 * Functions / procedures and sequences: open, copy, scripts, delete (spec §2.4 last item).
 * `groupItems` (sidebar groups, `groupEntries`) follow Copy Name.
 */
export function objectMenu(
  o: ObjectMenuOptions,
  on: ObjectMenuHandlers,
  groupItems: readonly MenuEntry[] = [],
): MenuEntry[] {
  return [
    { kind: "item", id: "open-new-tab", label: "Open in New Tab", icon: Code, onSelect: on.open },
    {
      kind: "item",
      id: "copy-name",
      label: o.count > 1 ? `Copy ${o.count} Names` : "Copy Name",
      icon: Copy,
      onSelect: on.copyName,
    },
    ...groupItems,
    {
      kind: "sub",
      id: "copy-script",
      label: "Copy Script As",
      icon: FileCode2,
      items: [
        {
          kind: "item",
          id: "script-creation",
          label: "CREATION",
          onSelect: () => on.copyScript("creation"),
        },
        {
          kind: "item",
          id: "script-drop",
          label: "DROP",
          onSelect: () => on.copyScript("drop"),
        },
      ],
    },
    sep("s1"),
    newSubmenu(o.driver, o.readOnly, on, o.groups),
    {
      kind: "item",
      id: "delete",
      label: `Delete${many(o.droppable ?? o.count)}…`,
      icon: Trash2,
      shortcut: "Backspace",
      destructive: true,
      disabled: o.readOnly || o.droppable === 0,
      onSelect: on.drop,
    },
  ];
}

/**
 * Right-click on a section header: Item Overview and Generate Diagram… (the Tables header) and
 * New ▸.
 */
export function headerMenu(
  o: { driver: Driver; readOnly: boolean; overview: boolean; diagram?: boolean; groups?: boolean },
  on: NewMenuHandlers & { itemOverview: () => void; generateDiagram?: () => void },
): MenuEntry[] {
  const entries: MenuEntry[] = [];
  if (o.overview)
    entries.push({
      kind: "item",
      id: "item-overview",
      label: "Item Overview",
      icon: Info,
      onSelect: on.itemOverview,
    });
  if (o.diagram && on.generateDiagram)
    entries.push({
      kind: "item",
      id: "generate-diagram",
      label: "Generate Diagram…",
      icon: Waypoints,
      onSelect: on.generateDiagram,
    });
  if (entries.length > 0) entries.push(sep("s1"));
  entries.push(newSubmenu(o.driver, o.readOnly, on, o.groups));
  return entries;
}

/**
 * Lazy load: right-click on a schema (PostgreSQL) / database (MySQL) node: Item Overview,
 * Generate Diagram…, Copy Name, Reload Items, Collapse All, New ▸.
 */
export function schemaNodeMenu(
  o: { driver: Driver; readOnly: boolean; loaded: boolean; diagram: boolean; groups?: boolean },
  on: NewMenuHandlers & {
    itemOverview: () => void;
    generateDiagram: () => void;
    copyName: () => void;
    reload: () => void;
    collapseAll: () => void;
  },
): MenuEntry[] {
  const entries: MenuEntry[] = [
    {
      kind: "item",
      id: "item-overview",
      label: "Item Overview",
      icon: Info,
      onSelect: on.itemOverview,
    },
  ];
  if (o.diagram)
    entries.push({
      kind: "item",
      id: "generate-diagram",
      label: "Generate Diagram…",
      icon: Waypoints,
      onSelect: on.generateDiagram,
    });
  entries.push(
    { kind: "item", id: "copy-name", label: "Copy Name", icon: Copy, onSelect: on.copyName },
    sep("s1"),
    {
      kind: "item",
      id: "reload",
      label: "Reload Items",
      icon: RefreshCw,
      disabled: !o.loaded,
      onSelect: on.reload,
    },
    {
      kind: "item",
      id: "collapse-all",
      label: "Collapse All",
      icon: ChevronsDownUp,
      onSelect: on.collapseAll,
    },
    sep("s2"),
    newSubmenu(o.driver, o.readOnly, on, o.groups),
  );
  return entries;
}

// --- the "+" menu and its settings ---------------------------------------------------------------

export interface SidebarSettings {
  showRecent: boolean;
  showFunctions: boolean;
  showSequences: boolean;
  showSystemSchemas: boolean;
  showRowCounts: boolean;
  /** Each table / view row expands to its columns. */
  showColumns: boolean;
  /** "Lazy Load Schema Items" of the connection; null where it does not apply (SQLite). */
  lazyLoad?: boolean | null;
}

export type SidebarSettingKey = keyof SidebarSettings;

/**
 * Sidebar Settings ▸ checkboxes, in TablePlus's words (sequences and system schemas are
 * PostgreSQL's; lazy loading PostgreSQL's and MySQL's).
 */
export function settingsEntries(
  driver: Driver,
  s: SidebarSettings,
  set: (key: SidebarSettingKey, value: boolean) => void,
): MenuEntry[] {
  const check = (id: SidebarSettingKey, label: string): MenuEntry => ({
    kind: "check",
    id,
    label,
    checked: s[id] === true,
    onChange: (v) => set(id, v),
  });
  const out = [check("showRecent", "Show Recently Section")];
  if (driver !== "sqlite") out.push(check("showFunctions", "Show Function Section"));
  if (driver === "postgres")
    out.push(
      check("showSequences", "Show Sequences"),
      check("showSystemSchemas", "Show System Schemas"),
    );
  if (s.lazyLoad !== undefined && s.lazyLoad !== null)
    out.push(check("lazyLoad", "Lazy Load Schema Items"));
  out.push(check("showRowCounts", "Show Row Counts"), check("showColumns", "Show Columns"));
  return out;
}

/** The search field menu's Settings ▸ (TablePlus: recently section, system schemas, lazy load). */
export function searchSettingsEntries(
  driver: Driver,
  s: Pick<SidebarSettings, "showRecent" | "showSystemSchemas" | "lazyLoad">,
  set: (key: SidebarSettingKey, value: boolean) => void,
): MenuEntry[] {
  const out: MenuEntry[] = [
    {
      kind: "check",
      id: "showRecent",
      label: "Show Recently Section",
      checked: s.showRecent,
      onChange: (v) => set("showRecent", v),
    },
  ];
  if (driver === "postgres")
    out.push({
      kind: "check",
      id: "showSystemSchemas",
      label: "Load System Schemas",
      checked: s.showSystemSchemas,
      onChange: (v) => set("showSystemSchemas", v),
    });
  if (s.lazyLoad !== undefined && s.lazyLoad !== null)
    out.push({
      kind: "check",
      id: "lazyLoad",
      label: "Lazy Load Item in Schema",
      checked: s.lazyLoad,
      onChange: (v) => set("lazyLoad", v),
    });
  return out;
}

/**
 * The sidebar search field's menu (spec §2.3): Search in Database…, Matching Strategy ▸ (Fuzzy /
 * Contains / Prefix / Regex), Settings ▸.
 */
export function searchMenu(
  o: {
    strategy: MatchStrategy;
    settings: MenuEntry[];
    /** Search in Database… needs a connection. */
    canSearchDatabase: boolean;
  },
  on: { searchInDatabase: () => void; setStrategy: (strategy: MatchStrategy) => void },
): MenuEntry[] {
  return [
    {
      kind: "item",
      id: "search-database",
      label: "Search in Database…",
      icon: DatabaseZap,
      disabled: !o.canSearchDatabase,
      onSelect: on.searchInDatabase,
    },
    sep("s1"),
    {
      kind: "sub",
      id: "matching-strategy",
      label: "Matching Strategy",
      items: [
        {
          kind: "radio",
          id: "strategy",
          value: o.strategy,
          options: MATCH_STRATEGIES.map((value) => ({
            value,
            label: MATCH_STRATEGY_LABELS[value],
          })),
          onChange: (v) => {
            if (isMatchStrategy(v)) on.setStrategy(v);
          },
        },
      ],
    },
    { kind: "sub", id: "settings", label: "Settings", items: o.settings },
  ];
}

/** The sidebar "+" menu: New Table… … New Schema…, New Group…, New Query, Settings ▸. */
export function plusMenu(
  driver: Driver,
  readOnly: boolean,
  settings: MenuEntry[],
  on: NewMenuHandlers,
  newQueryShortcut?: string,
): MenuEntry[] {
  const items = newEntries(driver, readOnly, on, "…", { group: true }).map((e) =>
    e.kind === "item" ? { ...e, label: `New ${e.label}` } : e,
  );
  return [
    ...items,
    {
      kind: "item",
      id: "new-query",
      label: "New Query",
      icon: SquareTerminal,
      shortcut: newQueryShortcut,
      onSelect: () => on.newItem("query"),
    },
    sep("s-settings"),
    { kind: "sub", id: "settings", label: "Settings", items: settings },
  ];
}
