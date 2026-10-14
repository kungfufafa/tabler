import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import { DataGrid } from "@/features/datagrid/DataGrid";
import { registerGridMenuExtension } from "@/features/datagrid/menuExtensions";
import type { GridColumn, ViewRow } from "@/features/datagrid/types";
import { seedEditor } from "@/features/editor/editorStates";
import {
  EditorMenuExtensions,
  registerEditorMenuExtension,
} from "@/features/editor/menuExtensions";
import { registerTableMenuExtension, tableMenu } from "@/features/objects/menu";
import { resetMockPlugins } from "@/lib/ipc/mock/plugins";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import {
  editorPluginMenu,
  gridPluginMenu,
  palettePluginItems,
  sidebarPluginEntries,
} from "./integration";
import { editorInvocation } from "./invocation";
import { refreshPlugins, resetPluginsStoreForTests } from "./store";

const clipboard = vi.hoisted(() => ({ write: vi.fn(async (_text: string) => {}) }));
vi.mock("@/features/datagrid/clipboard", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/datagrid/clipboard")>()),
  writeClipboard: clipboard.write,
}));

const col = (id: string, kind: GridColumn["kind"] = "text"): GridColumn => ({
  id,
  name: id,
  kind,
  dataType: kind,
  nullable: true,
  isPk: id === "id",
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
});
const columns = [col("id", "int"), col("name"), col("site")];
const rows: ViewRow[] = [
  {
    key: "pk:1",
    cells: [
      { t: "int", v: 1 },
      { t: "text", v: "Ann" },
      { t: "text", v: "https://ann.example" },
    ],
  },
];

