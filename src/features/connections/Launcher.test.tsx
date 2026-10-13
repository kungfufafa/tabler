import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { DEMO_IMPORT_PATH, resetMockConnectionFiles } from "@/lib/ipc/mock/connections_io";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useBackupStore } from "@/features/backup/store";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { Launcher } from "./Launcher";
import { ConnectionDialogHost } from "./ConnectionDialogHost";
import { useConnectionsIo } from "./ioStore";
import { useConnectionDialog, useLauncherPrefs } from "./store";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

function renderLauncher() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <Launcher />
        {/* The form is mounted app-wide (AppShell); the launcher opens it through the store. */}
        <ConnectionDialogHost />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const row = async (name: string) =>
  (await screen.findByText(name)).closest<HTMLElement>("[role='option']")!;
const list = () => screen.getByRole("listbox", { name: "Connections" });
/** Profile id of the selected row. */
const selectedId = () =>
  within(list())
    .getByRole("option", { selected: true })
    .id.replace(/^connection-row-/, "");
/** Profile ids of every row drawn as selected. */
const selectedIds = () =>
  within(list())
    .queryAllByRole("option", { selected: true })
    .map((o) => o.id.replace(/^connection-row-/, ""));
/** ⌘<key> on macOS, Ctrl+<key> elsewhere. */
const mod = (key: string) => ({
  key: key.toLowerCase(),
  code: `Key${key.toUpperCase()}`,
  metaKey: isMac,
  ctrlKey: !isMac,
});

function info(connectionId: string, profileId: string): ConnectionInfo {
  return {
    connectionId,
    profileId,
    name: profileId,
    color: null,
    driver: "postgres",
    serverVersion: "17",
    currentDatabase: "postgres",
    currentUser: "postgres",
    tunnel: null,
    readOnly: false,
  };
}

beforeAll(async () => {
  // jsdom lacks these; Radix menus and selects need them.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  await import("@/lib/ipc/mock");
});

