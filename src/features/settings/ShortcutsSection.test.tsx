import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { formatCombo } from "@/lib/keys/combo";
import { applyKeymapOverrides, shortcutFor } from "@/lib/keys/keymap";
import { EMPTY_OVERRIDES } from "@/lib/keys/overrides";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { ShortcutsSection } from "./ShortcutsSection";

/** jsdom is not macOS: Mod is Ctrl. */
const mod = { ctrlKey: true };

function renderSection(query = "") {
  return render(
    <KeyboardProvider>
      <ShortcutsSection query={query} onQueryChange={() => undefined} />
    </KeyboardProvider>,
  );
}

const slot = (label: string, combo: string) =>
  screen.getByLabelText(`${label}: ${formatCombo(combo)}. Change shortcut`);

const row = (id: string) => document.querySelector<HTMLElement>(`[data-row="${id}"]`)!;

describe("ShortcutsSection (keymap editor)", { timeout: 20_000 }, () => {
  beforeEach(() => {
    resetPrefsForTests();
  });
  afterEach(() => {
    resetPrefsForTests();
    applyKeymapOverrides(EMPTY_OVERRIDES);
  });

  it("records a new shortcut for the clicked binding", () => {
    renderSection();
    fireEvent.click(slot("New query tab", "Mod+T"));
    const recorder = screen.getByLabelText("New query tab: type a shortcut");
    expect(recorder).toHaveFocus();
    expect(recorder).toHaveTextContent("Type a shortcut…");
    // A modifier alone keeps waiting.
    fireEvent.keyDown(recorder, { key: "Control", code: "ControlLeft", ...mod });
    fireEvent.keyDown(recorder, { key: "y", code: "KeyY", ...mod });
    expect(getPrefs().keymapOverrides).toEqual({ "tab.new-query": ["Mod+Y", "Mod+E"] });
    expect(shortcutFor("tab.new-query")).toBe("Mod+Y");
    expect(slot("New query tab", "Mod+Y")).toBeInTheDocument();
    // Its alternative row (Open SQL editor ⌘E) is untouched.
    expect(slot("Open SQL editor", "Mod+E")).toBeInTheDocument();
  });

  it("cancels on Esc and removes the binding on Backspace", () => {
    renderSection();
    fireEvent.click(slot("Next tab", "Mod+]"));
    fireEvent.keyDown(screen.getByLabelText("Next tab: type a shortcut"), {
      key: "Escape",
      code: "Escape",
    });
    expect(getPrefs().keymapOverrides).toEqual({});
    fireEvent.click(slot("Go back", "Ctrl+-"));
    fireEvent.keyDown(screen.getByLabelText("Go back: type a shortcut"), {
      key: "Backspace",
      code: "Backspace",
    });
    expect(getPrefs().keymapOverrides).toEqual({ "tab.back": [""] });
    // With every binding gone the row offers "None" to bind it again.
    expect(within(row("tab.back")).getByRole("button", { name: /no shortcut/ })).toHaveTextContent(
      "None",
    );
  });

  it("warns about a combo another action of the scope uses, and refuses reserved ones", () => {
    renderSection();
    fireEvent.click(slot("Next tab", "Mod+]"));
    const recorder = () => screen.getByLabelText("Next tab: type a shortcut");
    fireEvent.keyDown(recorder(), { key: "r", code: "KeyR", ...mod });
    expect(within(row("tab.next")).getByText(/is reserved/)).toBeInTheDocument();
    expect(getPrefs().keymapOverrides).toEqual({});
    fireEvent.keyDown(recorder(), { key: "a", code: "KeyA", shiftKey: true });
    expect(within(row("tab.next")).getByText(/plain keys type text/)).toBeInTheDocument();
    fireEvent.keyDown(recorder(), { key: "k", code: "KeyK", ...mod });
    expect(
      within(row("tab.next")).getByText("Also used by “Switch database”."),
    ).toBeInTheDocument();
    expect(
      within(row("palette.databases")).getByText("Also used by “Next tab”."),
    ).toBeInTheDocument();
  });

  it("keeps ⌘R and ⌘0 fixed", () => {
    renderSection();
    expect(
      screen.queryByLabelText(`Refresh: ${formatCombo("Mod+R")}. Change shortcut`),
    ).not.toBeInTheDocument();
    expect(within(row("view.refresh")).getByTitle(/can't be changed/)).toBeInTheDocument();
    // Its other binding (⌥⌘R, its own row) can move.
    expect(slot("Reload current tab", "Mod+Alt+R")).toBeInTheDocument();
  });

  it("resets a row and everything", () => {
    renderSection();
    fireEvent.click(slot("New query tab", "Mod+T"));
    fireEvent.keyDown(screen.getByLabelText(/type a shortcut/), {
      key: "y",
      code: "KeyY",
      ...mod,
    });
    fireEvent.click(slot("Next tab", "Mod+]"));
    fireEvent.keyDown(screen.getByLabelText(/type a shortcut/), {
      key: "j",
      code: "KeyJ",
      ...mod,
      altKey: true,
    });
    expect(Object.keys(getPrefs().keymapOverrides)).toEqual(["tab.new-query", "tab.next"]);
    fireEvent.click(screen.getByLabelText("Reset New query tab"));
    expect(getPrefs().keymapOverrides).toEqual({
      "tab.next": ["Mod+Alt+J", "Ctrl+Tab", "Mod+Shift+]"],
    });
    fireEvent.click(screen.getByText("Reset All"));
    expect(getPrefs().keymapOverrides).toEqual({});
    expect(screen.getByText("Reset All")).toBeDisabled();
  });

  it("does not run actions while recording", () => {
    const close = vi.fn();
    const unregister = actions.register("tab.close", close);
    renderSection();
    fireEvent.click(slot("Close tab", "Mod+W"));
    const recorder = screen.getByLabelText("Close tab: type a shortcut");
    fireEvent.keyDown(recorder, { key: "w", code: "KeyW", ...mod });
    expect(close).not.toHaveBeenCalled();
    // Recording ended: the shortcut runs again.
    fireEvent.keyDown(document.body, { key: "w", code: "KeyW", ...mod });
    expect(close).toHaveBeenCalledTimes(1);
    unregister();
  });

  it("searches the current keys", () => {
    setPref("keymapOverrides", { "tab.new-query": ["Mod+Y", "Mod+E"] });
    renderSection("ctrl+y");
    expect(screen.getByText("New query tab")).toBeInTheDocument();
    expect(screen.queryByText("Open SQL editor")).not.toBeInTheDocument();
  });
});
