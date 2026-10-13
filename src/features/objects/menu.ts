import {
  Code,
  Copy,
  CopyPlus,
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
import type { Driver } from "@/lib/ipc";
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
  | { kind: "separator"; id: string }
  | { kind: "label"; id: string; label: string };

const sep = (id: string): MenuEntry => ({ kind: "separator", id });

/** What the "New ▸" submenu (and the "+" menu) can create. */
export type NewTarget = "table" | NewObjectKind | "schema" | "query" | "group";

export interface NewMenuHandlers {
  newItem: (what: NewTarget) => void;
}

/** "New ▸" entries for a driver: Table / View / Materialized View (PG) / Function / Procedure
 * (PG, MySQL) / Schema (PG). */
export function newEntries(
  driver: Driver,
  readOnly: boolean,
  on: NewMenuHandlers,
  suffix = "…",
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
    if (supportsNewObject(driver, kind))
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
  return out;
}

export function newSubmenu(driver: Driver, readOnly: boolean, on: NewMenuHandlers): MenuEntry {
  return {
    kind: "sub",
    id: "new",
    label: "New",
    icon: Plus,
    items: newEntries(driver, readOnly, on),
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
}

export interface TableMenuHandlers extends NewMenuHandlers {
  openInNewTab: () => void;
  openStructure: () => void;
  itemOverview: () => void;
  copyName: () => void;
  togglePin: () => void;
  exportItems: () => void;
  importCsv: () => void;
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
          id: "import-sql",
          label: "From SQL Dump…",
          disabled: o.readOnly,
          onSelect: on.importSql,
        },
      ],
    },
    sep("s2"),
    newSubmenu(o.driver, o.readOnly, on),
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
    newSubmenu(o.driver, o.readOnly, on),
    {
      kind: "item",
      id: "delete",
      label: `Delete${many(o.count)}…`,
      icon: Trash2,
      shortcut: "Backspace",
      destructive: true,
      disabled: o.readOnly,
      onSelect: on.drop,
    },
  ];
}

/**
 * Right-click on a section header: Item Overview and Generate Diagram… (the Tables header) and
 * New ▸.
 */
export function headerMenu(
  o: { driver: Driver; readOnly: boolean; overview: boolean; diagram?: boolean },
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
  entries.push(newSubmenu(o.driver, o.readOnly, on));
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
}

/** Sidebar Settings ▸ checkboxes (sequences and system schemas are PostgreSQL's). */
export function settingsEntries(
  driver: Driver,
  s: SidebarSettings,
  set: <K extends keyof SidebarSettings>(key: K, value: boolean) => void,
): MenuEntry[] {
  const check = (id: keyof SidebarSettings, label: string): MenuEntry => ({
    kind: "check",
    id,
    label,
    checked: s[id],
    onChange: (v) => set(id, v),
  });
  const out = [check("showRecent", "Show Recent")];
  if (driver !== "sqlite") out.push(check("showFunctions", "Show Function Section"));
  if (driver === "postgres")
    out.push(
      check("showSequences", "Show Sequences"),
      check("showSystemSchemas", "Show System Schemas"),
    );
  out.push(check("showRowCounts", "Show row counts"), check("showColumns", "Show Columns"));
  return out;
}

/** The sidebar "+" menu: New Table… … New Schema…, New Group…, New Query, Settings ▸. */
export function plusMenu(
  driver: Driver,
  readOnly: boolean,
  settings: MenuEntry[],
  on: NewMenuHandlers,
  newQueryShortcut?: string,
): MenuEntry[] {
  const items = newEntries(driver, readOnly, on).map((e) =>
    e.kind === "item" ? { ...e, label: `New ${e.label}` } : e,
  );
  return [
    ...items,
    {
      kind: "item",
      id: "new-group",
      label: "New Group…",
      icon: Folder,
      onSelect: () => on.newItem("group"),
    },
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