describe("Launcher", () => {
  beforeEach(() => {
    resetMockSettings();
    resetMockConnectionFiles();
    useConnectionsIo.setState({ exportScope: null, importPath: null, groupDialog: null });
    useLauncherPrefs.setState({ sortBy: "manual" });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    usePaletteStore.setState({ connecting: [] });
    useUiStore.setState({ view: "launcher" });
    useConnectionDialog.setState({ open: false, profile: null, prefill: null, urlOpen: false });
    useLauncherPrefs.setState({ collapsedGroups: [] });
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.mocked(toast.success).mockReset();
  });

  describe("welcome column", () => {
    it("shows the app name, version and the actions, Backup… and Restore… at the bottom", async () => {
      renderLauncher();
      expect(screen.getByRole("heading", { name: "Tabler" })).toBeInTheDocument();
      expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /Create a new connection/ })).toBeEnabled();
      expect(screen.getByRole("button", { name: /New connection from URL/ })).toBeEnabled();
      expect(screen.getByRole("button", { name: /Import connections/ })).toBeEnabled();
      const bottom = screen.getByRole("navigation", { name: "Backup and restore" });
      expect(within(bottom).getByRole("button", { name: /Backup…/ })).toBeEnabled();
      expect(within(bottom).getByRole("button", { name: /Restore…/ })).toBeEnabled();
    });

    it("Backup… / Restore… open the backup window for the selected connection", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await user.click(screen.getByRole("button", { name: /Backup…/ }));
      expect(useBackupStore.getState().request).toEqual({
        mode: "backup",
        profileId: null,
        database: null,
      });
      fireEvent.click(await row("Shop MySQL"));
      await user.click(screen.getByRole("button", { name: /Restore…/ }));
      expect(useBackupStore.getState().request).toEqual({
        mode: "restore",
        profileId: "p-shop-mysql",
        database: null,
      });
    });

    it("Create a new connection… opens the form at the driver picker", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await user.click(screen.getByRole("button", { name: /Create a new connection/ }));
      expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile: null });
      const form = await screen.findByRole("dialog");
      expect(within(form).getByRole("radiogroup", { name: "Database driver" })).toBeVisible();
    });

    it("New connection from URL… validates inline, then opens the form prefilled", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await user.click(screen.getByRole("button", { name: /New connection from URL/ }));
      const dialog = await screen.findByRole("dialog", { name: "New connection from URL" });
      const field = within(dialog).getByRole("textbox", { name: "Connection URL" });

      await user.type(field, "https://example.com{Enter}");
      expect(within(dialog).getByRole("alert")).toHaveTextContent(/postgres:\/\//);
      expect(useConnectionDialog.getState().open).toBe(false);

      await user.clear(field);
      await user.type(field, "postgresql://app:s3cret@db.example.com:6543/shop?name=Shop{Enter}");
      const form = await screen.findByRole("dialog", { name: /New PostgreSQL connection/ });
      expect(within(form).getByLabelText("Name")).toHaveValue("Shop");
      expect(within(form).getByLabelText("Host")).toHaveValue("db.example.com");
      expect(within(form).getByLabelText("Port")).toHaveValue("6543");
      expect(within(form).getByLabelText("Password")).toHaveValue("s3cret");
      expect(useConnectionDialog.getState()).toMatchObject({ urlOpen: false, open: true });
    });
  });

  describe("connection list", () => {
    it("groups the rows with tag pills and the host : port : user : database line", async () => {
      renderLauncher();
      const pg = await row("Local Postgres");
      expect(within(pg).getByText("local")).toBeInTheDocument();
      expect(within(pg).getByText("127.0.0.1 : 5432 : tabler : tabler_dev")).toBeInTheDocument();
      expect(within(await row("Production (read-only)")).getByText("read-only")).toBeVisible();
      expect(within(await row("Production (read-only)")).getByText("production")).toBeVisible();
      // No tag on profiles saved before tags existed.
      expect(within(await row("Rotated host")).queryByTitle(/^Tag:/)).toBeNull();
      const groups = within(list())
        .getAllByRole("group")
        .map((g) => g.getAttribute("aria-label"));
      expect(groups).toEqual(["Local", "Production", "Staging"]);
    });

    it("lists the connections without a group first, without an “Ungrouped” header", async () => {
      renderLauncher();
      const sqlite = await row("sample.sqlite");
      expect(within(list()).getAllByRole("option")[0]).toBe(sqlite);
      expect(sqlite.closest("[role='group']")).toBeNull();
      expect(screen.queryByText(/ungrouped/i)).toBeNull();
      expect(within(list()).getAllByRole("button", { expanded: true })).toHaveLength(3);
    });

    it("shows no group headers at all when no connection has a group", async () => {
      vi.spyOn(ipc.profiles, "list").mockResolvedValue(
        SEED_PROFILES.map((p) => ({ ...p, group: null })),
      );
      renderLauncher();
      await row("Rotated host");
      expect(within(list()).getAllByRole("option")).toHaveLength(SEED_PROFILES.length);
      expect(within(list()).queryAllByRole("group")).toEqual([]);
      expect(within(list()).queryAllByRole("button", { expanded: true })).toEqual([]);
    });

    it("keeps the ungrouped connections visible even if an older version stored them collapsed", async () => {
      useLauncherPrefs.setState({ collapsedGroups: [""] });
      renderLauncher();
      expect(await row("sample.sqlite")).toBeVisible();
      // The stored preference is cleaned up on load, and "" cannot be collapsed any more.
      const migrate = useLauncherPrefs.persist.getOptions().migrate;
      expect(await migrate?.({ collapsedGroups: ["", "Local"] }, 1)).toEqual({
        collapsedGroups: ["Local"],
        sortBy: "manual",
      });
      expect(await migrate?.({ collapsedGroups: [], sortBy: "tag" }, 2)).toEqual({
        collapsedGroups: [],
        sortBy: "tag",
      });
      useLauncherPrefs.getState().toggleGroup("");
      expect(useLauncherPrefs.getState().collapsedGroups).toEqual([""]);
    });

    it("selects nothing on load: the list only marks its keyboard target", async () => {
      const user = userEvent.setup();
      renderLauncher();
      const first = await row("sample.sqlite");
      expect(list()).toHaveFocus();
      expect(selectedIds()).toEqual([]);
      expect(list()).toHaveAttribute("aria-activedescendant", first.id);
      expect(first).toHaveAttribute("data-current");
      // ⌫ needs a selection.
      await user.keyboard("{Backspace}");
      expect(screen.queryByRole("alertdialog")).toBeNull();
      // A click selects.
      fireEvent.click(await row("Shop MySQL"));
      expect(selectedIds()).toEqual(["p-shop-mysql"]);
      expect(first).not.toHaveAttribute("data-current");
    });

    it("a click selects without connecting; a double-click connects once", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      renderLauncher();
      const el = await row("Local Postgres");
      fireEvent.click(el, { detail: 1 });
      expect(el).toHaveAttribute("aria-selected", "true");
      expect(connect).not.toHaveBeenCalled();
      expect(usePaletteStore.getState().connecting).toEqual([]);
      // The rest of a double-click: a second click, the dblclick, and a stray ↩.
      fireEvent.click(el, { detail: 2 });
      fireEvent.doubleClick(el);
      fireEvent.keyDown(list(), { key: "Enter" });
      expect(usePaletteStore.getState().connecting).toEqual(["p-local-pg"]);
      await vi.waitFor(() => expect(useConnectionsStore.getState().order).toHaveLength(1));
      expect(connect).toHaveBeenCalledTimes(1);
      expect(useUiStore.getState().view).toBe("workspace");
      await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
    });

    it("switches to an already open profile instead of connecting again", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      useConnectionsStore.getState().add(info("old", "p-local-pg"));
      useConnectionsStore.getState().add(info("newer", "p-local-pg"));
      useConnectionsStore.getState().add(info("mysql", "p-shop-mysql"));
      renderLauncher();
      fireEvent.doubleClick(await row("Local Postgres"));
      expect(connect).not.toHaveBeenCalled();
      expect(useConnectionsStore.getState().activeId).toBe("newer");
      expect(useUiStore.getState().view).toBe("workspace");
    });

    it("does not start a connect the palette already has in flight", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      usePaletteStore.getState().beginConnect("p-shop-mysql");
      renderLauncher();
      fireEvent.doubleClick(await row("Shop MySQL"));
      expect(connect).not.toHaveBeenCalled();
    });

    it("releases the guard when the connect fails", async () => {
      vi.spyOn(ipc.connections, "connect").mockRejectedValue(
        Object.assign(new Error("refused"), { code: "CONNECT_FAILED" }),
      );
      renderLauncher();
      fireEvent.doubleClick(await row("Shop MySQL"));
      await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
      expect(useConnectionsStore.getState().order).toEqual([]);
    });

    it("collapses a group from its header (remembered), and searching shows every match", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await row("Rotated host");
      const header = within(list()).getByRole("button", { name: /Staging\s*2/ });
      await user.click(header);
      expect(header).toHaveAttribute("aria-expanded", "false");
      expect(screen.queryByText("Rotated host")).toBeNull();
      expect(useLauncherPrefs.getState().collapsedGroups).toEqual(["Staging"]);
      await user.type(screen.getByRole("textbox", { name: "Search connections" }), "rotated");
      expect(await row("Rotated host")).toHaveAttribute("aria-selected", "true");
      expect(screen.queryByText("Local Postgres")).toBeNull();
    });
  });

  describe("keyboard", () => {
    it("the list has the focus: ↑/↓ move the selection, ↩ connects it", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      await row("Local Postgres");
      expect(list()).toHaveFocus();
      expect(selectedIds()).toEqual([]);
      // The first ↓ selects the first row (the ungrouped SQLite file) instead of skipping it.
      await user.keyboard("{ArrowDown}");
      expect(selectedId()).toBe("p-sample-sqlite");
      await user.keyboard("{ArrowDown}{ArrowDown}");
      expect(selectedId()).toBe("p-shop-mysql");
      await user.keyboard("{ArrowUp}");
      expect(selectedId()).toBe("p-local-pg");
      expect(list()).toHaveAttribute("aria-activedescendant", "connection-row-p-local-pg");
      await user.keyboard("{End}");
      expect(selectedId()).toBe("p-rotated-ssh");
      await user.keyboard("{Home}{ArrowDown}{Enter}");
      expect(connect).toHaveBeenCalledWith("p-local-pg");
      await vi.waitFor(() => expect(useUiStore.getState().view).toBe("workspace"));
    });

    it("⌫ asks before deleting the selected connection", async () => {
      const remove = vi.spyOn(ipc.profiles, "delete").mockResolvedValue(undefined);
      const user = userEvent.setup();
      renderLauncher();
      fireEvent.click(await row("Shop MySQL"));
      await user.keyboard("{Backspace}");
      const confirm = await screen.findByRole("alertdialog");
      expect(within(confirm).getByText("Delete “Shop MySQL”?")).toBeInTheDocument();
      expect(remove).not.toHaveBeenCalled();
      await user.click(within(confirm).getByRole("button", { name: "Delete" }));
      expect(remove).toHaveBeenCalledWith("p-shop-mysql");
    });

    it("⌘F focuses the search; ↓ and ↩ in the search pick a match", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      await row("Local Postgres");
      fireEvent.keyDown(list(), mod("F"));
      const search = screen.getByRole("textbox", { name: "Search connections" });
      expect(search).toHaveFocus();
      await user.type(search, "staging");
      // The tag, group and summary are searched too: both Staging rows match.
      expect(selectedId()).toBe("p-staging-ssh");
      await user.keyboard("{ArrowDown}");
      expect(selectedId()).toBe("p-rotated-ssh");
      await user.keyboard("{Escape}");
      expect(search).toHaveValue("");
      await user.type(search, "shop{Enter}");
      expect(connect).toHaveBeenCalledWith("p-shop-mysql");
      // Let the connect finish inside this test.
      await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
    });

    it("↩ on the focused list connects its marked row even before a selection", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      await row("sample.sqlite");
      await user.keyboard("{Enter}");
      expect(connect).toHaveBeenCalledWith("p-sample-sqlite");
      await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
    });

    it("↩ in an empty search field connects nothing until a row is selected", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      await row("sample.sqlite");
      await user.click(screen.getByRole("textbox", { name: "Search connections" }));
      await user.keyboard("{Enter}");
      expect(connect).not.toHaveBeenCalled();
      expect(selectedIds()).toEqual([]);
      // ↓ from the search field selects the first row; ↩ then connects it.
      await user.keyboard("{ArrowDown}");
      expect(selectedId()).toBe("p-sample-sqlite");
      await user.keyboard("{Enter}");
      expect(connect).toHaveBeenCalledWith("p-sample-sqlite");
      await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
    });

    it("⌘N opens the new-connection form", async () => {
      renderLauncher();
      await row("Local Postgres");
      fireEvent.keyDown(list(), mod("N"));
      expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile: null });
      expect(await screen.findByRole("dialog", { name: "New connection" })).toBeInTheDocument();
    });
  });

  describe("row menus", () => {
    const actionsButton = async (name: string) =>
      within(await row(name)).getByRole("button", { name: "Connection actions" });

    it("↩ on the … button opens the menu without connecting", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      (await actionsButton("Local Postgres")).focus();
      await user.keyboard("{Enter}");
      const menu = await screen.findByRole("menu");
      expect(
        within(menu)
          .getAllByRole("menuitem")
          .map((i) => i.textContent?.trim()),
      ).toEqual([
        "Connect",
        "Edit…",
        "Duplicate",
        "Copy as URL",
        "Move to Group",
        "Export Connections",
        "Import Connections…",
        "Delete…",
      ]);
      expect(usePaletteStore.getState().connecting).toEqual([]);
      expect(connect).not.toHaveBeenCalled();
      expect(useUiStore.getState().view).toBe("launcher");
    });

    it("↩ on Edit… of an open profile opens the form and stays on the launcher", async () => {
      useConnectionsStore.getState().add(info("c1", "p-local-pg"));
      useConnectionsStore.setState({ activeId: null });
      const user = userEvent.setup();
      renderLauncher();
      (await actionsButton("Local Postgres")).focus();
      await user.keyboard("{Enter}");
      const edit = await screen.findByRole("menuitem", { name: /Edit/ });
      edit.focus();
      await user.keyboard("{Enter}");
      expect(useConnectionDialog.getState().open).toBe(true);
      expect(useConnectionDialog.getState().profile?.id).toBe("p-local-pg");
      expect(useUiStore.getState().view).toBe("launcher");
      expect(useConnectionsStore.getState().activeId).toBeNull();
      // Editing skips the driver picker.
      expect(await screen.findByRole("dialog", { name: "Edit Local Postgres" })).toBeVisible();
    });

    it("↩ on Delete… only asks to delete", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      (await actionsButton("Shop MySQL")).focus();
      await user.keyboard("{Enter}");
      const remove = await screen.findByRole("menuitem", { name: /Delete/ });
      remove.focus();
      await user.keyboard("{Enter}");
      expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
      expect(usePaletteStore.getState().connecting).toEqual([]);
      expect(connect).not.toHaveBeenCalled();
    });

    it("right-click selects the row; Copy as URL copies it without the password", async () => {
      const user = userEvent.setup();
      renderLauncher();
      const el = await row("Staging via SSH");
      fireEvent.contextMenu(el);
      expect(el).toHaveAttribute("aria-selected", "true");
      await user.click(await screen.findByRole("menuitem", { name: "Copy as URL" }));
      const url = "postgresql://app@10.0.1.20:5432/app_staging?name=Staging+via+SSH&env=staging";
      await vi.waitFor(() =>
        expect(toast.success).toHaveBeenCalledWith("Copied", { description: url }),
      );
      expect(await navigator.clipboard.readText()).toBe(url);
    });
  });

  describe("groups, sorting, import and export", () => {
    /** Opens the submenu `name` of the open menu (→ on its trigger). */
    const openSub = async (user: ReturnType<typeof userEvent.setup>, name: string) => {
      const trigger = await screen.findByRole("menuitem", { name });
      trigger.focus();
      await user.keyboard("{ArrowRight}");
      return trigger;
    };
    const menuItems = (menu: HTMLElement) =>
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent?.trim());

    it("the … menu offers New ▸, Sort by ▸, Import / Export Connections and Backup / Restore", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await row("Local Postgres");
      await user.click(screen.getByRole("button", { name: "More connection actions" }));
      const menu = await screen.findByRole("menu");
      expect(menuItems(menu)).toEqual([
        "New",
        "Sort by",
        "Import Connections…",
        "Export Connections",
        "Backup…",
        "Restore…",
      ]);
    });

    it("Sort by ▸ Name reorders each group and is remembered", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await row("Rotated host");
      const staging = () =>
        within(screen.getByRole("group", { name: "Staging" }))
          .getAllByRole("option")
          .map((o) => o.textContent);
      expect(staging()[0]).toContain("Staging via SSH");
      fireEvent.contextMenu(screen.getByTestId("connection-list-area"));
      await openSub(user, "Sort by");
      await user.click(await screen.findByRole("menuitemradio", { name: "Name" }));
      expect(useLauncherPrefs.getState().sortBy).toBe("name");
      expect(staging()[0]).toContain("Rotated host");
    });

    it("right-click on empty space ▸ New ▸ Group… creates an empty group", async () => {
      const user = userEvent.setup();
      renderLauncher();
      await row("Local Postgres");
      fireEvent.contextMenu(screen.getByTestId("connection-list-area"));
      await openSub(user, "New");
      await user.click(await screen.findByRole("menuitem", { name: "Group…" }));
      const dialog = await screen.findByRole("dialog", { name: "New Group" });
      const name = within(dialog).getByRole("textbox", { name: "Group name" });
      await user.type(name, "staging{Enter}");
      expect(within(dialog).getByRole("alert")).toHaveTextContent("“staging” already exists");
      await user.clear(name);
      await user.type(name, "Archive{Enter}");
      const header = await within(list()).findByRole("button", { name: /Archive\s*0/ });
      expect(header).toBeInTheDocument();
      expect((await ipc.settings.get()).connectionGroups).toEqual([
        { path: "Archive", icon: null },
      ]);
    });

    it("a row's Move to Group ▸ moves it; No Group takes it out", async () => {
      const move = vi.spyOn(ipc.profiles, "moveToGroup");
      const user = userEvent.setup();
      renderLauncher();
      fireEvent.contextMenu(await row("Local Postgres"));
      await openSub(user, "Move to Group");
      const sub = (await screen.findAllByRole("menu")).at(-1)!;
      expect(menuItems(sub)).toEqual(["Local", "Production", "Staging", "New Group…", "No Group"]);
      expect(within(sub).getByRole("menuitem", { name: "Local" })).toHaveAttribute("data-disabled");
      await user.click(within(sub).getByRole("menuitem", { name: "Production" }));
      await vi.waitFor(() => expect(move).toHaveBeenCalledWith(["p-local-pg"], "Production"));
      await vi.waitFor(() =>
        expect(
          within(screen.getByRole("group", { name: "Production" })).getByText("Local Postgres"),
        ).toBeInTheDocument(),
      );
      fireEvent.contextMenu(await row("Local Postgres"));
      await openSub(user, "Move to Group");
      await user.click(await screen.findByRole("menuitem", { name: "No Group" }));
      await vi.waitFor(() => expect(move).toHaveBeenLastCalledWith(["p-local-pg"], null));
    });

    it("a group header's menu renames and deletes the group", async () => {
      const move = vi.spyOn(ipc.profiles, "moveToGroup");
      const user = userEvent.setup();
      renderLauncher();
      await row("Rotated host");
      fireEvent.contextMenu(within(list()).getByRole("button", { name: /Staging\s*2/ }));
      const menu = await screen.findByRole("menu");
      expect(menuItems(menu)).toEqual([
        "New",
        "New Subgroup…",
        "Rename Group…",
        "Change Icon…",
        "Move to Group",
        "Export Connections",
        "Import Connections…",
        "Delete Group",
      ]);
      await user.click(within(menu).getByRole("menuitem", { name: "Rename Group…" }));
      const dialog = await screen.findByRole("dialog", { name: "Rename Group" });
      const name = within(dialog).getByRole("textbox", { name: "Group name" });
      expect(name).toHaveValue("Staging");
      await user.clear(name);
      await user.type(name, "Pre-prod{Enter}");
      await vi.waitFor(() =>
        expect(move).toHaveBeenCalledWith(["p-staging-ssh", "p-rotated-ssh"], "Pre-prod"),
      );
      const header = await within(list()).findByRole("button", { name: /Pre-prod\s*2/ });
      expect((await ipc.settings.get()).connectionGroups).toEqual([
        { path: "Pre-prod", icon: null },
      ]);

      fireEvent.contextMenu(header);
      await user.click(await screen.findByRole("menuitem", { name: "Delete Group" }));
      await vi.waitFor(() =>
        expect(move).toHaveBeenLastCalledWith(["p-staging-ssh", "p-rotated-ssh"], null),
      );
      await vi.waitFor(() =>
        expect(within(list()).queryByRole("button", { name: /Pre-prod/ })).toBeNull(),
      );
      // Its connections stay, without a group.
      expect((await row("Rotated host")).closest("[role='group']")).toBeNull();
      expect((await ipc.settings.get()).connectionGroups).toEqual([]);
      // Put the seed data back for the other tests.
      await ipc.profiles.moveToGroup(["p-staging-ssh", "p-rotated-ssh"], "Staging");
    });

    describe("nested groups", () => {
      /** The header button of the group `path` (its own name and count). */
      const header = (path: string) =>
        within(screen.getByRole("group", { name: path }))
          .getAllByRole("button")
          .find((b) => b.hasAttribute("aria-expanded"))!;
      const restoreSeedGroups = async () => {
        for (const p of SEED_PROFILES) await ipc.profiles.moveToGroup([p.id], p.group);
      };
      afterEach(restoreSeedGroups);

      it("New Subgroup… nests a group with an icon; counts include the subgroups", async () => {
        const user = userEvent.setup();
        renderLauncher();
        await row("Rotated host");
        fireEvent.contextMenu(header("Staging"));
        await user.click(await screen.findByRole("menuitem", { name: "New Subgroup…" }));
        const dialog = await screen.findByRole("dialog", { name: "New Subgroup" });
        expect(dialog).toHaveTextContent("Inside “Staging”.");
        const name = within(dialog).getByRole("textbox", { name: "Group name" });
        await user.type(name, "EU / West");
        await user.click(within(dialog).getByRole("radio", { name: "Server" }));
        await user.click(within(dialog).getByRole("button", { name: "Create" }));
        expect(within(dialog).getByRole("alert")).toHaveTextContent("use New Subgroup…");
        await user.clear(name);
        await user.type(name, "EU{Enter}");
        await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
        expect((await ipc.settings.get()).connectionGroups).toEqual([
          { path: "Staging / EU", icon: "server" },
        ]);
        const eu = await within(list()).findByRole("group", { name: "Staging / EU" });
        expect(eu.closest("[role='group'][aria-label='Staging']")).not.toBeNull();
        expect(header("Staging / EU")).toHaveTextContent(/EU\s*0/);
        expect(header("Staging / EU").querySelector(".lucide-server")).not.toBeNull();

        // The row menu lists the tree, indented; moving a connection in updates both counts.
        fireEvent.contextMenu(await row("Rotated host"));
        await openSub(user, "Move to Group");
        const sub = (await screen.findAllByRole("menu")).at(-1)!;
        expect(menuItems(sub)).toEqual([
          "Local",
          "Production",
          "Staging",
          "EU",
          "New Group…",
          "No Group",
        ]);
        expect(within(sub).getByRole("menuitem", { name: "EU" })).toHaveStyle({
          paddingLeft: "22px",
        });
        await user.click(within(sub).getByRole("menuitem", { name: "EU" }));
        await vi.waitFor(() => expect(within(eu).getByText("Rotated host")).toBeInTheDocument());
        expect(header("Staging")).toHaveTextContent(/Staging\s*2/);
        expect(header("Staging / EU")).toHaveTextContent(/EU\s*1/);
        // Collapsing the parent hides the subgroup and its rows.
        await user.click(header("Staging"));
        expect(screen.queryByText("Rotated host")).toBeNull();
        expect(within(list()).queryByRole("group", { name: "Staging / EU" })).toBeNull();
        expect(useLauncherPrefs.getState().collapsedGroups).toEqual(["Staging"]);
      });

      it("Move to Group ▸ nests a group (never into itself) and Top Level un-nests it", async () => {
        const move = vi.spyOn(ipc.profiles, "moveToGroup");
        const user = userEvent.setup();
        useLauncherPrefs.setState({ collapsedGroups: ["Production"] });
        renderLauncher();
        await row("Rotated host");
        fireEvent.contextMenu(header("Production"));
        await openSub(user, "Move to Group");
        let sub = (await screen.findAllByRole("menu")).at(-1)!;
        expect(menuItems(sub)).toEqual(["Local", "Production", "Staging", "Top Level"]);
        expect(within(sub).getByRole("menuitem", { name: "Production" })).toHaveAttribute(
          "data-disabled",
        );
        expect(within(sub).getByRole("menuitem", { name: "Top Level" })).toHaveAttribute(
          "data-disabled",
        );
        await user.click(within(sub).getByRole("menuitem", { name: "Staging" }));
        await vi.waitFor(() =>
          expect(move).toHaveBeenCalledWith(["p-prod-pg"], "Staging / Production"),
        );
        await within(list()).findByRole("group", { name: "Staging / Production" });
        expect(header("Staging")).toHaveTextContent(/Staging\s*3/);
        // Its collapsed state followed it.
        expect(useLauncherPrefs.getState().collapsedGroups).toEqual(["Staging / Production"]);

        // Staging cannot go into its own subgroup.
        fireEvent.contextMenu(header("Staging"));
        await openSub(user, "Move to Group");
        sub = (await screen.findAllByRole("menu")).at(-1)!;
        expect(within(sub).getByRole("menuitem", { name: "Production" })).toHaveAttribute(
          "data-disabled",
        );
        await user.keyboard("{Escape}{Escape}");

        fireEvent.contextMenu(header("Staging / Production"));
        await openSub(user, "Move to Group");
        await user.click(await screen.findByRole("menuitem", { name: "Top Level" }));
        await vi.waitFor(() => expect(move).toHaveBeenLastCalledWith(["p-prod-pg"], "Production"));
        await within(list()).findByRole("group", { name: "Production" });
        expect(useLauncherPrefs.getState().collapsedGroups).toEqual(["Production"]);
      });

      it("Delete Group moves its connections and subgroups up one level", async () => {
        const user = userEvent.setup();
        await ipc.settings.set("connectionGroups", [
          { path: "Staging / EU", icon: "cloud" },
          { path: "Staging / EU / Old", icon: null },
        ]);
        await ipc.profiles.moveToGroup(["p-rotated-ssh"], "Staging / EU");
        renderLauncher();
        await row("Rotated host");
        expect(header("Staging")).toHaveTextContent(/Staging\s*2/);
        fireEvent.contextMenu(header("Staging"));
        await user.click(await screen.findByRole("menuitem", { name: "Delete Group" }));
        await within(list()).findByRole("group", { name: "EU" });
        expect(within(list()).queryByRole("group", { name: "Staging" })).toBeNull();
        expect(within(screen.getByRole("group", { name: "EU" })).getByText("Rotated host"));
        // Staging's own connection is now without a group.
        expect((await row("Staging via SSH")).closest("[role='group']")).toBeNull();
        expect((await ipc.settings.get()).connectionGroups).toEqual([
          { path: "EU", icon: "cloud" },
          { path: "EU / Old", icon: null },
        ]);
      });

      it("Change Icon… sets the icon shown on the header", async () => {
        const user = userEvent.setup();
        renderLauncher();
        await row("Local Postgres");
        expect(header("Local").querySelector("svg.lucide-house")).toBeNull();
        fireEvent.contextMenu(header("Local"));
        await user.click(await screen.findByRole("menuitem", { name: "Change Icon…" }));
        const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
        expect(within(dialog).queryByRole("textbox")).toBeNull();
        expect(within(dialog).getByRole("radio", { name: "None" })).toBeChecked();
        await user.click(within(dialog).getByRole("radio", { name: "Home" }));
        await user.click(within(dialog).getByRole("button", { name: "Save" }));
        await vi.waitFor(() =>
          expect(header("Local").querySelector("svg.lucide-house")).not.toBeNull(),
        );
        expect((await ipc.settings.get()).connectionGroups).toEqual([
          { path: "Local", icon: "home" },
        ]);
      });

      it("New ▸ Connection in this group… opens the form with the group filled in", async () => {
        const user = userEvent.setup();
        renderLauncher();
        await row("Rotated host");
        fireEvent.contextMenu(header("Staging"));
        await openSub(user, "New");
        await user.click(
          await screen.findByRole("menuitem", { name: "Connection in this group…" }),
        );
        // Like New ▸ Connection…, it starts at the driver picker and keeps the group for the form.
        const picker = await screen.findByRole("dialog");
        await user.click(within(picker).getByRole("button", { name: "Create" }));
        const form = await screen.findByRole("dialog", { name: /New PostgreSQL connection/ });
        expect(within(form).getByLabelText("Group")).toHaveValue("Staging");
      });

      it("search lists matches flat, with their group path as a breadcrumb", async () => {
        const user = userEvent.setup();
        await ipc.profiles.moveToGroup(["p-rotated-ssh"], "Staging / EU");
        renderLauncher();
        await row("Rotated host");
        await user.type(screen.getByRole("textbox", { name: "Search connections" }), "eu");
        const match = await row("Rotated host");
        expect(within(list()).queryAllByRole("group")).toEqual([]);
        expect(within(match).getByLabelText("Group Staging / EU")).toHaveTextContent("StagingEU");
      });

      it("Export this group… takes the subgroups and their connections along", async () => {
        const user = userEvent.setup();
        await ipc.settings.set("connectionGroups", [{ path: "Staging / Empty", icon: null }]);
        await ipc.profiles.moveToGroup(["p-rotated-ssh"], "Staging / EU");
        renderLauncher();
        await row("Rotated host");
        fireEvent.contextMenu(header("Staging"));
        await openSub(user, "Export Connections");
        await user.click(await screen.findByRole("menuitem", { name: "Export this group…" }));
        expect(useConnectionsIo.getState().exportScope).toEqual({
          title: "Export “Staging”",
          profileIds: ["p-staging-ssh", "p-rotated-ssh"],
          groups: ["Staging", "Staging / Empty", "Staging / EU"],
          fileName: "Staging",
        });
      });
    });

    it("Export Connections ▸ Export this group… opens the export dialog for the group", async () => {
      const user = userEvent.setup();
      renderLauncher();
      fireEvent.contextMenu(await row("Rotated host"));
      await openSub(user, "Export Connections");
      await user.click(await screen.findByRole("menuitem", { name: "Export this group…" }));
      expect(useConnectionsIo.getState().exportScope).toEqual({
        title: "Export “Staging”",
        profileIds: ["p-staging-ssh", "p-rotated-ssh"],
        groups: ["Staging"],
        fileName: "Staging",
      });
      expect(await screen.findByRole("dialog", { name: "Export “Staging”" })).toBeVisible();
    });

    it("Import connections… picks a file, asks its password and imports the ticked ones", async () => {
      vi.spyOn(window, "prompt").mockReturnValue(DEMO_IMPORT_PATH);
      const user = userEvent.setup();
      renderLauncher();
      await row("Local Postgres");
      await user.click(screen.getByRole("button", { name: /Import connections/ }));
      const dialog = await screen.findByRole("dialog", { name: "Import Connections" });
      await user.type(await within(dialog).findByLabelText("File password"), "tabler{Enter}");
      await user.click(
        await within(dialog).findByRole("checkbox", { name: "Import Local Postgres" }),
      );
      await user.click(within(dialog).getByRole("button", { name: "Import" }));
      expect(await row("Team Postgres")).toHaveAttribute("aria-selected", "true");
      expect(await row("Analytics warehouse")).toBeInTheDocument();
      expect(within(list()).getAllByText("Local Postgres")).toHaveLength(1);
      expect(
        within(screen.getByRole("group", { name: "Team" })).getAllByRole("option"),
      ).toHaveLength(2);
      // The file's empty group came along only with a full import: not here.
      expect(within(list()).queryByRole("group", { name: "Archive" })).toBeNull();
      expect(toast.success).toHaveBeenCalledWith("Imported 2 connections", {
        description: "team.tablerconnections",
      });
    });
  });
});
