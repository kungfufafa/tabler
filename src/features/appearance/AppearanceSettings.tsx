import { Copy, FileDown, FileUp, Pencil, Plus, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  NumberField,
  SectionShell,
  Segmented,
  SettingGroup,
  SettingRow,
} from "@/features/settings/controls";
import { useSystemDark } from "@/hooks/useTheme";
import { setPref, usePrefs, EDITOR_FONT_SIZE_RANGE } from "@/stores/prefs";
import { isThemePref, useUiStore, type ThemePref } from "@/stores/ui";
import {
  DEFAULT_EDITOR_LINE_HEIGHT,
  DEFAULT_GRID_FONT_SIZE,
  DEFAULT_GRID_LINE_HEIGHT,
  DEFAULT_GRID_ROW_PADDING,
  DEFAULT_SIDEBAR_FONT_SIZE,
  EDITOR_LINE_HEIGHT_RANGE,
  GRID_FONT_SIZE_RANGE,
  GRID_FONT_SUGGESTIONS,
  GRID_LINE_HEIGHT_RANGE,
  GRID_ROW_PADDING_PRESETS,
  GRID_ROW_PADDING_RANGE,
  MONO_FONT_SUGGESTIONS,
  SIDEBAR_FONT_SIZE_RANGE,
  withColorOverride,
  withoutThemeOverrides,
} from "./appearancePrefs";
import { BUILTIN_THEMES } from "./builtinThemes";
import { ColorField, DecimalField, FontField } from "./fields";
import { gridRowHeight, paddingPreset } from "./gridMetrics";
import { EditorPreview, GridPreview } from "./previews";
import { resolveSelection, type ThemeSelection } from "./selection";
import {
  deleteCustomTheme,
  exportThemeToFile,
  importThemeFromFile,
  newThemeDraft,
  saveNewTheme,
  saveThemeEdit,
  themeLabel,
  draftFromSelection,
  type ThemeDraft,
} from "./themeActions";
import { ThemeEditorDialog } from "./ThemeEditorDialog";
import { useThemes } from "./themesStore";
import { EDITOR_COLOR_TOKENS, GRID_COLOR_TOKENS, THEME_TOKENS, type ThemeToken } from "./tokens";

const range = (min: number, max: number) =>
  Array.from({ length: max - min + 1 }, (_, i) => min + i);

const PADDING_OPTIONS = GRID_ROW_PADDING_PRESETS.map((p) => ({
  value: p.id,
  label: p.label,
  title: `${p.padding} px`,
}));

/** The theme in effect (and so the key the colour overrides are stored under). */
function useSelection(): ThemeSelection {
  const themePref = useUiStore((s) => s.themePref);
  const customs = useThemes((s) => s.themes);
  const systemDark = useSystemDark();
  return useMemo(
    () => resolveSelection(themePref, systemDark, customs),
    [themePref, systemDark, customs],
  );
}

/** A colour field wired to the per-theme override of one token. */
function TokenColorField({ token, selection }: { token: ThemeToken; selection: ThemeSelection }) {
  const overrides = usePrefs((p) => p.colorOverrides);
  const override = overrides[selection.key]?.[token];
  const label = THEME_TOKENS.find((t) => t.name === token)?.label ?? token;
  const set = (color: string | null) =>
    setPref("colorOverrides", withColorOverride(overrides, selection.key, token, color));
  return (
    <ColorField
      id={`pref-color-${token}`}
      label={label}
      value={override ?? selection.tokens[token]}
      overridden={override !== undefined}
      onChange={set}
      onReset={() => set(null)}
    />
  );
}

/** Colour fields for `tokens` with a "Reset to theme default" row when any is overridden. */
function ColorGrid({
  tokens,
  selection,
}: {
  tokens: readonly ThemeToken[];
  selection: ThemeSelection;
}) {
  const overrides = usePrefs((p) => p.colorOverrides);
  const own = overrides[selection.key] ?? {};
  const anyOverridden = tokens.some((t) => own[t] !== undefined);
  const resetAll = () => {
    let next = overrides;
    for (const token of tokens) next = withColorOverride(next, selection.key, token, null);
    setPref("colorOverrides", next);
  };
  return (
    <div className="flex flex-col gap-1 px-3 py-2.5">
      <div className="flex flex-col">
        {tokens.map((token) => (
          <TokenColorField key={token} token={token} selection={selection} />
        ))}
      </div>
      <div className="flex justify-end">
        <Button variant="ghost" size="xs" disabled={!anyOverridden} onClick={resetAll}>
          Reset to theme default
        </Button>
      </div>
    </div>
  );
}

