import { EditorState } from "@codemirror/state";
import { indentUnit } from "@codemirror/language";
import { describe, expect, it } from "vitest";
import { editorPrefsOf, prefsExtension } from "@/features/editor/cmExtensions";
import { DEFAULT_PREFS, sanitizePrefs, TAB_SIZES } from "./prefs";

describe("SQL editor indent (Settings ▸ Editor ▸ Tab size)", () => {
  it("is 4 spaces by default, like TablePlus", () => {
    expect(DEFAULT_PREFS.editorTabSize).toBe(4);
    expect(DEFAULT_PREFS.editorIndentWithTabs).toBe(false);
    expect(sanitizePrefs(null).editorTabSize).toBe(4);
  });

  it("still accepts 2 and 4 and rejects everything else", () => {
    expect([...TAB_SIZES]).toEqual([2, 4]);
    expect(sanitizePrefs({ editorTabSize: 2 }).editorTabSize).toBe(2);
    expect(sanitizePrefs({ editorTabSize: 4 }).editorTabSize).toBe(4);
    for (const bad of [3, 8, 0, "4", null]) {
      expect(sanitizePrefs({ editorTabSize: bad }).editorTabSize).toBe(4);
    }
  });

  it("is what the Tab key indents with out of the box (the editor's real extension)", () => {
    const state = EditorState.create({
      doc: "select 1",
      extensions: prefsExtension(editorPrefsOf(DEFAULT_PREFS)),
    });
    expect(state.facet(indentUnit)).toBe("    ");
    expect(state.tabSize).toBe(4);
  });

  it("is what Beautify SQL indents with out of the box", async () => {
    const { format } = await import("sql-formatter");
    const out = format("select a, b from t where x = 1 and y = 2", {
      language: "postgresql",
      tabWidth: DEFAULT_PREFS.editorTabSize,
      useTabs: DEFAULT_PREFS.editorIndentWithTabs,
    });
    const indented = out.split("\n").filter((l) => /^\s+\S/.test(l));
    expect(indented.length).toBeGreaterThan(0);
    for (const line of indented) expect(line).toMatch(/^ {4}\S/);
  });
});

describe("sidebarFontFamily (Settings ▸ Appearance ▸ Left sidebar ▸ Font)", () => {
  it("defaults to the UI font (empty)", () => {
    expect(DEFAULT_PREFS.sidebarFontFamily).toBe("");
    expect(sanitizePrefs(null).sidebarFontFamily).toBe("");
  });

  it("keeps a valid family, drops junk", () => {
    expect(sanitizePrefs({ sidebarFontFamily: "Menlo" }).sidebarFontFamily).toBe("Menlo");
    expect(sanitizePrefs({ sidebarFontFamily: "Helvetica Neue, Arial" }).sidebarFontFamily).toBe(
      "Helvetica Neue, Arial",
    );
    for (const bad of ['Menlo"; x', "a{b}", "url(x)", 12, null, "x".repeat(200)]) {
      expect(sanitizePrefs({ sidebarFontFamily: bad }).sidebarFontFamily).toBe("");
    }
  });
});
