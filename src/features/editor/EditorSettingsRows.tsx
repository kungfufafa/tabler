import type { ReactNode } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Segmented, SettingGroup, SettingRow } from "@/features/settings/controls";
import { formatCombo, isMac } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import {
  EDITOR_ACCEPT_KEYS,
  HISTORY_RETENTIONS,
  NEW_TAB_OPENS,
  QUERY_PARAM_SYNTAXES,
  setPref,
  usePrefs,
  type Prefs,
  type QueryParamSyntax,
} from "@/stores/prefs";
import {
  ACCEPT_KEY_LABELS,
  HISTORY_RETENTION_LABELS,
  NEW_TAB_LABELS,
  QUERY_PARAM_EXAMPLES,
} from "./editorSettings";

type BoolPref = {
  [K in keyof Prefs]: Prefs[K] extends boolean ? K : never;
}[keyof Prefs];

function SwitchRow({
  pref,
  label,
  description,
  invert = false,
}: {
  pref: BoolPref;
  label: string;
  description?: ReactNode;
  /** The switch shows the opposite of the preference ("Disable…" preferences). */
  invert?: boolean;
}) {
  const value = usePrefs((p) => p[pref]);
  const id = `pref-${pref}`;
  return (
    <SettingRow id={id} label={label} description={description}>
      <Switch
        id={id}
        checked={invert ? !value : value}
        onCheckedChange={(v) => setPref(pref, invert ? !v : v)}
        aria-describedby={description ? `${id}-description` : undefined}
      />
    </SettingRow>
  );
}

