import { describe, expect, it, vi } from "vitest";
import {
  headerMenu,
  objectMenu,
  plusMenu,
  settingsEntries,
  tableMenu,
  type MenuEntry,
  type TableMenuHandlers,
  type TableMenuOptions,
} from "./menu";

/** Labels of a menu, submenus as `Label ▸ [..]`, separators as `—`. */
function labels(entries: readonly MenuEntry[]): unknown[] {
  return entries.map((e) => {
    switch (e.kind) {
      case "separator":
        return "—";
      case "sub":
        return { [e.label]: labels(e.items) };
      default:
        return e.label;
    }
  });
}

function find(entries: readonly MenuEntry[], id: string): MenuEntry | undefined {
  for (const e of entries) {
    if (e.id === id) return e;
    if (e.kind === "sub") {
      const inner = find(e.items, id);
      if (inner) return inner;
    }
  }
  return undefined;
}

const disabled = (entries: readonly MenuEntry[], id: string) => {
  const e = find(entries, id);
  return e?.kind === "item" ? (e.disabled ?? false) : undefined;
};

function tableHandlers(): TableMenuHandlers {
  return {
    newItem: vi.fn(),
    openInNewTab: vi.fn(),
    openStructure: vi.fn(),
    itemOverview: vi.fn(),
    copyName: vi.fn(),
    togglePin: vi.fn(),
    exportItems: vi.fn(),
    importCsv: vi.fn(),
    importSql: vi.fn(),
    clone: vi.fn(),
    rename: vi.fn(),
    truncate: vi.fn(),
    drop: vi.fn(),
    copyScript: vi.fn(),
  };
}

const TABLE: TableMenuOptions = {
  driver: "postgres",
  readOnly: false,
  count: 1,
  truncatable: 1,
  droppable: 1,
  isTable: true,
  renamable: true,
  allPinned: false,
};

describe("sidebar menus", () => {
  it("lists the table menu in TablePlus's order", () => {
    expect(labels(tableMenu(TABLE, tableHandlers()))).toEqual([
      "Open in New Tab",
      "Open Structure",
      "Item Overview",
      "Copy Name",
      "Pin to Top",
      "—",
      "Export…",
      { Import: ["From CSV…", "From SQL Dump…"] },
      "—",
      {
        New: ["Table…", "View…", "Materialized View…", "Function…", "Procedure…", "Schema…"],
      },
      "Clone…",
      "Rename…",
      "Truncate…",
      "Delete…",
      "—",
      { "Copy Script As": ["CREATION", "DROP", "TRUNCATE"] },
    ]);
  });

  it("counts a multi-selection and disables single-item actions", () => {
    const on = tableHandlers();
    const m = tableMenu({ ...TABLE, count: 3, truncatable: 2, droppable: 3, allPinned: true }, on);
    const l = labels(m);
    expect(l).toContain("Copy 3 Names");
    expect(l).toContain("Unpin from Top");
    expect(l).toContain("Export 3 Items…");
    expect(l).toContain("Truncate 2 Items…");
    expect(l).toContain("Delete 3 Items…");
    expect(disabled(m, "clone")).toBe(true);
    expect(disabled(m, "rename")).toBe(true);
    const del = find(m, "delete");
    if (del?.kind === "item") del.onSelect();
    expect(on.drop).toHaveBeenCalledOnce();
    const script = find(m, "script-truncate");
    if (script?.kind === "item") script.onSelect();
    expect(on.copyScript).toHaveBeenCalledWith("truncate");
  });

  it("keeps views and read-only connections from changing data", () => {
    const view = tableMenu(
      { ...TABLE, isTable: false, renamable: false, truncatable: 0, allPinned: null },
      tableHandlers(),
    );
    expect(labels(view)).not.toContain("Pin to Top");
    expect(disabled(view, "clone")).toBe(true);
    expect(disabled(view, "truncate")).toBe(true);
    expect(disabled(view, "import-csv")).toBe(true);
    expect(disabled(view, "script-truncate")).toBe(true);
    expect(disabled(view, "delete")).toBe(false);

    const ro = tableMenu({ ...TABLE, readOnly: true }, tableHandlers());
    for (const id of ["clone", "rename", "truncate", "delete", "new-table", "new-schema"])
      expect(disabled(ro, id), id).toBe(true);
    expect(disabled(ro, "open-new-tab")).toBe(false);
  });

  it("offers what each driver can create", () => {
    const newOf = (driver: "mysql" | "sqlite") => {
      const sub = find(tableMenu({ ...TABLE, driver }, tableHandlers()), "new");
      return sub?.kind === "sub" ? labels(sub.items) : [];
    };
    expect(newOf("mysql")).toEqual(["Table…", "View…", "Function…", "Procedure…"]);
    expect(newOf("sqlite")).toEqual(["Table…", "View…"]);
  });

  it("lists routine / sequence and header menus", () => {
    const on = {
      newItem: vi.fn(),
      open: vi.fn(),
      copyName: vi.fn(),
      copyScript: vi.fn(),
      drop: vi.fn(),
    };
    const m = objectMenu({ driver: "postgres", readOnly: false, count: 2 }, on);
    expect(labels(m)).toEqual([
      "Open in New Tab",
      "Copy 2 Names",
      { "Copy Script As": ["CREATION", "DROP"] },
      "—",
      { New: ["Table…", "View…", "Materialized View…", "Function…", "Procedure…", "Schema…"] },
      "Delete 2 Items…",
    ]);
    const h = headerMenu(
      { driver: "sqlite", readOnly: false, overview: true },
      { newItem: vi.fn(), itemOverview: vi.fn() },
    );
    expect(labels(h)).toEqual(["Item Overview", "—", { New: ["Table…", "View…"] }]);
  });

  it("builds the + menu with New… items, New Query and Settings", () => {
    const settings = settingsEntries(
      "postgres",
      {
        showRecent: false,
        showFunctions: true,
        showSequences: false,
        showSystemSchemas: false,
        showRowCounts: true,
      },
      vi.fn(),
    );
    expect(labels(plusMenu("postgres", false, settings, { newItem: vi.fn() }, "Mod+T"))).toEqual([
      "New Table…",
      "New View…",
      "New Materialized View…",
      "New Function…",
      "New Procedure…",
      "New Schema…",
      "New Query",
      "—",
      {
        Settings: [
          "Show Recent",
          "Show Function Section",
          "Show Sequences",
          "Show System Schemas",
          "Show row counts",
        ],
      },
    ]);
    const mysql = settingsEntries(
      "mysql",
      {
        showRecent: true,
        showFunctions: true,
        showSequences: true,
        showSystemSchemas: true,
        showRowCounts: false,
      },
      vi.fn(),
    );
    expect(labels(mysql)).toEqual(["Show Recent", "Show Function Section", "Show row counts"]);
    expect(labels(settingsEntries("sqlite", {} as never, vi.fn()))).toEqual([
      "Show Recent",
      "Show row counts",
    ]);
  });
});
