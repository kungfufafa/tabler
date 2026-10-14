import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { resetThemesForTests, useThemes } from "@/features/appearance/themesStore";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { EditorSection, GeneralSection } from "./sections";
import { useSettingsDialog } from "./store";

function show(ui: ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

beforeEach(() => {
  localStorage.clear();
  resetMockSettings();
  resetPrefsForTests();
  resetThemesForTests();
  useUiStore.setState({ themePref: "system" });
  useSettingsDialog.setState({ open: true, section: "general", query: "" });
});
afterEach(() => {
  resetPrefsForTests();
  resetThemesForTests();
  useSettingsDialog.setState({ open: false, section: "general", query: "" });
});

describe("General ▸ Appearance ▸ Theme", () => {
  it("no longer has its own System / Light / Dark control", () => {
    show(<GeneralSection />);
    expect(screen.queryByRole("radiogroup", { name: "Theme" })).toBeNull();
    for (const name of ["System", "Light", "Dark"])
      expect(screen.queryByRole("radio", { name })).toBeNull();
    // The interface text size stays here.
    expect(screen.getByRole("radiogroup", { name: "Text size" })).toBeInTheDocument();
  });

  it("points to Appearance: the button opens that section", () => {
    show(<GeneralSection />);
    fireEvent.click(screen.getByRole("button", { name: "More themes in Appearance" }));
    expect(useSettingsDialog.getState()).toMatchObject({ open: true, section: "appearance" });
  });

  it("names the theme in use, the stored preference untouched", () => {
    useUiStore.setState({ themePref: "dark" });
    const { rerender } = show(<GeneralSection />);
    expect(screen.getByText(/In use: Dark\./)).toBeInTheDocument();
    expect(useUiStore.getState().themePref).toBe("dark");

    act(() => useUiStore.setState({ themePref: "system" }));
    expect(screen.getByText(/In use: Auto\./)).toBeInTheDocument();
    act(() => useUiStore.setState({ themePref: "solarized-dark" }));
    expect(screen.getByText(/In use: Solarized Dark\./)).toBeInTheDocument();

    const theme = useThemes.getState().add({ name: "Midnight", base: "dark", tokens: {} })!;
    act(() => useUiStore.setState({ themePref: `custom:${theme.id}` }));
    expect(screen.getByText(/In use: Midnight\./)).toBeInTheDocument();
    rerender(<GeneralSection />);
  });
});

describe("Editor ▸ preview", () => {
  const preview = () => screen.getByLabelText("Editor preview");
  /** The colour a preview run is painted with, by its text. */
  const colorOf = (text: string | RegExp) =>
    within(preview()).getByText(text).closest<HTMLElement>("span[style*='color']")!.style.color;
  /** The font and line height are the editor's own custom properties, not fixed values. */
  it("takes the editor font family, line height and colours from the editor's custom properties", () => {
    show(<EditorSection />);
    const pre = preview();
    expect(pre.style.fontFamily).toBe("var(--editor-font-family)");
    expect(pre.style.lineHeight).toBe("var(--editor-line-height)");
    expect(pre.style.color).toBe("var(--syn-other)");
    expect(colorOf("SELECT")).toBe("var(--syn-keyword)");
    expect(colorOf("'7 days'")).toBe("var(--syn-string)");
    expect(colorOf("100")).toBe("var(--syn-number)");
    expect(colorOf(/^-- Recently active/)).toBe("var(--syn-comment)");
    expect(colorOf("SELEC")).toBe("var(--syn-error)");
  });

  it("shows double quotes, backticks and brackets in their own colours", () => {
    show(<EditorSection />);
    expect(colorOf('"Created At"')).toBe("var(--syn-string-double)");
    expect(colorOf("`region`")).toBe("var(--syn-string-backtick)");
    expect(colorOf("[plan]")).toBe("var(--syn-string-bracket)");
  });

  it("paints the current statement and the selection with their backgrounds", () => {
    show(<EditorSection />);
    const html = preview().innerHTML;
    expect(html).toContain("var(--editor-statement-bg)");
    expect(html).toContain("var(--editor-selection-bg)");
  });

  it("follows the editor font size", () => {
    show(<EditorSection />);
    act(() => setPref("editorFontSize", 17));
    expect(preview().style.fontSize).toBe("17px");
    act(() => setPref("editorFontSize", 11));
    expect(preview().style.fontSize).toBe("11px");
  });

  it("follows tab size, wrapping, keyword case and invisible characters", () => {
    show(<EditorSection />);
    // Uppercase keywords is on by default.
    expect(preview().textContent).toContain("SELECT id");
    expect(preview().textContent).not.toContain("¬");
    expect(preview().style.whiteSpace).toBe("pre");

    act(() => setPref("editorUppercaseKeywords", false));
    expect(preview().textContent).toContain("select id");
    act(() => setPref("editorLineWrap", true));
    expect(preview().style.whiteSpace).toBe("pre-wrap");

    // The default indent is 4 spaces (TablePlus); 2 is the other choice.
    expect(preview().textContent).toContain("\n    last_seen_at");
    act(() => setPref("editorTabSize", 2));
    expect(preview().textContent).toContain("\n  last_seen_at");
    expect(preview().textContent).not.toContain("\n    last_seen_at");
    act(() => setPref("editorTabSize", 4));
    expect(preview().textContent).toContain("\n    last_seen_at");
    act(() => setPref("editorShowInvisibles", true));
    expect(preview().textContent).toContain("\n····last_seen_at");
    expect(preview().textContent).toContain("¬");
    expect(
      within(preview())
        .getAllByText("¬")
        .every((m) => m.style.color === "var(--syn-invisible)"),
    ).toBe(true);
  });
});