function ParamSyntaxRow() {
  const syntax = usePrefs((p) => p.editorQueryParamsRegex);
  const enabled = usePrefs((p) => p.editorQueryParams);
  return (
    <SettingRow
      id="pref-editorQueryParamsRegex"
      label="Query params regex"
      description="What a variable looks like."
    >
      <Select
        value={syntax}
        onValueChange={(v) => setPref("editorQueryParamsRegex", v as QueryParamSyntax)}
        disabled={!enabled}
      >
        <SelectTrigger
          id="pref-editorQueryParamsRegex"
          size="sm"
          aria-describedby="pref-editorQueryParamsRegex-description"
          className="h-7 w-44 rounded-md font-mono text-xs"
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent data-scope="dialog">
          {QUERY_PARAM_SYNTAXES.map((s) => (
            <SelectItem key={s} value={s} className="font-mono text-xs">
              {s}
              <span className="text-muted-foreground ml-2">{QUERY_PARAM_EXAMPLES[s]}</span>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </SettingRow>
  );
}

/** A select over one string preference. */
function SelectRow<K extends keyof Prefs>({
  pref,
  label,
  description,
  values,
  labels,
  width = "w-40",
}: {
  pref: K;
  label: string;
  description?: ReactNode;
  values: readonly Extract<Prefs[K], string>[];
  labels: Record<Extract<Prefs[K], string>, string>;
  width?: string;
}) {
  const value = usePrefs((p) => p[pref]);
  const id = `pref-${pref}`;
  return (
    <SettingRow id={id} label={label} description={description}>
      <Select value={String(value)} onValueChange={(v) => setPref(pref, v as Prefs[K])}>
        <SelectTrigger
          id={id}
          size="sm"
          aria-describedby={description ? `${id}-description` : undefined}
          className={`h-7 ${width} rounded-md text-xs`}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent data-scope="dialog">
          {values.map((v) => (
            <SelectItem key={v} value={v} className="text-xs">
              {labels[v]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </SettingRow>
  );
}

const INDENT_OPTIONS = [
  { value: "spaces", label: "Spaces" },
  { value: "tabs", label: "Tabs" },
] as const;

/** "Indent with: Spaces / Tabs" (the width is the tab size above). */
function IndentRow() {
  const tabs = usePrefs((p) => p.editorIndentWithTabs);
  const size = usePrefs((p) => p.editorTabSize);
  return (
    <SettingRow
      id="pref-editorIndentWithTabs"
      label="Indent with"
      description={`${tabs ? "A tab" : `${size} spaces`} per indent level (Tab key, Beautify).`}
    >
      <Segmented
        id="pref-editorIndentWithTabs"
        value={tabs ? "tabs" : "spaces"}
        options={INDENT_OPTIONS}
        onChange={(v) => setPref("editorIndentWithTabs", v === "tabs")}
      />
    </SettingRow>
  );
}

/** General ▸ Application: "Command + T should open" (TablePlus). */
export function NewTabOpensRow() {
  return (
    <SelectRow
      pref="newTabOpens"
      label="Command + T should open"
      description="A SQL Query Editor tab brings back the connection's most recent script when none of its query tabs is open; an empty tab never does."
      values={NEW_TAB_OPENS}
      labels={NEW_TAB_LABELS}
      width="w-44"
    />
  );
}

/**
 * Settings ▸ Editor rows that mirror the SQL editor's ⚙ menu: highlighting, invisible characters,
 * indentation, autocomplete, files, query parameters, Return on Error, history and saved queries
 * (the same preferences).
 */
export function EditorWorkflowSettings() {
  const runKeys = formatCombo(shortcutFor("query.run-current") ?? "Mod+Enter");
  const saveKeys = formatCombo(shortcutFor("query.save") ?? "Mod+S");
  return (
    <>
      <SettingGroup title="Editing">
        <SwitchRow
          pref="editorHighlightCurrentQuery"
          label="Highlight current query"
          description={`Tints the statement under the cursor (what ${runKeys} runs).`}
        />
        <SwitchRow pref="editorShowInvisibles" label="Show invisible characters" />
        <SwitchRow pref="editorShowScrollers" label="Show scrollers" />
        <IndentRow />
        <SwitchRow pref="editorCloseBrackets" label="Insert closing braces and quotes" />
        <SwitchRow
          pref="editorAutoSaveFiles"
          label="Automatically save queries while editing"
          description={`A query tab opened from (or saved to) a .sql file saves to it as you type. Off: ${saveKeys} saves.`}
        />
      </SettingGroup>
      <SettingGroup title="Autocomplete">
        <SwitchRow
          pref="editorDisableSuggestions"
          invert
          label="Suggest while typing"
          description={`Off: completions only open with ${isMac ? "⌃Space" : "Ctrl+Space"}.`}
        />
        <SwitchRow pref="editorCompleteTables" label="Suggest tables and columns" />
        <SwitchRow pref="editorCompleteKeywords" label="Suggest keywords" />
        <SwitchRow
          pref="editorPrefixSchemas"
          label="Automatically prefix schemas"
          description="Tables complete as schema.table, those of every schema included."
        />
        <SwitchRow
          pref="editorAddWhitespace"
          label="Automatically add a whitespace"
          description="A space follows an accepted suggestion."
        />
        <SelectRow
          pref="editorAcceptKey"
          label="Auto-complete key"
          values={EDITOR_ACCEPT_KEYS}
          labels={ACCEPT_KEY_LABELS}
        />
      </SettingGroup>
      <SettingGroup title="Query parameters">
        <SwitchRow
          pref="editorQueryParams"
          label="Enable query params"
          description="Running SQL with variables (e.g. :id) asks for their values first."
        />
        <ParamSyntaxRow />
        <SwitchRow
          pref="editorQueryParamsInQuotes"
          label="Search for variables in quoted strings"
        />
      </SettingGroup>
      <SettingGroup title="Running">
        <SwitchRow
          pref="editorStopOnError"
          label="Return on error"
          description="A run of several statements stops at the first error."
        />
        <SwitchRow
          pref="editorRaiseNotices"
          label="Raise notice notifications"
          description="PostgreSQL: RAISE NOTICE / WARNING of a running query show as notifications (Messages lists them either way)."
        />
      </SettingGroup>
      <SettingGroup title="History and queries">
        <SelectRow
          pref="historyRetention"
          label="Keep history for"
          description="Older entries are deleted when Tabler starts."
          values={HISTORY_RETENTIONS}
          labels={HISTORY_RETENTION_LABELS}
          width="w-28"
        />
        <SwitchRow
          pref="queriesDoubleClickInsert"
          label="Double click to insert a saved query"
          description="Off: a double-click runs it in a new tab."
        />
        <SwitchRow
          pref="queriesAutoSave"
          label="Auto save favorite"
          description={`The saved query editor saves while you type (no ${formatCombo("Mod+S")}).`}
        />
      </SettingGroup>
    </>
  );
}
