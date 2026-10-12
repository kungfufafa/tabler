import type { ReactNode } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import {
  QUERY_PARAM_SYNTAXES,
  setPref,
  usePrefs,
  type Prefs,
  type QueryParamSyntax,
} from "@/stores/prefs";
import { QUERY_PARAM_EXAMPLES } from "./editorSettings";

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

/**
 * Settings ▸ Editor rows that mirror the SQL editor's ⚙ menu: highlighting, invisible characters,
 * autocomplete, query parameters and Return on Error (the same preferences).
 */
export function EditorWorkflowSettings() {
  return (
    <>
      <SettingGroup title="Editing">
        <SwitchRow
          pref="editorHighlightCurrentQuery"
          label="Highlight current query"
          description="Tints the statement under the cursor (what ⌘↩ runs)."
        />
        <SwitchRow pref="editorShowInvisibles" label="Show invisible characters" />
        <SwitchRow
          pref="editorDisableSuggestions"
          invert
          label="Suggest while typing"
          description="Off: completions only open with ⌃Space."
        />
        <SwitchRow pref="editorCloseBrackets" label="Insert closing braces and quotes" />
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
      </SettingGroup>
    </>
  );
}
