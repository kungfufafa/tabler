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
  return render(
    <KeyboardProvider>
      <SettingsDialog />
    </KeyboardProvider>,
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

    await user.click(within(dialog).getByRole("radio", { name: /Dark/ }));
    expect(useUiStore.getState().themePref).toBe("dark");

    await user.click(within(dialog).getByRole("radio", { name: "Large" }));
    expect(getPrefs().uiFontSize).toBe("large");
    expect(document.documentElement.style.fontSize).not.toBe("");

    const pageSize = within(dialog).getByRole("radiogroup", { name: "Default page size" });
    await user.click(within(pageSize).getByRole("radio", { name: "200" }));
    await user.keyboard("{ArrowRight}");
    expect(getPrefs().pageSize).toBe(500);
    expect(within(pageSize).getByRole("radio", { name: "500" })).toHaveFocus();

    await user.click(within(dialog).getByRole("switch", { name: "Confirm before commit" }));
    expect(getPrefs().confirmCommit).toBe(false);
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

    await user.type(within(dialog).getByRole("searchbox", { name: "Search shortcuts" }), "commit");
    expect(within(dialog).getByText("Commit changes")).toBeInTheDocument();
    expect(within(dialog).queryByText("New query tab")).not.toBeInTheDocument();
    expect(within(dialog).queryByText("Global")).not.toBeInTheDocument();

    // Esc clears a non-empty search before it closes the dialog.
    await user.keyboard("{Escape}");
    expect(useSettingsDialog.getState().query).toBe("");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
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
    await user.clear(timeout);
    await user.type(timeout, "60");
    expect(timeout).toHaveFocus();
    expect(getPrefs().connectTimeout).toBe(DEFAULT_PREFS.connectTimeout);

    fireEvent.keyDown(timeout, key);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(getPrefs().connectTimeout).toBe(60);
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

    const expected = [
      () => within(dialog).getByRole("radio", { name: /System/ }),
      () => within(dialog).getByRole("combobox", { name: "Font size" }),
      () => within(dialog).getByRole("spinbutton", { name: "Connect timeout" }),
      () => within(dialog).getByRole("searchbox", { name: "Search shortcuts" }),
    ];
    for (const [i, first] of expected.entries()) {
      fireEvent.keyDown(document.activeElement ?? dialog, {
        code: `Digit${i + 1}`,
        key: String(i + 1),
        ctrlKey: true,
      });
      expect(within(dialog).getAllByRole("tab")[i]).toHaveFocus();
      await user.tab();
      expect(first()).toHaveFocus();
    }
  });
});
