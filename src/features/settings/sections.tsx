import { KeyRound, Monitor, Moon, Sun } from "lucide-react";
import type { ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { shortcutFor } from "@/lib/keys/keymap";
import {
  CONNECT_TIMEOUT_RANGE,
  EDITOR_FONT_SIZE_RANGE,
  PAGE_SIZES,
  setPref,
  TAB_SIZES,
  UI_FONT_SIZES,
  usePrefs,
  type UiFontSize,
} from "@/stores/prefs";
import { useUiStore, type ThemePref } from "@/stores/ui";
import { NumberField, SectionShell, Segmented, SettingGroup, SettingRow } from "./controls";

const THEME_OPTIONS = [
  { value: "system", label: "System", icon: <Monitor /> },
  { value: "light", label: "Light", icon: <Sun /> },
  { value: "dark", label: "Dark", icon: <Moon /> },
] as const satisfies readonly { value: ThemePref; label: string; icon: ReactNode }[];

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
  const setThemePref = useUiStore((s) => s.setThemePref);
  const uiFontSize = usePrefs((p) => p.uiFontSize);
  const pageSize = usePrefs((p) => p.pageSize);
  const confirmCommit = usePrefs((p) => p.confirmCommit);
  const commitCombo = shortcutFor("grid.commit") ?? "Mod+S";
  return (
    <SectionShell title="General">
      <SettingGroup title="Appearance">
        <SettingRow id="pref-theme" label="Theme">
          <Segmented
            id="pref-theme"
            value={themePref}
            options={THEME_OPTIONS}
            onChange={setThemePref}
          />
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
    </SectionShell>
  );
}

const kwClass = "text-[color:var(--syn-keyword)]";

function EditorPreview() {
  const fontSize = usePrefs((p) => p.editorFontSize);
  const tabSize = usePrefs((p) => p.editorTabSize);
  const wrap = usePrefs((p) => p.editorLineWrap);
  const upper = usePrefs((p) => p.editorUppercaseKeywords);
  const kw = (word: string) => (
    <span className={kwClass}>{upper ? word.toUpperCase() : word.toLowerCase()}</span>
  );
  const indent = " ".repeat(tabSize);
  return (
    <pre
      aria-label="Editor preview"
      className="border-border bg-surface text-foreground selectable overflow-x-auto rounded-lg border px-3 py-2.5 font-mono leading-[1.6]"
      style={{ fontSize, whiteSpace: wrap ? "pre-wrap" : "pre", overflowWrap: "anywhere" }}
    >
      <span className="text-[color:var(--syn-comment)]">
        -- Recently active customers who have a verified email address and at least one paid order
      </span>
      {"\n"}
      {kw("select")} id, email, created_at{"\n"}
      {kw("from")} customers{"\n"}
      {kw("where")} verified {kw("and")} ({"\n"}
      {indent}last_seen_at {">"} now() - {kw("interval")}{" "}
      <span className="text-[color:var(--syn-string)]">'7 days'</span>
      {"\n"}){"\n"}
      {kw("order by")} created_at {kw("desc")} {kw("limit")}{" "}
      <span className="text-[color:var(--syn-number)]">100</span>;
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
      </SettingGroup>
      <section className="flex flex-col gap-1.5">
        <h3 className="text-muted-foreground px-1 text-[11px] font-medium">Preview</h3>
        <EditorPreview />
      </section>
    </SectionShell>
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