const sizes: Array<[string, number]> = [
  ["offsetWidth", 1200],
  ["offsetHeight", 600],
  ["clientWidth", 1200],
  ["clientHeight", 600],
];
const saved = new Map<string, PropertyDescriptor | undefined>();
beforeAll(() => {
  for (const [prop, value] of sizes) {
    saved.set(prop, Object.getOwnPropertyDescriptor(HTMLElement.prototype, prop));
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, get: () => value });
  }
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});
afterAll(() => {
  for (const [prop, desc] of saved) {
    if (desc) Object.defineProperty(HTMLElement.prototype, prop, desc);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});

let off: Array<() => void> = [];
beforeEach(() => {
  resetPrefsForTests();
  resetPluginsStoreForTests();
  resetMockPlugins();
  clipboard.write.mockClear();
});
afterEach(() => {
  off.forEach((u) => u());
  off = [];
});

describe("Plugins ▸ in the grid's cell menu", () => {
  it("lists the grid commands and runs one on the clicked row", async () => {
    off.push(registerGridMenuExtension(gridPluginMenu));
    render(
      <DataGrid
        columns={columns}
        rowCount={rows.length}
        getRow={(i) => rows[i]}
        copyContext={{ dialect: "postgres", schema: "public", table: "people" }}
        editable
        active
        onEditCommit={vi.fn()}
      />,
    );
    const cell = document.querySelector<HTMLElement>('[data-cell][data-r="0"][data-c="1"]')!;
    fireEvent.pointerDown(cell, { button: 0 });
    fireEvent.pointerDown(cell, { button: 2 });
    fireEvent.contextMenu(cell);
    const trigger = screen.getByRole("menuitem", { name: /Plugins/ });
    fireEvent.keyDown(trigger, { key: "ArrowRight" });
    const copy = await screen.findByRole("menuitem", { name: "Copy as UPDATE Statement" });
    expect(screen.getByRole("menuitem", { name: "Open URL" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Insert UUID" })).toBeInTheDocument();
    fireEvent.click(copy);
    await waitFor(() =>
      expect(clipboard.write).toHaveBeenCalledWith(
        `UPDATE "public"."people" SET "name" = 'Ann', "site" = 'https://ann.example' WHERE "id" = 1;`,
      ),
    );
  });

  it("adds nothing when no enabled plugin offers a grid command", () => {
    setPref("pluginStates", {
      "tabler.open-url": { enabled: false },
      "tabler.copy-as-update": { enabled: false },
      "tabler.uuid-generator": { enabled: false },
    });
    const context = () => ({
      columns,
      focusedRow: null,
      focusedColumn: null,
      selectedRows: () => [],
    });
    expect(gridPluginMenu(context)).toBeNull();
  });
});

describe("Plugins ▸ in the sidebar's item menu", () => {
  it("appears after the table menu's own entries", async () => {
    setPref("pluginStates", { "com.example.row-counter": { enabled: true, granted: true } });
    await refreshPlugins();
    off.push(registerTableMenuExtension(() => sidebarPluginEntries(null)));
    const noop = () => {};
    const entries = tableMenu(
      {
        driver: "postgres",
        readOnly: false,
        count: 1,
        truncatable: 1,
        droppable: 1,
        isTable: true,
        renamable: true,
        allPinned: false,
      } as Parameters<typeof tableMenu>[0],
      new Proxy({}, { get: () => noop }) as Parameters<typeof tableMenu>[1],
    );
    const plugins = entries[entries.length - 1]!;
    expect(plugins).toMatchObject({ kind: "sub", label: "Plugins" });
    const labels =
      plugins.kind === "sub" ? plugins.items.map((e) => ("label" in e ? e.label : "")) : [];
    expect(labels).toEqual(["Generate Dummy Data…", "Count Rows"]);
  });
});

describe("the palette's plugin commands", () => {
  it('lists them as "Plugin: <name>: <command>"', async () => {
    const labels = palettePluginItems().map((i) => i.label);
    expect(labels).toContain("Plugin: Open URL: Open URL");
    expect(labels).toContain("Plugin: Dummy Data: Generate Dummy Data…");
    // SQL Format is listed through its keymap action (⌃I) instead.
    expect(labels.some((l) => l.includes("SQL Format"))).toBe(false);
  });
});

describe("Plugins ▸ in the SQL editor's context menu", () => {
  const INFO = {
    connectionId: "c-editor",
    profileId: "p1",
    name: "local",
    color: null,
    driver: "postgres",
    serverVersion: "17",
    currentDatabase: "app",
    currentUser: "me",
    tunnel: null,
    readOnly: false,
  } as const;

  function openEditorTab(doc: string): string {
    useConnectionsStore.getState().add(INFO);
    const id = useTabsStore.getState().newQuery(INFO.connectionId);
    seedEditor(id, doc);
    return id;
  }

  afterEach(() => {
    useTabsStore.setState({ tabs: [], activeId: null });
    useConnectionsStore.getState().remove(INFO.connectionId);
  });

  it("hands a command the editor's text, selection and current statement", () => {
    const tabId = openEditorTab("SELECT 1;\nSELECT 2;");
    const inv = editorInvocation(tabId);
    expect(inv).toMatchObject({
      source: "editor",
      connectionId: INFO.connectionId,
      driver: "postgres",
      editorTabId: tabId,
      clickedRow: null,
      selectedRows: [],
    });
    expect(inv.sql).toMatchObject({ text: "SELECT 1;\nSELECT 2;", selected: "" });
    // Not an editor tab: no SQL, nothing to insert into.
    expect(editorInvocation("missing")).toMatchObject({ editorTabId: null, sql: null });
  });

  it("lists the enabled editor commands and runs one on this editor", async () => {
    const tabId = openEditorTab("SELECT 1");
    setPref("pluginStates", {
      "com.example.sandbox-demo": { enabled: true, granted: true },
      "tabler.uuid-generator": { enabled: true },
    });
    await refreshPlugins();
    off.push(registerEditorMenuExtension(editorPluginMenu));
    render(
      <ContextMenu>
        <ContextMenuTrigger>
          <div data-testid="editor">editor</div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <EditorMenuExtensions tabId={tabId} />
        </ContextMenuContent>
      </ContextMenu>,
    );
    fireEvent.contextMenu(screen.getByTestId("editor"));
    const trigger = await screen.findByRole("menuitem", { name: /Plugins/ });
    fireEvent.keyDown(trigger, { key: "ArrowRight" });
    // Commands whose manifest says "editor" (a built-in and an installed one), not the others.
    expect(await screen.findByRole("menuitem", { name: "Insert UUID" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /^SQL Format/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Open Docs" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Copy as UPDATE Statement" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Try the Network" })).toBeNull();

    // Insert UUID writes into the editor the menu was opened on.
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Insert UUID" }));
      await new Promise((r) => setTimeout(r, 50));
    });
    const { getEditorState } = await import("@/features/editor/editorStates");
    await waitFor(() =>
      expect(getEditorState(tabId)?.doc.toString()).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}SELECT 1$/,
      ),
    );
  });

  it("adds nothing when no enabled plugin has an editor command", () => {
    setPref("pluginStates", {
      "tabler.sql-formatter": { enabled: false },
      "tabler.uuid-generator": { enabled: false },
    });
    expect(editorPluginMenu({ tabId: "t" })).toBeNull();
  });

  it("reads the enabled plugins each time the menu opens", async () => {
    const tabId = openEditorTab("SELECT 1");
    setPref("pluginStates", {
      "tabler.sql-formatter": { enabled: false },
      "tabler.uuid-generator": { enabled: false },
    });
    off.push(registerEditorMenuExtension(editorPluginMenu));
    const { unmount } = render(
      <ContextMenu>
        <ContextMenuTrigger>
          <div data-testid="editor">editor</div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <EditorMenuExtensions tabId={tabId} />
        </ContextMenuContent>
      </ContextMenu>,
    );
    fireEvent.contextMenu(screen.getByTestId("editor"));
    await act(async () => {});
    expect(screen.queryByRole("menuitem", { name: /Plugins/ })).toBeNull();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    // Enabled after the first opening: the next one shows it.
    setPref("pluginStates", { "tabler.uuid-generator": { enabled: true } });
    fireEvent.contextMenu(screen.getByTestId("editor"));
    expect(await screen.findByRole("menuitem", { name: /Plugins/ })).toBeInTheDocument();
    unmount();
  });
});
