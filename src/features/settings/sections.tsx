import { KeyRound } from "lucide-react";
import type { ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { themeLabel } from "@/features/appearance/themeActions";
import { useThemes } from "@/features/appearance/themesStore";
import { StartupConnectionRow } from "@/features/connections/StartupConnectionRow";
import { EditorWorkflowSettings, NewTabOpensRow } from "@/features/editor/EditorSettingsRows";
import { NotifySettingRows } from "@/features/notify/NotifySettingRows";
import { PasscodeSettings, SafeModeTouchIdRow } from "@/features/lock/SecuritySettings";
import { SAFE_MODE_OPTIONS } from "@/features/safety/levels";
import { LazyLoadSettingRow } from "@/features/sidebar/LazyLoadSettingRow";
import { CsvDefaultSettings } from "@/features/table-tab/CsvDefaultSettings";
import {
  DataDecoderSettings,
  LoadingWallpaperRow,
  TableDataSettings,
} from "@/features/table-tab/TableDataSettings";
import { shortcutFor } from "@/lib/keys/keymap";
import {
  CONNECT_TIMEOUT_RANGE,
  EDITOR_FONT_SIZE_RANGE,
  QUERY_TIMEOUT_RANGE,
  PAGE_SIZES,
  setPref,
  TAB_SIZES,
  UI_FONT_SIZES,
  usePrefs,
  type Prefs,
  type UiFontSize,
} from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { NumberField, SectionShell, Segmented, SettingGroup, SettingRow } from "./controls";
import { AutoSaveQueryTabsRow, ReopenWorkspacesRow } from "./sessionRows";
import { openSettings } from "./store";

const FONT_SIZE_LABELS: Record<UiFontSize, string> = {
  small: "Small",
  default: "Default",
  large: "Large",
};
const FONT_SIZE_OPTIONS = (Object.keys(UI_FONT_SIZES) as UiFontSize[]).map((value) => ({
  value,
  label: FONT_SIZE_LABELS[value],
  title: `${UI_FONT_SIZES[value]}px`,
}));

const PAGE_SIZE_OPTIONS = PAGE_SIZES.map((value) => ({ value, label: value.toLocaleString() }));
const TAB_SIZE_OPTIONS = TAB_SIZES.map((value) => ({ value, label: String(value) }));
const EDITOR_FONT_SIZES = Array.from(
  { length: EDITOR_FONT_SIZE_RANGE.max - EDITOR_FONT_SIZE_RANGE.min + 1 },
  (_, i) => EDITOR_FONT_SIZE_RANGE.min + i,
);

function InlineKbd({ combo }: { combo: string }) {
  return <Kbd combo={combo} className="mx-0.5 h-4 min-w-4 align-[-2px] text-[10px]" />;
}

export function GeneralSection() {
  const themePref = useUiStore((s) => s.themePref);
  const customs = useThemes((s) => s.themes);
  const uiFontSize = usePrefs((p) => p.uiFontSize);
  const pageSize = usePrefs((p) => p.pageSize);
  const confirmCommit = usePrefs((p) => p.confirmCommit);
  const commitCombo = shortcutFor("grid.commit") ?? "Mod+S";
  return (
    <SectionShell title="General">
      <SettingGroup title="Appearance">
        {/* The choice (Auto, Light, Dark, Solarized, custom themes) lives in Appearance; the
            stored preference is the same one, so what was picked here before still applies. */}
        <SettingRow
          label="Theme"
          description={
            <>
              In use: {themeLabel(themePref, customs)}. Auto, Light, Dark, Solarized and your own
              themes are chosen in Appearance.
            </>
          }
        >
          <Button variant="outline" size="xs" onClick={() => openSettings("appearance")}>
            More themes in Appearance
          </Button>
        </SettingRow>
        <SettingRow
          id="pref-ui-font-size"
          label="Text size"
          description={`Interface text, ${Object.values(UI_FONT_SIZES).join(" / ")} px.`}
        >
          <Segmented
            id="pref-ui-font-size"
            value={uiFontSize}
            options={FONT_SIZE_OPTIONS}
            onChange={(v) => setPref("uiFontSize", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Application">
        <ReopenWorkspacesRow />
        <StartupConnectionRow />
        <NewTabOpensRow />
        <LoadingWallpaperRow />
      </SettingGroup>

      <SettingGroup title="Tables">
        <SettingRow
          id="pref-page-size"
          label="Default page size"
          description="Rows per page in new table tabs."
        >
          <Segmented
            id="pref-page-size"
            value={pageSize}
            options={PAGE_SIZE_OPTIONS}
            onChange={(v) => setPref("pageSize", v)}
          />
        </SettingRow>
        <SettingRow
          id="pref-confirm-commit"
          label="Confirm before commit"
          description={
            confirmCommit ? (
              <>
                <InlineKbd combo={commitCombo} /> opens a SQL preview before applying staged
                changes.
              </>
            ) : (
              <>
                <InlineKbd combo={commitCombo} /> applies staged changes immediately;{" "}
                <InlineKbd combo={shortcutFor("grid.preview") ?? ""} /> previews the SQL first.
              </>
            )
          }
        >
          <Switch
            id="pref-confirm-commit"
            checked={confirmCommit}
            onCheckedChange={(v) => setPref("confirmCommit", v)}
            aria-describedby="pref-confirm-commit-description"
          />
        </SettingRow>
      </SettingGroup>
      <TableDataSettings />
      <LazyLoadSettingRow />
      <NotifySettingRows />
      <CsvDefaultSettings />
      <DataDecoderSettings />
    </SectionShell>
  );
}

/** A run of the preview in the colour of one SQL editor token (Settings ▸ Appearance ▸ SQL editor). */
function Tok({
  token,
  italic,
  children,
}: {
  token: string;
  italic?: boolean;
  children: ReactNode;
}) {
  return (
    <span style={{ color: `var(--${token})`, fontStyle: italic ? "italic" : undefined }}>
      {children}
    </span>
  );
}

/**
 * The editor preview: sample SQL in the SQL editor's own font family, size and line height and
 * every syntax colour (comment, keyword, number, the four quote kinds, others, error, invisible
 * characters, the current statement's and the selection's background), read live from the same
 * custom properties the editor uses, so it follows Settings ▸ Appearance (font, line height,
 * colours, theme) as well as the options of this section (tab size, wrapping, keyword case).
 */
export function EditorPreview() {
  const fontSize = usePrefs((p) => p.editorFontSize);
  const tabSize = usePrefs((p) => p.editorTabSize);
  const wrap = usePrefs((p) => p.editorLineWrap);
  const upper = usePrefs((p) => p.editorUppercaseKeywords);
  const invisibles = usePrefs((p) => p.editorShowInvisibles);
  const kw = (word: string) => (
    <Tok token="syn-keyword">{upper ? word.toUpperCase() : word.toLowerCase()}</Tok>
  );
  const indent = (invisibles ? "·" : " ").repeat(tabSize);
  // Invisible characters: a pilcrow-like mark at the line ends.
  const eol = invisibles ? <Tok token="syn-invisible">¬</Tok> : null;
  return (
    <pre
      aria-label="Editor preview"
      className="border-border bg-surface selectable overflow-x-auto rounded-lg border px-3 py-2.5"
      style={{
        fontSize,
        fontFamily: "var(--editor-font-family)",
        lineHeight: "var(--editor-line-height)",
        color: "var(--syn-other)",
        whiteSpace: wrap ? "pre-wrap" : "pre",
        overflowWrap: "anywhere",
      }}
    >
      <Tok token="syn-comment" italic>
        -- Recently active customers who have a verified email address and a paid order
      </Tok>
      {eol}
      {"\n"}
      <span className="-mx-3 block px-3" style={{ backgroundColor: "var(--editor-statement-bg)" }}>
        {kw("select")} id, email, <Tok token="syn-string-double">"Created At"</Tok>,{" "}
        <Tok token="syn-string-backtick">`region`</Tok>,{" "}
        <Tok token="syn-string-bracket">[plan]</Tok>
        {eol}
        {"\n"}
        {kw("from")}{" "}
        <span style={{ backgroundColor: "var(--editor-selection-bg)" }}>customers</span>
        {eol}
        {"\n"}
        {kw("where")} verified {kw("and")} ({eol}
        {"\n"}
        {indent}last_seen_at {">"} now() - {kw("interval")} <Tok token="syn-string">'7 days'</Tok>
        {eol}
        {"\n"}){eol}
        {"\n"}
        {kw("order by")} created_at {kw("desc")} {kw("limit")} <Tok token="syn-number">100</Tok>;
      </span>
      <span
        className="underline decoration-wavy underline-offset-2"
        style={{ color: "var(--syn-error)", textDecorationColor: "var(--syn-error)" }}
      >
        {upper ? "SELEC" : "selec"}
      </span>{" "}
      1;
    </pre>
  );
}

export function EditorSection() {
  const fontSize = usePrefs((p) => p.editorFontSize);
  const tabSize = usePrefs((p) => p.editorTabSize);
  const wrap = usePrefs((p) => p.editorLineWrap);
  const upper = usePrefs((p) => p.editorUppercaseKeywords);
  const formatShortcut = shortcutFor("query.format") ?? "Mod+I";
  return (
    <SectionShell title="Editor">
      <SettingGroup title="SQL editor">
        <SettingRow id="pref-editor-font-size" label="Font size">
          <Select
            value={String(fontSize)}
            onValueChange={(v) => setPref("editorFontSize", Number(v))}
          >
            <SelectTrigger
              id="pref-editor-font-size"
              size="sm"
              className="h-7 w-22 rounded-md text-xs tabular-nums"
            >
              <SelectValue />
            </SelectTrigger>
            {/* The listbox is portaled outside [role=dialog]; keep it in the dialog's keyboard
                scope so global shortcuts (⌘1, ⌘B, ⌘⇧W…) don't reach the workspace behind the
                modal and ⌘1–4 / ⌘F still bubble to the dialog through the React tree. */}
            <SelectContent data-scope="dialog">
              {EDITOR_FONT_SIZES.map((n) => (
                <SelectItem key={n} value={String(n)} className="text-xs tabular-nums">
                  {n} px
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingRow>
        <SettingRow id="pref-editor-tab-size" label="Tab size" description="Spaces per indent.">
          <Segmented
            id="pref-editor-tab-size"
            value={tabSize}
            options={TAB_SIZE_OPTIONS}
            onChange={(v) => setPref("editorTabSize", v)}
          />
        </SettingRow>
        <SettingRow
          id="pref-editor-wrap"
          label="Wrap long lines"
          description="Soft-wrap instead of scrolling horizontally."
        >
          <Switch
            id="pref-editor-wrap"
            checked={wrap}
            onCheckedChange={(v) => setPref("editorLineWrap", v)}
          />
        </SettingRow>
        <SettingRow
          id="pref-editor-upper"
          label="Uppercase keywords"
          description={
            <>
              Beautify SQL <InlineKbd combo={formatShortcut} /> writes SELECT, FROM, WHERE in
              capitals.
            </>
          }
        >
          <Switch
            id="pref-editor-upper"
            checked={upper}
            onCheckedChange={(v) => setPref("editorUppercaseKeywords", v)}
            aria-describedby="pref-editor-upper-description"
          />
        </SettingRow>
        <AutoSaveQueryTabsRow />
      </SettingGroup>
      <EditorWorkflowSettings />
      <section className="flex flex-col gap-1.5">
        <h3 className="text-muted-foreground px-1 text-[11px] font-medium">Preview</h3>
        <EditorPreview />
      </section>
    </SectionShell>
  );
}

/**
 * TablePlus's "Query time out": a statement running longer is cancelled on the server. It covers
 * every query the app runs for the user: the SQL editor, a table's page loads, row counts, cell
 * fetches and Explain (the backend reads this preference).
 */
export const QUERY_TIMEOUT_DESCRIPTION =
  "Applies to every query: the SQL editor, table data, row counts and Explain. 0 = no limit.";
function QueryTimeoutRow() {
  const timeout = usePrefs((p) => p.queryTimeout);
  return (
    <SettingRow
      id="pref-query-timeout"
      label="Query time out"
      description={QUERY_TIMEOUT_DESCRIPTION}
    >
      <NumberField
        id="pref-query-timeout"
        value={timeout}
        min={QUERY_TIMEOUT_RANGE.min}
        max={QUERY_TIMEOUT_RANGE.max}
        suffix="seconds"
        onCommit={(v) => setPref("queryTimeout", v)}
      />
    </SettingRow>
  );
}

/** TablePlus's "Keep connection alive" (on by default). */
function KeepAliveRow() {
  const keepAlive = usePrefs((p) => p.keepAlive);
  return (
    <SettingRow
      id="pref-keep-alive"
      label="Keep connection alive (ping every 30 s)"
      description="Stops servers and firewalls from dropping idle connections. When off, a health check still pings every 60 s."
    >
      <Switch
        id="pref-keep-alive"
        checked={keepAlive}
        onCheckedChange={(v) => setPref("keepAlive", v)}
      />
    </SettingRow>
  );
}

export function ConnectionsSection() {
  const timeout = usePrefs((p) => p.connectTimeout);
  return (
    <SectionShell title="Connections">
      <SettingGroup title="Connecting">
        <SettingRow
          id="pref-connect-timeout"
          label="Connect timeout"
          description="Default for new connections."
        >
          <NumberField
            id="pref-connect-timeout"
            value={timeout}
            min={CONNECT_TIMEOUT_RANGE.min}
            max={CONNECT_TIMEOUT_RANGE.max}
            suffix="seconds"
            onCommit={(v) => setPref("connectTimeout", v)}
          />
        </SettingRow>
        <QueryTimeoutRow />
        <KeepAliveRow />
      </SettingGroup>
      <SettingGroup title="Security">
        <div className="flex items-start gap-2.5 px-3 py-2.5">
          <KeyRound className="text-muted-foreground mt-0.5 size-4 shrink-0" />
          <div className="flex flex-col gap-0.5">
            <p className="text-[13px] leading-5">Passwords are stored in the macOS Keychain</p>
            <p className="text-muted-foreground text-[11px] leading-4">
              Connection profiles never contain secrets.
            </p>
          </div>
        </div>
      </SettingGroup>
    </SectionShell>
  );
}

export function SecuritySection() {
  const defaultSafeMode = usePrefs((p) => p.defaultSafeMode);
  const selected = SAFE_MODE_OPTIONS.find((o) => o.level === defaultSafeMode);
  return (
    <SectionShell title="Security">
      <SettingGroup title="Safe mode">
        <SettingRow
          id="pref-default-safe-mode"
          label="Default safe mode for new connections"
          description={`${selected?.description ?? ""}. Change a connection's level from the lock in the toolbar.`}
        >
          <Select
            value={defaultSafeMode}
            onValueChange={(v) => setPref("defaultSafeMode", v as Prefs["defaultSafeMode"])}
          >
            <SelectTrigger
              id="pref-default-safe-mode"
              size="sm"
              className="h-7 w-32 rounded-md text-xs"
            >
              <SelectValue />
            </SelectTrigger>
            {/* Kept in the dialog's keyboard scope, like the editor font size list. */}
            <SelectContent data-scope="dialog">
              {SAFE_MODE_OPTIONS.map((o) => (
                <SelectItem key={o.level} value={o.level} className="text-xs">
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingRow>
        <SafeModeTouchIdRow />
      </SettingGroup>
      <PasscodeSettings />
    </SectionShell>
  );
}
