import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as controller from "@/features/editor/controller";
import { getEditorState, seedEditor } from "@/features/editor/editorStates";
import { closeImport, useImportStore } from "@/features/import/store";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockSavedQueries } from "@/lib/ipc/mock/queries";
import type { OpenConnection } from "@/stores/connections";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import { closeSavedQueryDialog, openAddToQueries, useSavedQueryDialogStore } from "./dialogStore";
import { COLLAPSED_FOLDERS_KEY, QueriesPane } from "./QueriesPane";
import { QueriesHost } from "./QueriesHost";
import { resetSavedQueriesStore, useSavedQueriesStore } from "./store";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// The mock library (src/lib/ipc/mock/queries.ts), as Local Postgres sees it:
//   Maintenance/  Active sessions            (Local Postgres only)
//                 Table sizes (MySQL)        (Shop MySQL only: hidden here)
//   Reports/      Posts per user  [ppu]      (Local Postgres only)
//                 User by id      [uid]
//   Recent users  [recent]
function connection(): OpenConnection {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId: "p-local-pg",
    name: "Local Postgres",
    color: null,
    driver: "postgres",
    serverVersion: "17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
  };
  return { info, status: "connected", message: null, currentSchema: "public" };
}

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <QueriesPane connection={connection()} />
      <QueriesHost />
    </QueryClientProvider>,
  );
  // The first render of a file pays for loading the menus and dialogs: allow it some time.
  const row = async (name: string) => screen.findByRole("treeitem", { name }, { timeout: 5000 });
  const names = () => screen.queryAllByRole("treeitem").map((r) => r.getAttribute("aria-label"));
  return { row, names };
}

/** A DataTransfer stand-in (jsdom has none). */
function dataTransfer() {
  const data = new Map<string, string>();
  return {
    setData: (type: string, value: string) => void data.set(type, value),
    getData: (type: string) => data.get(type) ?? "",
    get types() {
      return [...data.keys()];
    },
    dropEffect: "none",
    effectAllowed: "all",
  };
}

const tabs = () => useTabsStore.getState().tabs;

