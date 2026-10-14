import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { DEFAULT_PREFS, getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { AppearanceSettings } from "./AppearanceSettings";
import { GridBehaviourRows } from "./GridBehaviourRows";
import { builtinTheme } from "./builtinThemes";
import { type CustomTheme } from "./customTheme";
import { resetThemesForTests, useThemes } from "./themesStore";
import { THEME_TOKENS } from "./tokens";

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
  resetThemesForTests();
  useUiStore.setState({ themePref: "light" });
});
afterEach(() => {
  vi.restoreAllMocks();
  resetPrefsForTests();
  resetThemesForTests();
});

const hexOf = (el: HTMLElement) => (el as HTMLInputElement).value;

describe("AppearanceSettings", () => {
  it("lists every colour of the SQL editor and the data table with the theme's own value", () => {
    render(<AppearanceSettings />);
    const light = builtinTheme("light")!;
    for (const name of [
      "Comment",
      "Number",
      "Single quote string",
      "Double quote string",
      "Backtick quote string",
      "Bracket quote string",
      "Reserved keyword",
      "Current query",
      "Selected background",
      "Error highlight",
      "Others",
      "Invisible characters",
      "Soft deleted row",
      "Modified value",
      "New row",
      "Date text",
    ]) {
      const field = screen.getByLabelText(name, { selector: "input[id^='pref-color-']" });
      expect(hexOf(field)).toMatch(/^#[0-9a-f]{6}([0-9a-f]{2})?$/i);
    }
    // Number is the Light theme's `syn-number`, shown as hex.
    expect(
      hexOf(screen.getByLabelText("Number", { selector: "input[id^='pref-color-']" })),
    ).toMatch(/^#/);
    expect(light.tokens["syn-number"]).toContain("oklch");
  });

  it("overrides one colour for the theme in effect and resets it", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const field = screen.getByLabelText("Reserved keyword", {
      selector: "input[id^='pref-color-']",
    });
    await user.clear(field);
    await user.type(field, "#ff0000{Enter}");
    expect(getPrefs().colorOverrides).toEqual({ light: { "syn-keyword": "#ff0000" } });
    expect(hexOf(field)).toBe("#ff0000");
    // A reset button appears for the overridden colour only.
    await user.click(
      screen.getByRole("button", { name: "Reset Reserved keyword to theme default" }),
    );
    expect(getPrefs().colorOverrides).toEqual({});
    expect(hexOf(field)).not.toBe("#ff0000");
  });

  it("keeps a colour override per theme", async () => {
    const user = userEvent.setup();
    useUiStore.setState({ themePref: "dark" });
    render(<AppearanceSettings />);
    const field = screen.getByLabelText("Comment", { selector: "input[id^='pref-color-']" });
    await user.clear(field);
    await user.type(field, "#123456{Enter}");
    expect(getPrefs().colorOverrides).toEqual({ dark: { "syn-comment": "#123456" } });
    act(() => useUiStore.setState({ themePref: "light" }));
    expect(
      hexOf(screen.getByLabelText("Comment", { selector: "input[id^='pref-color-']" })),
    ).not.toBe("#123456");
  });

  it("rejects text that is not a colour and keeps the old value", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const field = screen.getByLabelText("Number", { selector: "input[id^='pref-color-']" });
    const before = hexOf(field);
    await user.clear(field);
    await user.type(field, "banana{Enter}");
    expect(getPrefs().colorOverrides).toEqual({});
    expect(hexOf(field)).toBe(before);
  });

  it("accepts short hex and keeps a translucent tint's alpha through the colour picker", () => {
    render(<AppearanceSettings />);
    const text = screen.getByLabelText("Current query", { selector: "input[id^='pref-color-']" });
    fireEvent.change(text, { target: { value: "#0f0" } });
    fireEvent.blur(text);
    expect(getPrefs().colorOverrides.light?.["editor-statement-bg"]).toBe("#00ff00");
    const picker = screen.getByLabelText("Current query colour picker");
    fireEvent.change(picker, { target: { value: "#112233" } });
    // The Light theme's tint is translucent (6%); a picked colour keeps that.
    expect(getPrefs().colorOverrides.light?.["editor-statement-bg"]).toBe("#112233");
  });

  it("Reset to theme default clears all the colours of a group", async () => {
    const user = userEvent.setup();
    setPref("colorOverrides", {
      light: { "syn-comment": "#111111", "syn-number": "#222222", "change-new": "#333333" },
    });
    render(<AppearanceSettings />);
    const resets = screen.getAllByRole("button", { name: "Reset to theme default" });
    expect(resets).toHaveLength(2);
    await user.click(resets[0]!);
    // The editor group's colours are gone; the data table's stay.
    expect(getPrefs().colorOverrides).toEqual({ light: { "change-new": "#333333" } });
  });

  it("sets the editor font, line height and size", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const font = screen.getByLabelText("Font", { selector: "#pref-editor-font" });
    expect(font).toHaveAttribute("placeholder", "System monospace");
    const list = document.getElementById("pref-editor-font-list")!;
    expect([...list.querySelectorAll("option")].map((o) => o.getAttribute("value"))).toEqual([
      "System monospace",
      "SF Mono",
      "Menlo",
      "Monaco",
      "JetBrains Mono",
      "Fira Code",
      "Source Code Pro",
      "Courier New",
    ]);
    await user.type(font, "JetBrains Mono{Enter}");
    expect(getPrefs().editorFontFamily).toBe("JetBrains Mono");

    const lh = screen.getByLabelText("Line height", { selector: "#pref-editor-line-height" });
    await user.clear(lh);
    await user.type(lh, "1.83{Enter}");
    // Snapped to the 0.05 step.
    expect(getPrefs().editorLineHeight).toBe(1.85);
    await user.clear(lh);
    await user.type(lh, "9{Enter}");
    expect(getPrefs().editorLineHeight).toBe(2.5);
    await user.clear(lh);
    await user.type(lh, "0.2{Enter}");
    expect(getPrefs().editorLineHeight).toBe(1);
  });

  it("System monospace stands for the default and clears the font", async () => {
    const user = userEvent.setup();
    setPref("editorFontFamily", "Menlo");
    render(<AppearanceSettings />);
    const font = screen.getByLabelText("Font", { selector: "#pref-editor-font" });
    expect(font).toHaveValue("Menlo");
    await user.clear(font);
    await user.type(font, "system monospace{Enter}");
    expect(getPrefs().editorFontFamily).toBe("");
    expect(font).toHaveValue("");
  });

  it("commits a font typed when the section goes away without a blur", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<AppearanceSettings />);
    await user.type(screen.getByLabelText("Font", { selector: "#pref-grid-font" }), "Menlo");
    unmount();
    expect(getPrefs().gridFontFamily).toBe("Menlo");
  });

  it("refuses a font name that is not one", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const font = screen.getByLabelText("Font", { selector: "#pref-editor-font" });
    await user.type(font, 'Menlo"; x{Enter}');
    expect(getPrefs().editorFontFamily).toBe("");
    expect(font).toHaveValue("");
  });

  it("changes the data table's row padding with the presets and a number", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const group = screen.getByRole("radiogroup", { name: "Row padding" });
    expect(within(group).getByRole("radio", { name: "Default" })).toBeChecked();
    await user.click(within(group).getByRole("radio", { name: "Compact" }));
    expect(getPrefs().gridRowPadding).toBe(1);
    await user.click(within(group).getByRole("radio", { name: "Comfortable" }));
    expect(getPrefs().gridRowPadding).toBe(6);
    expect(screen.getByText(/Rows are 28 px high/)).toBeInTheDocument();
    const px = screen.getByRole("spinbutton", { name: "Padding in pixels" });
    await user.clear(px);
    await user.type(px, "4{Enter}");
    expect(getPrefs().gridRowPadding).toBe(4);
    // A custom number matches no preset.
    expect(within(group).queryByRole("radio", { checked: true })).toBeNull();
  });

  it("changes the data table and sidebar font sizes", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    await user.click(document.getElementById("pref-sidebar-font-size")!);
    await user.click(await screen.findByRole("option", { name: "14 px" }));
    expect(getPrefs().sidebarFontSize).toBe(14);
    await user.click(document.getElementById("pref-grid-font-size")!);
    await user.click(await screen.findByRole("option", { name: "15 px" }));
    expect(getPrefs().gridFontSize).toBe(15);
  });

  it("has a font field for the left sidebar that sets and clears sidebarFontFamily", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const font = screen.getByLabelText("Font", { selector: "#pref-sidebar-font" });
    expect(font).toHaveValue("");
    expect(font).toHaveAttribute("placeholder", "System font");
    await user.type(font, "Helvetica Neue{Enter}");
    expect(getPrefs().sidebarFontFamily).toBe("Helvetica Neue");
    expect(getPrefs().editorFontFamily).toBe("");
    expect(getPrefs().gridFontFamily).toBe("");
    // The default's name stands for "empty".
    await user.clear(font);
    await user.type(font, "system font{Enter}");
    expect(getPrefs().sidebarFontFamily).toBe("");
    // Something that is not a font name is refused.
    await user.type(font, 'Menlo"; x{Enter}');
    expect(getPrefs().sidebarFontFamily).toBe("");
  });

  it("chooses a theme from the list, built-in and custom", async () => {
    const user = userEvent.setup();
    const custom: CustomTheme = { id: "tnight", name: "Night", base: "dark", tokens: {} };
    useThemes.setState({ themes: [custom] });
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("combobox", { name: "Theme" }));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual([
      "Auto (follows macOS)",
      "Light",
      "Dark",
      "Solarized Light",
      "Solarized Dark",
      "Night",
    ]);
    await user.click(screen.getByRole("option", { name: "Solarized Dark" }));
    expect(useUiStore.getState().themePref).toBe("solarized-dark");
    await user.click(screen.getByRole("combobox", { name: "Theme" }));
    await user.click(await screen.findByRole("option", { name: "Night" }));
    expect(useUiStore.getState().themePref).toBe("custom:tnight");
    await user.click(screen.getByRole("combobox", { name: "Theme" }));
    await user.click(await screen.findByRole("option", { name: /Auto/ }));
    expect(useUiStore.getState().themePref).toBe("system");
  });

  it("New Theme… copies the current theme into an editor and saves it as a custom theme", async () => {
    const user = userEvent.setup();
    useUiStore.setState({ themePref: "solarized-light" });
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "New Theme…" }));
    const dialog = await screen.findByRole("dialog", { name: "New Theme" });
    const name = within(dialog).getByLabelText("Name");
    expect(name).toHaveValue("Solarized Light copy");
    // Every colour token is a field, showing the copied theme's hex.
    for (const t of THEME_TOKENS)
      expect(
        within(dialog).getByLabelText(t.label, { selector: `#theme-color-${t.name}` }),
      ).toBeInTheDocument();
    expect(
      hexOf(within(dialog).getByLabelText("Text", { selector: "#theme-color-foreground" })),
    ).toBe("#3b5159");
    // The sample window follows the edits.
    const sample = within(dialog).getByRole("img", { name: "Theme preview" });
    expect(sample.style.getPropertyValue("--foreground")).toBe("#3b5159");

    const text = within(dialog).getByLabelText("Text", { selector: "#theme-color-foreground" });
    await user.clear(text);
    await user.type(text, "#102030{Enter}");
    expect(sample.style.getPropertyValue("--foreground")).toBe("#102030");

    await user.clear(name);
    await user.type(name, "Paper");
    await user.click(within(dialog).getByRole("radio", { name: "Dark" }));
    await user.click(within(dialog).getByRole("button", { name: "Save Theme" }));

    expect(screen.queryByRole("dialog", { name: "New Theme" })).not.toBeInTheDocument();
    const [saved] = useThemes.getState().themes;
    expect(saved).toMatchObject({ name: "Paper", base: "dark" });
    expect(saved!.tokens.foreground).toBe("#102030");
    expect(useUiStore.getState().themePref).toBe(`custom:${saved!.id}`);
  });

  it("does not save a theme without a name, and Cancel changes nothing", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "New Theme…" }));
    const dialog = await screen.findByRole("dialog", { name: "New Theme" });
    await user.clear(within(dialog).getByLabelText("Name"));
    expect(within(dialog).getByRole("button", { name: "Save Theme" })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(useThemes.getState().themes).toEqual([]);
    expect(useUiStore.getState().themePref).toBe("light");
  });

  it("Edit… changes the custom theme in use; built-in themes cannot be edited or deleted", async () => {
    const user = userEvent.setup();
    const custom: CustomTheme = {
      id: "tnight",
      name: "Night",
      base: "dark",
      tokens: { surface: "#0a0a0a" },
    };
    useThemes.setState({ themes: [custom] });
    render(<AppearanceSettings />);
    expect(screen.getByRole("button", { name: "Edit…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    act(() => useUiStore.setState({ themePref: "custom:tnight" }));
    await user.click(screen.getByRole("button", { name: "Edit…" }));
    const dialog = await screen.findByRole("dialog", { name: /Edit/ });
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Night");
    const surface = within(dialog).getByLabelText("Content (grid, editor)", {
      selector: "#theme-color-surface",
    });
    expect(hexOf(surface)).toBe("#0a0a0a");
    await user.clear(surface);
    await user.type(surface, "#202020{Enter}");
    await user.click(within(dialog).getByRole("button", { name: "Save Theme" }));
    expect(useThemes.getState().themes[0]!.tokens.surface).toBe("#202020");
    expect(useThemes.getState().themes).toHaveLength(1);
  });

  it("Duplicate opens a copy of the theme in effect", async () => {
    const user = userEvent.setup();
    useUiStore.setState({ themePref: "dark" });
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "Duplicate" }));
    const dialog = await screen.findByRole("dialog", { name: "Duplicate Theme" });
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Dark copy");
  });

  it("Delete removes the custom theme in use and falls back to Auto", async () => {
    const user = userEvent.setup();
    const custom: CustomTheme = { id: "tnight", name: "Night", base: "dark", tokens: {} };
    useThemes.setState({ themes: [custom] });
    useUiStore.setState({ themePref: "custom:tnight" });
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(useThemes.getState().themes).toEqual([]);
    expect(useUiStore.getState().themePref).toBe("system");
  });

  it("Export Theme… writes a .tablertheme file; Import Theme… reads one back and selects it", async () => {
    const user = userEvent.setup();
    useUiStore.setState({ themePref: "solarized-dark" });
    const prompt = vi.spyOn(window, "prompt").mockReturnValue("/tmp/sd.tablertheme");
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "Export Theme…" }));
    await waitFor(async () =>
      expect(await ipc.files.readText("/tmp/sd.tablertheme")).toContain("Solarized Dark"),
    );
    expect(prompt).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Import Theme…" }));
    await waitFor(() => expect(useThemes.getState().themes).toHaveLength(1));
    const [imported] = useThemes.getState().themes;
    expect(imported).toMatchObject({ name: "Solarized Dark", base: "dark" });
    expect(useUiStore.getState().themePref).toBe(`custom:${imported!.id}`);
  });

  it("Import Theme… with a broken file imports nothing", async () => {
    const user = userEvent.setup();
    await ipc.files.writeText(
      "/tmp/bad.tablertheme",
      '{"format":"tabler-theme","version":1,"name":"x","base":"light","tokens":{"bogus":"#fff"}}',
    );
    vi.spyOn(window, "prompt").mockReturnValue("/tmp/bad.tablertheme");
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "Import Theme…" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(useThemes.getState().themes).toEqual([]);
  });

  it("Reset Fonts & Colors restores every default of the theme in effect", async () => {
    const user = userEvent.setup();
    setPref("editorFontFamily", "Menlo");
    setPref("editorLineHeight", 2);
    setPref("gridFontFamily", "Arial");
    setPref("gridFontSize", 15);
    setPref("gridLineHeight", 1.8);
    setPref("gridRowPadding", 7);
    setPref("sidebarFontSize", 14);
    setPref("sidebarFontFamily", "Menlo");
    setPref("colorOverrides", {
      light: { "syn-comment": "#111111" },
      dark: { "syn-comment": "#222222" },
    });
    render(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: "Reset Fonts & Colors" }));
    expect(getPrefs()).toMatchObject({
      editorFontFamily: DEFAULT_PREFS.editorFontFamily,
      editorLineHeight: DEFAULT_PREFS.editorLineHeight,
      gridFontFamily: "",
      gridFontSize: DEFAULT_PREFS.gridFontSize,
      gridLineHeight: DEFAULT_PREFS.gridLineHeight,
      gridRowPadding: DEFAULT_PREFS.gridRowPadding,
      sidebarFontSize: DEFAULT_PREFS.sidebarFontSize,
      sidebarFontFamily: "",
    });
    // Only the theme in effect loses its overrides.
    expect(getPrefs().colorOverrides).toEqual({ dark: { "syn-comment": "#222222" } });
  });

  it("previews the editor and the data table", () => {
    render(<AppearanceSettings />);
    expect(screen.getByLabelText("SQL editor preview")).toBeInTheDocument();
    expect(screen.getByLabelText("Data table preview")).toBeInTheDocument();
  });
});

describe("GridBehaviourRows (General ▸ Table Data)", () => {
  it("toggles Disable scroll elastically and Auto expand text column, both off by default", async () => {
    const user = userEvent.setup();
    render(<GridBehaviourRows />);
    const elastic = screen.getByRole("switch", { name: "Disable scroll elastically" });
    const expand = screen.getByRole("switch", { name: "Auto expand text column" });
    expect(elastic).not.toBeChecked();
    expect(expand).not.toBeChecked();
    await user.click(elastic);
    await user.click(expand);
    expect(getPrefs().gridNoElastic).toBe(true);
    expect(getPrefs().gridAutoExpandText).toBe(true);
  });
});
