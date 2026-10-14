import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { SettingsDialog } from "@/features/settings/SettingsDialog";
import { closeSettings, SETTINGS_SECTIONS, useSettingsDialog } from "@/features/settings/store";
import { TableDataSettings } from "@/features/table-tab/TableDataSettings";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { getPrefs, resetPrefsForTests, usePrefsStore } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { AppearanceHost } from "./AppearanceHost";
import { resetThemesForTests } from "./themesStore";

function renderDialog() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <KeyboardProvider>
        <SettingsDialog />
        <AppearanceHost />
      </KeyboardProvider>
    </QueryClientProvider>,
  );
}

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});
beforeEach(() => {
  localStorage.clear();
  resetMockSettings();
  resetPrefsForTests();
  resetThemesForTests();
  useSettingsDialog.setState({ open: false, section: "general", query: "" });
  useUiStore.setState({ themePref: "system" });
});
afterEach(() => {
  act(() => closeSettings());
  resetPrefsForTests();
  resetThemesForTests();
  document.documentElement.removeAttribute("style");
});

describe("Settings ▸ Appearance", () => {
  it("is a section of the settings dialog", async () => {
    expect(SETTINGS_SECTIONS).toContain("appearance");
    renderDialog();
    await waitFor(() => expect(usePrefsStore.getState().loaded).toBe(true));
    act(() => useSettingsDialog.setState({ open: true, section: "appearance" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("tab", { name: /Appearance/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(dialog).getByRole("heading", { name: "Appearance" })).toBeInTheDocument();
    expect(within(dialog).getByRole("combobox", { name: "Theme" })).toHaveTextContent("Auto");
  });

  it("changes the theme of the whole window at once", async () => {
    const user = userEvent.setup();
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true, section: "appearance" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("combobox", { name: "Theme" }));
    await user.click(await screen.findByRole("option", { name: "Solarized Light" }));
    expect(useUiStore.getState().themePref).toBe("solarized-light");
    await waitFor(() => expect(document.documentElement.dataset.themeName).toBe("solarized-light"));
  });

  it("the theme editor opening over Settings does not pass its keys on to the dialog behind", async () => {
    const user = userEvent.setup();
    renderDialog();
    act(() => useSettingsDialog.setState({ open: true, section: "appearance" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("button", { name: "New Theme…" }));
    const editor = await screen.findByRole("dialog", { name: "New Theme" });
    // Mod+3 would switch Settings to Connections; here it is the editor's.
    fireEvent.keyDown(within(editor).getByLabelText("Name"), {
      code: "Digit3",
      key: "3",
      ctrlKey: true,
    });
    expect(useSettingsDialog.getState().section).toBe("appearance");
  });
});

describe("General ▸ Table Data", () => {
  it("has Disable scroll elastically and Auto expand text column", async () => {
    const user = userEvent.setup();
    render(<TableDataSettings />);
    await user.click(screen.getByRole("switch", { name: "Disable scroll elastically" }));
    await user.click(screen.getByRole("switch", { name: "Auto expand text column" }));
    expect(getPrefs()).toMatchObject({ gridNoElastic: true, gridAutoExpandText: true });
  });
});