beforeEach(() => {
  closeImport();
  closeSavedQueryDialog();
  resetMockSavedQueries();
  resetSavedQueriesStore();
  resetPrefsForTests();
  localStorage.removeItem(COLLAPSED_FOLDERS_KEY);
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    closeRequest: null,
    queryCounter: 0,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("QueriesPane", () => {
  it("lists folders, then queries, of this connection and of every connection", async () => {
    const { row, names } = setup();
    await row("Recent users");
    expect(names()).toEqual([
      "Maintenance",
      "Active sessions",
      "Reports",
      "Posts per user",
      "User by id",
      "Recent users",
    ]);
    // Counts what the folder holds for this connection; keywords show next to the name.
    expect(within(await row("Maintenance")).getByText("1")).toBeInTheDocument();
    expect(within(await row("Recent users")).getByText("recent")).toBeInTheDocument();
  });

  it("collapses and expands folders, and remembers it", async () => {
    const { row, names } = setup();
    fireEvent.click(await row("Reports"));
    expect(names()).not.toContain("Posts per user");
    expect((await row("Reports")).getAttribute("aria-expanded")).toBe("false");
    expect(JSON.parse(localStorage.getItem(COLLAPSED_FOLDERS_KEY)!)).toEqual(["f-reports"]);
    fireEvent.keyDown(await row("Reports"), { key: "ArrowRight" });
    expect(names()).toContain("Posts per user");
  });

  it("searches names, keywords and SQL, opening the folders of the matches", async () => {
    const { row, names } = setup();
    fireEvent.click(await row("Reports"));
    fireEvent.change(screen.getByRole("textbox", { name: "Search queries" }), {
      target: { value: "uid" },
    });
    expect(names()).toEqual(["Reports", "User by id"]);
    fireEvent.change(screen.getByRole("textbox", { name: "Search queries" }), {
      target: { value: "pg_stat" },
    });
    expect(names()).toEqual(["Maintenance", "Active sessions"]);
    fireEvent.change(screen.getByRole("textbox", { name: "Search queries" }), {
      target: { value: "nothing like it" },
    });
    expect(screen.getByText("No matches")).toBeInTheDocument();
  });

  it("runs a query in a new tab on double-click", async () => {
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const { row } = setup();
    fireEvent.doubleClick(await row("Recent users"));
    expect(tabs()).toHaveLength(1);
    const tab = tabs()[0]!;
    expect(tab.title).toBe("Recent users");
    expect(getEditorState(tab.id)?.doc.toString()).toContain("from users");
    expect(run).toHaveBeenCalledWith(tab.id, "auto", expect.any(Object));
  });

  it("inserts into the SQL editor on double-click with “Double Click to Insert”", async () => {
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const tabId = useTabsStore.getState().newQuery("c1");
    seedEditor(tabId, "-- mine\n");
    setPref("queriesDoubleClickInsert", true);
    const { row } = setup();
    fireEvent.doubleClick(await row("User by id"));
    expect(tabs()).toHaveLength(1);
    expect(getEditorState(tabId)?.doc.toString()).toContain("select * from users where id = :id;");
    expect(run).not.toHaveBeenCalled();
  });

  it("switches double-click to insert from the ⚙ menu", async () => {
    setup();
    fireEvent.pointerDown(screen.getByRole("button", { name: "Queries options" }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.click(
      await screen.findByRole("menuitemcheckbox", { name: "Double Click to Insert" }),
    );
    expect(getPrefs().queriesDoubleClickInsert).toBe(true);
  });

  it("offers the item menu: open, insert, copy, edit", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText: write } });
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const { row } = setup();

    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open in New Tab" }));
    expect(tabs().map((t) => t.title)).toEqual(["Recent users"]);
    expect(run).not.toHaveBeenCalled();

    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Run/ }));
    expect(tabs()).toHaveLength(2);
    expect(run).toHaveBeenCalledTimes(1);

    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Copy/ }));
    await waitFor(() => expect(write).toHaveBeenCalledWith(expect.stringContaining("from users")));

    fireEvent.contextMenu(await row("User by id"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Edit…" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Query" });
    expect(within(dialog).getByLabelText("Keyword")).toHaveValue("uid");
  });

  it("lists Run, Copy, Import…, Open in New Tab, Insert in the item menu, in TablePlus' order", async () => {
    const { row } = setup();
    fireEvent.contextMenu(await row("Recent users"));
    const items = screen.getAllByRole("menuitem").map((m) => (m.textContent ?? "").trim());
    const at = (re: RegExp) => items.findIndex((n) => re.test(n));
    expect(at(/^Run/)).toBe(0);
    expect(at(/^Copy/)).toBe(1);
    expect(at(/^Import…/)).toBe(2);
    expect(at(/^Open in New Tab/)).toBe(3);
    expect(at(/^Insert to SQL Editor/)).toBe(4);
    // Between Copy and Open in New Tab, not a second "Import…" of the folder kind.
    expect(items.filter((n) => n.startsWith("Import…"))).toHaveLength(1);
  });

  it("Import… runs the query's .sql file through the SQL import wizard", async () => {
    const path = vi.spyOn(ipc.savedQueries, "path");
    const { row } = setup();
    fireEvent.contextMenu(await row("User by id"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Import…" }));
    await waitFor(() => expect(useImportStore.getState().request).not.toBeNull());
    expect(path).toHaveBeenCalledWith("q-user-by-id");
    const { request, path: file } = useImportStore.getState();
    // The SQL wizard, on this connection, started on the query's file (no file panel).
    expect(request).toEqual({ connectionId: "c1", kind: "sql", table: null });
    expect(file).toMatch(/User by id\.sql$/);
    // Nothing was opened in the editor and nothing was added to the library.
    expect(tabs()).toHaveLength(0);
  });

  it("says why when the query's file cannot be found", async () => {
    vi.spyOn(ipc.savedQueries, "path").mockRejectedValue(new Error("the query no longer exists"));
    const { row } = setup();
    fireEvent.contextMenu(await row("User by id"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Import…" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(useImportStore.getState().request).toBeNull();
  });

  it("names the folder menu's and the + menu's file import “Add Files to Queries…”", async () => {
    const { row } = setup();
    fireEvent.contextMenu(await row("Reports"));
    expect(screen.getByRole("menuitem", { name: "Add Files to Queries…" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Import…" })).toBeNull();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
  });

  it("renames inline", async () => {
    const save = vi.spyOn(ipc.savedQueries, "save");
    const { row } = setup();
    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename…" }));
    const input = await screen.findByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: "Newest users" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await row("Newest users");
    await waitFor(() =>
      expect(useSavedQueriesStore.getState().queries.map((q) => q.name)).toContain("Newest users"),
    );
    // A rename must not write the pane's copy of the SQL back: the file's own stays.
    expect(save).toHaveBeenCalledOnce();
    expect(save.mock.calls[0]![0]).toMatchObject({
      id: "q-recent-users",
      name: "Newest users",
      keepSql: true,
    });
  });

  it("creates a folder and names it", async () => {
    const { row } = setup();
    await row("Recent users");
    fireEvent.pointerDown(screen.getByRole("button", { name: "New" }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Folder…" }));
    const input = await screen.findByRole("textbox", { name: "Name" });
    expect(input).toHaveValue("New Folder");
    fireEvent.change(input, { target: { value: "Drafts" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await row("Drafts")).toBeInTheDocument();
  });

  it("moves a query into a folder by dragging it, or with Move to Folder", async () => {
    const save = vi.spyOn(ipc.savedQueries, "save");
    const { row } = setup();
    const dt = dataTransfer();
    fireEvent.dragStart(await row("Recent users"), { dataTransfer: dt });
    fireEvent.dragOver(await row("Maintenance"), { dataTransfer: dt });
    fireEvent.drop(await row("Maintenance"), { dataTransfer: dt });
    await waitFor(() =>
      expect(
        useSavedQueriesStore.getState().queries.find((q) => q.id === "q-recent-users")?.folderId,
      ).toBe("f-maintenance"),
    );

    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.pointerMove(screen.getByRole("menuitem", { name: "Move to Folder" }));
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "Move to Folder" }), {
      key: "ArrowRight",
    });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Top Level" }));
    await waitFor(() =>
      expect(
        useSavedQueriesStore.getState().queries.find((q) => q.id === "q-recent-users")?.folderId,
      ).toBeNull(),
    );
    // Both moves keep the file's SQL.
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[0]![0]).toMatchObject({ folderId: "f-maintenance", keepSql: true });
    expect(save.mock.calls[1]![0]).toMatchObject({ folderId: null, keepSql: true });
  });

  it("deletes after asking", async () => {
    const { row, names } = setup();
    fireEvent.keyDown(await row("Reports"), { key: "Delete" });
    const alert = await screen.findByRole("alertdialog");
    expect(alert).toHaveTextContent("Delete the folder “Reports”?");
    expect(alert).toHaveTextContent("The 2 queries inside it will be deleted too.");
    fireEvent.click(within(alert).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(names()).not.toContain("Reports"));
    expect(names()).not.toContain("User by id");
  });
});

describe("Add to Queries popup", () => {
  it("saves name, keyword and SQL with ⌘S, for this connection or all", async () => {
    const { row } = setup();
    await row("Recent users");
    act(() =>
      openAddToQueries({
        sql: "select count(*) from posts;",
        connection: { profileId: "p-local-pg", name: "Local Postgres" },
      }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Add to Queries" });
    expect(within(dialog).getByLabelText("Name")).toHaveValue("select count(*) from posts");
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Post count" } });
    fireEvent.change(within(dialog).getByLabelText("Keyword"), { target: { value: "two words" } });
    expect(dialog).toHaveTextContent("The keyword must be a single word.");
    expect(within(dialog).getByRole("button", { name: /Save/ })).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Keyword"), { target: { value: "pc" } });
    fireEvent.keyDown(within(dialog).getByLabelText("SQL"), { key: "s", metaKey: true });
    await waitFor(() => expect(useSavedQueryDialogStore.getState().request).toBeNull());
    const saved = useSavedQueriesStore.getState().queries.find((q) => q.name === "Post count");
    expect(saved).toMatchObject({
      keyword: "pc",
      sql: "select count(*) from posts;",
      profileId: null,
    });
    expect(await row("Post count")).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith("Added “Post count” to Queries");
  });
});

describe("Queries files", () => {
  it("imports .sql files, adds a folder from disk and shows items in Finder", async () => {
    const { row } = setup();
    await row("Recent users");
    const prompt = vi
      .spyOn(window, "prompt")
      .mockReturnValueOnce("/Users/me/queries/sample.sql, /Users/me/queries/nightly.sql.gz");
    fireEvent.pointerDown(screen.getByRole("button", { name: "New" }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Add Files to Queries…" }));
    expect(await row("sample")).toBeInTheDocument();
    expect(await row("nightly")).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith("Imported 2 queries");

    prompt.mockReturnValueOnce("/Users/me/queries");
    fireEvent.pointerDown(screen.getByRole("button", { name: "New" }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Add Folder to Favorites…" }));
    expect(await row("queries")).toBeInTheDocument();
    expect(await row("orders by day")).toBeInTheDocument();

    const reveal = vi.spyOn(ipc.savedQueries, "reveal");
    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Show in Finder" }));
    expect(reveal).toHaveBeenCalledWith("q-recent-users", false);
    fireEvent.contextMenu(await row("Reports"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Show in Finder" }));
    expect(reveal).toHaveBeenLastCalledWith("f-reports", true);
  });

  it("auto saves an edited query with Auto Save Favorite", async () => {
    setPref("queriesAutoSave", true);
    const save = vi.spyOn(ipc.savedQueries, "save");
    const { row } = setup();
    fireEvent.contextMenu(await row("Recent users"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit…" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Query" });
    expect(dialog).toHaveTextContent("Changes save automatically");
    fireEvent.change(within(dialog).getByLabelText("SQL"), { target: { value: "select 1;" } });
    await waitFor(() => expect(save).toHaveBeenCalledOnce(), { timeout: 3000 });
    expect(save.mock.calls[0]![0]).toMatchObject({ id: "q-recent-users", sql: "select 1;" });
    // The dialog edits the SQL: its saves write it.
    expect(save.mock.calls[0]![0]).not.toHaveProperty("keepSql");
    expect(await within(dialog).findByText("Saved automatically")).toBeInTheDocument();
    // The dialog stays open (Close ends it).
    expect(useSavedQueryDialogStore.getState().request).not.toBeNull();
  });
});
