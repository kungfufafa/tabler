import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { actions, type ActionId } from "@/features/palette/actions";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { DEFAULT_PREFS, getPrefs, resetPrefsForTests, usePrefsStore } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { SettingsDialog } from "./SettingsDialog";
import { closeSettings, useSettingsDialog } from "./store";

function renderDialog() {
  // Sections may read through React Query (the provider is app-wide in the real shell).
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <KeyboardProvider>
        <SettingsDialog />
      </KeyboardProvider>
    </QueryClientProvider>,
  );
}

describe("SettingsDialog", () => {
  /** Action handlers registered by a test, removed after it. */
  const unregister: (() => void)[] = [];

  beforeAll(() => {
    // jsdom lacks these; Radix Select needs them to open and scroll to the selected item.
    Element.prototype.hasPointerCapture ??= () => false;
    Element.prototype.releasePointerCapture ??= () => {};
    Element.prototype.scrollIntoView ??= () => {};
  });
  beforeEach(() => {
    localStorage.clear();
    resetMockSettings();
    resetPrefsForTests();
    useSettingsDialog.setState({ open: false, section: "general", query: "" });
    useUiStore.setState({ themePref: "system" });
  });
  afterEach(() => {
    for (const off of unregister.splice(0)) off();
    act(() => closeSettings());
    resetPrefsForTests();
    document.documentElement.removeAttribute("style");
  });

  it("opens with Mod+, and edits general preferences", async () => {
    const user = userEvent.setup();
    renderDialog();
    await waitFor(() => expect(usePrefsStore.getState().loaded).toBe(true));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // jsdom is not macOS, so Mod is Ctrl.
    fireEvent.keyDown(window, { code: "Comma", key: ",", ctrlKey: true });
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("tab", { name: /General/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );

    // The theme choice moved to Appearance (see generalAppearance.test.tsx).
    expect(within(dialog).queryByRole("radiogroup", { name: "Theme" })).toBeNull();

    await user.click(within(dialog).getByRole("radio", { name: "Large" }));
    expect(getPrefs().uiFontSize).toBe("large");
    expect(document.documentElement.style.fontSize).not.toBe("");

    const pageSize = within(dialog).getByRole("radiogroup", { name: "Default page size" });
    await user.click(within(pageSize).getByRole("radio", { name: "200" }));
    await user.keyboard("{ArrowRight}");
    expect(getPrefs().pageSize).toBe(300);
    expect(within(pageSize).getByRole("radio", { name: "300" })).toHaveFocus();

    await user.click(within(dialog).getByRole("switch", { name: "Confirm before commit" }));
    expect(getPrefs().confirmCommit).toBe(true);
  });

  it("switches sections with Mod+digit and commits the connect timeout", async () => {
    const user = userEvent.setup();
    renderDialog();
    await act(() => actions.run("settings.open"));
    const dialog = await screen.findByRole("dialog");

    fireEvent.keyDown(dialog, { code: "Digit3", key: "3", ctrlKey: true });
    const timeout = await within(dialog).findByRole("spinbutton", { name: "Connect timeout" });
    expect(within(dialog).getByText(/macOS Keychain/)).toBeInTheDocument();
    await user.clear(timeout);
    await user.type(timeout, "999{Enter}");
    expect(getPrefs().connectTimeout).toBe(300);
    expect(timeout).toHaveValue(300);

    fireEvent.keyDown(dialog, { code: "Digit2", key: "2", ctrlKey: true });
    await user.click(await within(dialog).findByRole("switch", { name: "Wrap long lines" }));
    expect(getPrefs().editorLineWrap).toBe(true);
  });

  it("lists shortcuts and filters them by search", async () => {
    const user = userEvent.setup();
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true, section: "shortcuts" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Global")).toBeInTheDocument();
    expect(within(dialog).getByText("Table grid")).toBeInTheDocument();
    expect(within(dialog).getByText("SQL editor")).toBeInTheDocument();
    expect(within(dialog).getByText("Settings", { selector: "span" })).toBeInTheDocument();

    await user.type(
      within(dialog).getByRole("searchbox", { name: "Search shortcuts" }),
      "duplicate",
    );
    expect(within(dialog).getByText("Duplicate row")).toBeInTheDocument();
    expect(within(dialog).queryByText("New query tab")).not.toBeInTheDocument();
    expect(within(dialog).queryByText("Global")).not.toBeInTheDocument();

    // Esc clears a non-empty search before it closes the dialog.
    await user.keyboard("{Escape}");
    expect(useSettingsDialog.getState().query).toBe("");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("gives Locations, AI Assistant and MCP Server sections of their own (Mod+4, Mod+7, Mod+8)", async () => {
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true }));
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog)
        .getAllByRole("tab")
        .slice(0, 8)
        .map((t) => t.textContent?.trim()),
    ).toEqual([
      expect.stringContaining("General"),
      expect.stringContaining("Editor"),
      expect.stringContaining("Connections"),
      expect.stringContaining("Locations"),
      expect.stringContaining("Shortcuts"),
      expect.stringContaining("Security"),
      expect.stringContaining("AI Assistant"),
      expect.stringContaining("MCP Server"),
    ]);
    // Not inside General / Connections / Security any more.
    expect(within(dialog).queryByText("Connections & settings")).not.toBeInTheDocument();

    fireEvent.keyDown(dialog, { code: "Digit4", key: "4", ctrlKey: true });
    expect(await within(dialog).findByText("Connections & settings")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Locations", level: 2 })).toBeVisible();

    fireEvent.keyDown(dialog, { code: "Digit7", key: "7", ctrlKey: true });
    expect(within(dialog).getByRole("heading", { name: "AI Assistant", level: 2 })).toBeVisible();
    expect(useSettingsDialog.getState().section).toBe("assistant");

    fireEvent.keyDown(dialog, { code: "Digit8", key: "8", ctrlKey: true });
    expect(within(dialog).getByRole("heading", { name: "MCP Server", level: 2 })).toBeVisible();
    expect(
      await within(dialog).findByRole("switch", { name: /Enable MCP server/ }),
    ).toBeInTheDocument();

    fireEvent.keyDown(dialog, { code: "Digit3", key: "3", ctrlKey: true });
    expect(within(dialog).queryByRole("switch", { name: /Enable MCP server/ })).toBeNull();
    fireEvent.keyDown(dialog, { code: "Digit6", key: "6", ctrlKey: true });
    expect(within(dialog).getByRole("heading", { name: "Security", level: 2 })).toBeVisible();
    expect(within(dialog).queryByRole("heading", { name: "AI Assistant" })).toBeNull();
  });

  it("has nine sections, ⌘1…⌘9: Appearance is the ninth", async () => {
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true }));
    const dialog = await screen.findByRole("dialog");
    const labels = within(dialog)
      .getAllByRole("tab")
      .map((t) => t.textContent?.trim());
    expect(labels).toHaveLength(9);
    expect(labels[8]).toContain("Appearance");
    fireEvent.keyDown(dialog, { code: "Digit9", key: "9", ctrlKey: true });
    expect(useSettingsDialog.getState().section).toBe("appearance");
    fireEvent.keyDown(dialog, { code: "Digit4", key: "4", ctrlKey: true });
    expect(useSettingsDialog.getState().section).toBe("locations");
  });

  it("closes on Mod+W instead of closing a tab", async () => {
    // A workspace tab.close handler registered first, like useDefaultActions does at startup.
    const closeTab = vi.fn();
    unregister.push(actions.register("tab.close", closeTab));
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true }));
    const dialog = await screen.findByRole("dialog");

    fireEvent.keyDown(dialog, { code: "KeyW", key: "w", ctrlKey: true });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(closeTab).not.toHaveBeenCalled();

    // Once the dialog is gone, Mod+W is the workspace's again.
    fireEvent.keyDown(document.body, { code: "KeyW", key: "w", ctrlKey: true });
    await waitFor(() => expect(closeTab).toHaveBeenCalledTimes(1));
  });

  it.each([
    ["Mod+W", { code: "KeyW", key: "w", ctrlKey: true }],
    ["Escape", { code: "Escape", key: "Escape" }],
  ] as const)("keeps a typed connect timeout when closed with %s", async (_, key) => {
    const user = userEvent.setup();
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true, section: "connections" }));
    const dialog = await screen.findByRole("dialog");
    const timeout = within(dialog).getByRole("spinbutton", { name: "Connect timeout" });
    // Not the default (60): typing the default would pass without the value being kept at all.
    const typed = 90;
    expect(typed).not.toBe(DEFAULT_PREFS.connectTimeout);
    await user.clear(timeout);
    await user.type(timeout, String(typed));
    expect(timeout).toHaveFocus();
    // Before closing: the field shows what was typed, the stored preference is still the default.
    expect(timeout).toHaveValue(typed);
    expect(getPrefs().connectTimeout).toBe(DEFAULT_PREFS.connectTimeout);

    fireEvent.keyDown(timeout, key);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // After closing: the typed value was committed.
    expect(getPrefs().connectTimeout).toBe(typed);
    expect(getPrefs().connectTimeout).not.toBe(DEFAULT_PREFS.connectTimeout);
  });

  it("keeps the open font size dropdown in the dialog's keyboard scope", async () => {
    const user = userEvent.setup();
    const background = {
      "tab.goto-1": vi.fn(),
      "tab.close-all": vi.fn(),
      "tab.new-query": vi.fn(),
      "ui.toggle-sidebar": vi.fn(),
    } satisfies Partial<Record<ActionId, () => void>>;
    for (const [id, fn] of Object.entries(background)) {
      unregister.push(actions.register(id as ActionId, fn));
    }
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true, section: "editor" }));
    const dialog = await screen.findByRole("dialog");
    within(dialog).getByRole("combobox", { name: "Font size" }).focus();
    await user.keyboard("{Enter}");

    // Radix portals the listbox to <body>, outside [role=dialog].
    const listbox = await screen.findByRole("listbox");
    expect(dialog).not.toContainElement(listbox);
    await waitFor(() => expect(screen.getByRole("option", { name: "13 px" })).toHaveFocus());

    await user.keyboard("{Control>}b{/Control}");
    await user.keyboard("{Control>}{Shift>}w{/Shift}{/Control}");
    await user.keyboard("{Control>}t{/Control}");
    await user.keyboard("{Control>}1{/Control}");

    for (const fn of Object.values(background)) expect(fn).not.toHaveBeenCalled();
    // Mod+1 reached the dialog: General is shown and its tab has focus; the dropdown is gone.
    expect(useSettingsDialog.getState().section).toBe("general");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(within(dialog).getByRole("tab", { name: /General/ })).toHaveFocus();
  });

  it("tabs from the section list straight to the first control", async () => {
    const user = userEvent.setup();
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true }));
    const dialog = await screen.findByRole("dialog");

    const expected: [number, () => HTMLElement][] = [
      // General opens with the link to Appearance (the theme choice lives there now).
      [1, () => within(dialog).getByRole("button", { name: "More themes in Appearance" })],
      [2, () => within(dialog).getByRole("combobox", { name: "Font size" })],
      [3, () => within(dialog).getByRole("spinbutton", { name: "Connect timeout" })],
      [5, () => within(dialog).getByRole("searchbox", { name: "Search shortcuts" })],
    ];
    for (const [digit, first] of expected) {
      fireEvent.keyDown(document.activeElement ?? dialog, {
        code: `Digit${digit}`,
        key: String(digit),
        ctrlKey: true,
      });
      expect(within(dialog).getAllByRole("tab")[digit - 1]).toHaveFocus();
      await user.tab();
      expect(first()).toHaveFocus();
    }
  });
});
