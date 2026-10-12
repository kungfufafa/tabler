import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { Launcher } from "./Launcher";
import { useConnectionDialog } from "./store";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

function renderLauncher() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <Launcher />
    </QueryClientProvider>,
  );
}

const card = async (name: string) =>
  (await screen.findByText(name)).closest<HTMLElement>("[role='button']")!;

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
    useConnectionDialog.setState({ open: false, profile: null });
  });
  afterEach(() => vi.restoreAllMocks());

  it("opens one session for a double-click", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    renderLauncher();
    const el = await card("Local Postgres");
    // A double-click: two clicks and a dblclick event.
    fireEvent.click(el, { detail: 1 });
    fireEvent.click(el, { detail: 2 });
    fireEvent.doubleClick(el);
    fireEvent.keyDown(el, { key: "Enter" });
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
    fireEvent.click(await card("Local Postgres"));
    expect(connect).not.toHaveBeenCalled();
    expect(useConnectionsStore.getState().activeId).toBe("newer");
    expect(useUiStore.getState().view).toBe("workspace");
  });

  it("does not start a connect the palette already has in flight", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    usePaletteStore.getState().beginConnect("p-shop-mysql");
    renderLauncher();
    fireEvent.click(await card("Shop MySQL"));
    expect(connect).not.toHaveBeenCalled();
  });

  describe("keyboard on the card's actions menu", () => {
    const actionsButton = async (name: string) =>
      within(await card(name)).getByRole("button", { name: "Connection actions" });

    it("↩ on the … button opens the menu without connecting", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      const user = userEvent.setup();
      renderLauncher();
      (await actionsButton("Local Postgres")).focus();
      await user.keyboard("{Enter}");
      expect(await screen.findByRole("menu")).toBeInTheDocument();
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
    });

    it("↩ on Delete only asks to delete", async () => {
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

    it("Space on the card itself still opens it", async () => {
      const connect = vi.spyOn(ipc.connections, "connect");
      renderLauncher();
      fireEvent.keyDown(await card("Shop MySQL"), { key: " " });
      expect(usePaletteStore.getState().connecting).toEqual(["p-shop-mysql"]);
      await vi.waitFor(() => expect(connect).toHaveBeenCalledTimes(1));
      await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
    });
  });

  it("releases the guard when the connect fails", async () => {
    vi.spyOn(ipc.connections, "connect").mockRejectedValue(
      Object.assign(new Error("refused"), { code: "CONNECT_FAILED" }),
    );
    renderLauncher();
    fireEvent.click(await card("Shop MySQL"));
    await vi.waitFor(() => expect(usePaletteStore.getState().connecting).toEqual([]));
    expect(useConnectionsStore.getState().order).toEqual([]);
  });
});
