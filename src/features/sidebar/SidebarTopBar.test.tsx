import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { formatCombo } from "@/lib/keys/combo";
import { applyKeymapOverrides, shortcutFor } from "@/lib/keys/keymap";
import { EMPTY_OVERRIDES } from "@/lib/keys/overrides";
import type { ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { SidebarTopBar } from "./SidebarTopBar";

function connect() {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId: "p1",
    name: "Local",
    color: null,
    driver: "postgres",
    serverVersion: "17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
  };
  useConnectionsStore.setState({
    byId: { c1: { info, status: "connected", message: null, currentSchema: null } },
    order: ["c1"],
    activeId: "c1",
  });
}

const disconnect = () => useConnectionsStore.setState({ byId: {}, order: [], activeId: null });

function renderBar() {
  return render(
    <TooltipProvider>
      <SidebarTopBar />
    </TooltipProvider>,
  );
}

const connections = () => screen.getByRole("button", { name: "Show Connections" });
const databases = () => screen.getByRole("button", { name: "Show Databases" });
const editor = () => screen.getByRole("button", { name: "Open Query Editor" });

/** The tooltip a button shows on keyboard focus. */
function tooltipOf(button: HTMLElement): HTMLElement {
  act(() => button.focus());
  return screen.getAllByRole("tooltip").at(-1)!;
}

describe("SidebarTopBar (TablePlus §2.2 quick navigation)", () => {
  let run: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    run = vi.spyOn(actions, "run").mockResolvedValue(true);
    connect();
  });
  afterEach(() => {
    vi.restoreAllMocks();
    resetPrefsForTests();
    applyKeymapOverrides(EMPTY_OVERRIDES);
    disconnect();
  });

  it("has Show Connections, Show Databases and Open Query Editor, in that order", () => {
    renderBar();
    const bar = screen.getByRole("toolbar", { name: "Quick navigation" });
    expect(
      within(bar)
        .getAllByRole("button")
        .map((b) => b.getAttribute("aria-label")),
    ).toEqual(["Show Connections", "Show Databases", "Open Query Editor"]);
  });

  it("opens the connection palette, the database palette and a NEW SQL tab", () => {
    renderBar();
    fireEvent.click(connections());
    expect(run).toHaveBeenLastCalledWith("palette.connections");
    fireEvent.click(databases());
    expect(run).toHaveBeenLastCalledWith("palette.databases");
    fireEvent.click(editor());
    // `tab.new-query`, not `query.open-editor`: in a SQL tab the latter would run its statement.
    expect(run).toHaveBeenLastCalledWith("tab.new-query");
    expect(run).toHaveBeenCalledTimes(3);
  });

  it("shows each button's shortcut in its tooltip (⇧⌘K, ⌘K, ⌘↩)", () => {
    expect(shortcutFor("palette.connections")).toBe("Mod+Shift+K");
    expect(shortcutFor("palette.databases")).toBe("Mod+K");
    expect(shortcutFor("query.open-editor")).toBe("Mod+Enter");
    renderBar();
    const tip = (button: HTMLElement, label: string, combo: string) => {
      const t = tooltipOf(button);
      expect(t).toHaveTextContent(label);
      expect(within(t).getByText(formatCombo(combo), { selector: "kbd" })).toBeInTheDocument();
      act(() => button.blur());
    };
    tip(connections(), "Show Connections", "Mod+Shift+K");
    tip(databases(), "Show Databases", "Mod+K");
    tip(editor(), "Open Query Editor", "Mod+Enter");
  });

  it("follows a remapped key", () => {
    renderBar();
    act(() => setPref("keymapOverrides", { "palette.databases": ["Mod+Alt+D"] }));
    expect(
      within(tooltipOf(databases())).getByText(formatCombo("Mod+Alt+D"), { selector: "kbd" }),
    ).toBeInTheDocument();
  });

  it("disables Show Databases and Open Query Editor without a connection, not Show Connections", () => {
    disconnect();
    renderBar();
    expect(connections()).toBeEnabled();
    expect(databases()).toBeDisabled();
    expect(editor()).toBeDisabled();
    fireEvent.click(databases());
    fireEvent.click(editor());
    expect(run).not.toHaveBeenCalled();
    fireEvent.click(connections());
    expect(run).toHaveBeenCalledWith("palette.connections");
  });

  it("enables them as soon as a connection opens", () => {
    disconnect();
    renderBar();
    expect(databases()).toBeDisabled();
    act(() => connect());
    expect(databases()).toBeEnabled();
    expect(editor()).toBeEnabled();
  });
});