function ThemeSelect() {
  const themePref = useUiStore((s) => s.themePref);
  const setThemePref = useUiStore((s) => s.setThemePref);
  const customs = useThemes((s) => s.themes);
  // A choice whose custom theme is gone shows as Auto (what the window does).
  const known = themePref.startsWith("custom:")
    ? customs.some((t) => `custom:${t.id}` === themePref)
    : true;
  return (
    <Select
      value={known ? themePref : "system"}
      onValueChange={(v) => isThemePref(v) && setThemePref(v)}
    >
      <SelectTrigger id="pref-theme" size="sm" className="h-7 w-44 rounded-md text-xs">
        <SelectValue />
      </SelectTrigger>
      <SelectContent data-scope="dialog">
        <SelectItem value="system" className="text-xs">
          Auto (follows macOS)
        </SelectItem>
        {BUILTIN_THEMES.map((t) => (
          <SelectItem key={t.id} value={t.id} className="text-xs">
            {t.label}
          </SelectItem>
        ))}
        {customs.length > 0 && (
          <>
            <SelectSeparator />
            <SelectGroup>
              <SelectLabel className="text-[11px]">Custom</SelectLabel>
              {customs.map((t) => (
                <SelectItem key={t.id} value={`custom:${t.id}`} className="text-xs">
                  {t.name}
                </SelectItem>
              ))}
            </SelectGroup>
          </>
        )}
      </SelectContent>
    </Select>
  );
}

type EditorState = { title: string; draft: ThemeDraft; editing: string | null } | null;

