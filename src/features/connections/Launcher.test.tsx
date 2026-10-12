import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { Launcher } from "./Launcher";
import { ConnectionDialogHost } from "./ConnectionDialogHost";
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

describe("Launcher", () => {
  beforeEach(() => {
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
    it("shows the app name, version and the actions; Import is not there yet", async () => {
      renderLauncher();
      expect(screen.getByRole("heading", { name: "Tabler" })).toBeInTheDocument();
      expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /Create a new connection/ })).toBeEnabled();
      expect(screen.getByRole("button", { name: /New connection from URL/ })).toBeEnabled();
      expect(screen.getByRole("button", { name: /Import connections/ })).toBeDisabled();
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
      ).toEqual(["Connect", "Edit…", "Duplicate", "Copy as URL", "Delete…"]);
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
      const url = "postgresql://app@10.0.1.20:5432/app_staging?name=Staging+via+SSH";
      await vi.waitFor(() =>
        expect(toast.success).toHaveBeenCalledWith("Copied", { description: url }),
      );
      expect(await navigator.clipboard.readText()).toBe(url);
    });
  });
});
