import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RunState } from "@/features/editor/runModel";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { AdminHost } from "./AdminHost";
import { closeAdmin, useAdminStore } from "./store";

const info: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "Local",
  color: null,
  driver: "postgres",
  serverVersion: "x",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};

const offs: (() => void)[] = [];

beforeEach(() => {
  vi.spyOn(ipc.admin, "processes").mockResolvedValue([]);
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useConnectionsStore.getState().setActive("c1");
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useQueryTabsStore.setState({ slices: {} });
  closeAdmin();
});

afterEach(() => {
  offs.splice(0).forEach((off) => off());
  act(() => closeAdmin());
  vi.restoreAllMocks();
});

/**
 * The app with a SQL tab on screen, its results grid focused. `cancel` stands for the tab's
 * "Cancel running query", registered while it runs (as QueryTab does).
 */
function setup() {
  const tab = useTabsStore.getState().newQuery("c1");
  const cancel = vi.fn();
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <KeyboardProvider>
        <AdminHost />
        <div data-scope="editor">
          <div data-scope="grid" tabIndex={0} data-testid="grid" />
        </div>
      </KeyboardProvider>
    </QueryClientProvider>,
  );
  const grid = document.querySelector<HTMLElement>("[data-testid=grid]")!;
  act(() => grid.focus());
  const running = () => {
    updateQueryTab(tab, { run: { id: "r1", status: "running" } as unknown as RunState });
    offs.push(actions.register("query.cancel", cancel));
  };
  const pressCmdPeriod = () =>
    act(
      () =>
        void grid.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: ".",
            code: "Period",
            metaKey: isMac,
            ctrlKey: !isMac,
            bubbles: true,
            cancelable: true,
          }),
        ),
    );
  return { cancel, running, pressCmdPeriod };
}

const panel = () => useAdminStore.getState().request?.panel ?? null;

describe("⌘. in a SQL tab", () => {
  it("cancels the running query from the results grid instead of opening the Process List", () => {
    const { cancel, running, pressCmdPeriod } = setup();
    running();
    pressCmdPeriod();
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(panel()).toBeNull();
  });

  it("opens the Process List when nothing runs", () => {
    const { cancel, pressCmdPeriod } = setup();
    pressCmdPeriod();
    expect(cancel).not.toHaveBeenCalled();
    expect(panel()).toBe("processList");
  });

  it("leaves Connection ▸ Process List… and the palette opening the list while a query runs", async () => {
    const { cancel, running } = setup();
    running();
    await act(() => actions.run("admin.process-list"));
    expect(cancel).not.toHaveBeenCalled();
    expect(panel()).toBe("processList");
  });

  it("opens the Process List from a table tab", () => {
    const { cancel, running, pressCmdPeriod } = setup();
    running();
    useTabsStore.getState().openTable("c1", { schema: "public", name: "users" });
    pressCmdPeriod();
    expect(cancel).not.toHaveBeenCalled();
    expect(panel()).toBe("processList");
  });
});