/** Settings ▸ Appearance ▸ Theme: the choice, and New / Import / Edit / Duplicate / Export / Delete. */
function ThemeGroup({ selection }: { selection: ThemeSelection }) {
  const customs = useThemes((s) => s.themes);
  const [editor, setEditor] = useState<EditorState>(null);
  const colorOverrides = usePrefs((p) => p.colorOverrides);
  const custom = selection.custom;
  const label = themeLabel(selection.key as ThemePref, customs);

  const edit = (draft: ThemeDraft, title: string, editing: string | null) =>
    setEditor({ draft, title, editing });

  const onImport = async () => {
    try {
      const result = await importThemeFromFile();
      if (!result) return;
      if (result.ok) toast.success(`Imported “${result.theme.name}”`);
      else toast.error("Could not import the theme", { description: result.error });
    } catch (err) {
      toast.error("Could not import the theme", {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };
  const onExport = async () => {
    try {
      const path = await exportThemeToFile(selection, label);
      if (path) toast.success(`Exported “${label}”`, { description: path });
    } catch (err) {
      toast.error("Could not export the theme", {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };

  return (
    <SettingGroup title="Theme">
      <SettingRow
        id="pref-theme"
        label="Theme"
        description="Auto follows the macOS appearance. A theme applies to the whole app at once, native menus and dialogs included."
      >
        <ThemeSelect />
      </SettingRow>
      <div className="flex flex-wrap items-center gap-1.5 px-3 py-2.5">
        <Button
          variant="outline"
          size="xs"
          onClick={() => edit(newThemeDraft(), "New Theme", null)}
        >
          <Plus /> New Theme…
        </Button>
        <Button variant="outline" size="xs" onClick={() => void onImport()}>
          <FileUp /> Import Theme…
        </Button>
        <span className="bg-border mx-1 h-4 w-px" aria-hidden />
        <Button
          variant="outline"
          size="xs"
          disabled={!custom}
          onClick={() =>
            custom &&
            edit(
              { name: custom.name, base: custom.base, tokens: selection.tokens },
              `Edit “${custom.name}”`,
              custom.id,
            )
          }
        >
          <Pencil /> Edit…
        </Button>
        <Button
          variant="outline"
          size="xs"
          onClick={() =>
            edit(
              draftFromSelection(selection, colorOverrides[selection.key], label),
              "Duplicate Theme",
              null,
            )
          }
        >
          <Copy /> Duplicate
        </Button>
        <Button variant="outline" size="xs" onClick={() => void onExport()}>
          <FileDown /> Export Theme…
        </Button>
        <Button
          variant="outline"
          size="xs"
          disabled={!custom}
          onClick={() => {
            if (!custom) return;
            deleteCustomTheme(custom.id);
            toast.success(`Deleted “${custom.name}”`);
          }}
          className="text-error"
        >
          <Trash2 /> Delete
        </Button>
      </div>
      {editor && (
        <ThemeEditorDialog
          key={editor.editing ?? "new"}
          title={editor.title}
          initial={editor.draft}
          onClose={() => setEditor(null)}
          onSave={(draft) => {
            if (editor.editing) saveThemeEdit(editor.editing, draft);
            else saveNewTheme(draft);
            setEditor(null);
          }}
        />
      )}
    </SettingGroup>
  );
}

function FontSizeSelect({
  id,
  value,
  min,
  max,
  onChange,
}: {
  id: string;
  value: number;
  min: number;
  max: number;
  onChange: (n: number) => void;
}) {
  return (
    <Select value={String(value)} onValueChange={(v) => onChange(Number(v))}>
      <SelectTrigger id={id} size="sm" className="h-7 w-22 rounded-md text-xs tabular-nums">
        <SelectValue />
      </SelectTrigger>
      <SelectContent data-scope="dialog">
        {range(min, max).map((n) => (
          <SelectItem key={n} value={String(n)} className="text-xs tabular-nums">
            {n} px
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function EditorGroup({ selection }: { selection: ThemeSelection }) {
  const family = usePrefs((p) => p.editorFontFamily);
  const size = usePrefs((p) => p.editorFontSize);
  const lineHeight = usePrefs((p) => p.editorLineHeight);
  return (
    <SettingGroup title="SQL editor">
      <SettingRow
        id="pref-editor-font"
        label="Font"
        description="Any installed font; several, separated by commas, work as a fallback list."
      >
        <FontField
          id="pref-editor-font"
          value={family}
          suggestions={MONO_FONT_SUGGESTIONS}
          defaultLabel="System monospace"
          onCommit={(v) => setPref("editorFontFamily", v)}
        />
      </SettingRow>
      <SettingRow id="pref-editor-font-size-appearance" label="Font size">
        <FontSizeSelect
          id="pref-editor-font-size-appearance"
          value={size}
          min={EDITOR_FONT_SIZE_RANGE.min}
          max={EDITOR_FONT_SIZE_RANGE.max}
          onChange={(n) => setPref("editorFontSize", n)}
        />
      </SettingRow>
      <SettingRow
        id="pref-editor-line-height"
        label="Line height"
        description={`${EDITOR_LINE_HEIGHT_RANGE.min}–${EDITOR_LINE_HEIGHT_RANGE.max}, as a multiple of the font size (default ${DEFAULT_EDITOR_LINE_HEIGHT}).`}
      >
        <DecimalField
          id="pref-editor-line-height"
          value={lineHeight}
          min={EDITOR_LINE_HEIGHT_RANGE.min}
          max={EDITOR_LINE_HEIGHT_RANGE.max}
          step={EDITOR_LINE_HEIGHT_RANGE.step}
          onCommit={(v) => setPref("editorLineHeight", v)}
        />
      </SettingRow>
      <ColorGrid tokens={EDITOR_COLOR_TOKENS} selection={selection} />
      <div className="px-3 pb-3">
        <EditorPreview />
      </div>
    </SettingGroup>
  );
}

function DataTableGroup({ selection }: { selection: ThemeSelection }) {
  const family = usePrefs((p) => p.gridFontFamily);
  const size = usePrefs((p) => p.gridFontSize);
  const lineHeight = usePrefs((p) => p.gridLineHeight);
  const padding = usePrefs((p) => p.gridRowPadding);
  const preset = paddingPreset(padding);
  return (
    <SettingGroup title="Data table">
      <SettingRow
        id="pref-grid-font"
        label="Font"
        description="Any installed font. Empty uses the system font."
      >
        <FontField
          id="pref-grid-font"
          value={family}
          suggestions={GRID_FONT_SUGGESTIONS}
          defaultLabel="System font"
          onCommit={(v) => setPref("gridFontFamily", v)}
        />
      </SettingRow>
      <SettingRow id="pref-grid-font-size" label="Font size">
        <FontSizeSelect
          id="pref-grid-font-size"
          value={size}
          min={GRID_FONT_SIZE_RANGE.min}
          max={GRID_FONT_SIZE_RANGE.max}
          onChange={(n) => setPref("gridFontSize", n)}
        />
      </SettingRow>
      <SettingRow
        id="pref-grid-line-height"
        label="Line height"
        description={`As a multiple of the font size (default ${DEFAULT_GRID_LINE_HEIGHT}).`}
      >
        <DecimalField
          id="pref-grid-line-height"
          value={lineHeight}
          min={GRID_LINE_HEIGHT_RANGE.min}
          max={GRID_LINE_HEIGHT_RANGE.max}
          step={GRID_LINE_HEIGHT_RANGE.step}
          onCommit={(v) => setPref("gridLineHeight", v)}
        />
      </SettingRow>
      <SettingRow
        id="pref-grid-padding"
        label="Row padding"
        description={`Rows are ${gridRowHeight(size, lineHeight, padding)} px high: one text line plus the padding above and below.`}
      >
        <Segmented
          id="pref-grid-padding"
          value={preset ?? ""}
          options={PADDING_OPTIONS}
          onChange={(id) => {
            const next = GRID_ROW_PADDING_PRESETS.find((p) => p.id === id);
            if (next) setPref("gridRowPadding", next.padding);
          }}
        />
      </SettingRow>
      <SettingRow
        id="pref-grid-padding-px"
        label="Padding in pixels"
        description={`${GRID_ROW_PADDING_RANGE.min}–${GRID_ROW_PADDING_RANGE.max}: any value, not just the three above.`}
      >
        <NumberField
          id="pref-grid-padding-px"
          value={padding}
          min={GRID_ROW_PADDING_RANGE.min}
          max={GRID_ROW_PADDING_RANGE.max}
          suffix="px"
          onCommit={(v) => setPref("gridRowPadding", v)}
        />
      </SettingRow>
      <ColorGrid tokens={GRID_COLOR_TOKENS} selection={selection} />
      <div className="px-3 pb-3">
        <GridPreview />
      </div>
    </SettingGroup>
  );
}

function SidebarGroup() {
  const family = usePrefs((p) => p.sidebarFontFamily);
  const size = usePrefs((p) => p.sidebarFontSize);
  return (
    <SettingGroup title="Left sidebar">
      <SettingRow
        id="pref-sidebar-font"
        label="Font"
        description="Any installed font. Empty uses the system font."
      >
        <FontField
          id="pref-sidebar-font"
          value={family}
          suggestions={GRID_FONT_SUGGESTIONS}
          defaultLabel="System font"
          onCommit={(v) => setPref("sidebarFontFamily", v)}
        />
      </SettingRow>
      <SettingRow
        id="pref-sidebar-font-size"
        label="Font size"
        description={`Tables, views and the other items in the sidebar (default ${DEFAULT_SIDEBAR_FONT_SIZE} px).`}
      >
        <FontSizeSelect
          id="pref-sidebar-font-size"
          value={size}
          min={SIDEBAR_FONT_SIZE_RANGE.min}
          max={SIDEBAR_FONT_SIZE_RANGE.max}
          onChange={(n) => setPref("sidebarFontSize", n)}
        />
      </SettingRow>
    </SettingGroup>
  );
}

/** Resets every Appearance preference (fonts, sizes, and the colour overrides of this theme). */
function ResetRow({ selection }: { selection: ThemeSelection }) {
  const overrides = usePrefs((p) => p.colorOverrides);
  return (
    <div className="flex justify-end">
      <Button
        variant="outline"
        size="xs"
        onClick={() => {
          setPref("editorFontFamily", "");
          setPref("editorLineHeight", DEFAULT_EDITOR_LINE_HEIGHT);
          setPref("gridFontFamily", "");
          setPref("gridFontSize", DEFAULT_GRID_FONT_SIZE);
          setPref("gridLineHeight", DEFAULT_GRID_LINE_HEIGHT);
          setPref("gridRowPadding", DEFAULT_GRID_ROW_PADDING);
          setPref("sidebarFontFamily", "");
          setPref("sidebarFontSize", DEFAULT_SIDEBAR_FONT_SIZE);
          setPref("colorOverrides", withoutThemeOverrides(overrides, selection.key));
        }}
      >
        Reset Fonts &amp; Colors
      </Button>
    </div>
  );
}

/** Settings ▸ Appearance: TablePlus's Fonts & Colors. */
export function AppearanceSettings() {
  const selection = useSelection();
  return (
    <SectionShell title="Appearance">
      <ThemeGroup selection={selection} />
      <EditorGroup selection={selection} />
      <DataTableGroup selection={selection} />
      <SidebarGroup />
      <ResetRow selection={selection} />
    </SectionShell>
  );
}
