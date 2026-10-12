import { Settings } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import {
  EDITOR_FONT_SIZE_RANGE,
  QUERY_PARAM_SYNTAXES,
  setPref,
  usePrefs,
  type Prefs,
  type QueryParamSyntax,
} from "@/stores/prefs";
import { changeEditorFontSize, EDITOR_FONT_SIZES, QUERY_PARAM_EXAMPLES } from "./editorSettings";

type BoolPref = {
  [K in keyof Prefs]: Prefs[K] extends boolean ? K : never;
}[keyof Prefs];

function Check({ pref, label }: { pref: BoolPref; label: string }) {
  const checked = usePrefs((p) => p[pref]);
  return (
    <DropdownMenuCheckboxItem
      checked={checked}
      onCheckedChange={(v) => setPref(pref, v === true)}
      // Several options are usually toggled in a row.
      onSelect={(e) => e.preventDefault()}
    >
      {label}
    </DropdownMenuCheckboxItem>
  );
}

function Shortcut({ action }: { action: Parameters<typeof shortcutFor>[0] }) {
  const combo = shortcutFor(action);
  return combo ? <DropdownMenuShortcut>{formatCombo(combo)}</DropdownMenuShortcut> : null;
}

/**
 * The ⚙ menu of the editor bar (TablePlus's SQL editor settings): font size, line wrapping, the
 * current-query highlight, invisible characters, autocomplete and query params options, Return
 * on Error. Every item is a preference shared by all query tabs (also in Settings ▸ Editor).
 */
export function EditorSettingsMenu() {
  const fontSize = usePrefs((p) => p.editorFontSize);
  const paramsOn = usePrefs((p) => p.editorQueryParams);
  const syntax = usePrefs((p) => p.editorQueryParamsRegex);
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Editor settings"
              className="text-muted-foreground hover:text-foreground data-[state=open]:bg-muted"
            >
              <Settings />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>Editor settings</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="start" side="top" className="w-72 text-xs">
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>Font Size</DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-48">
            <DropdownMenuItem
              disabled={fontSize >= EDITOR_FONT_SIZE_RANGE.max}
              onSelect={(e) => {
                e.preventDefault();
                changeEditorFontSize(fontSize, 1);
              }}
            >
              Bigger
              <Shortcut action="query.font-bigger" />
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={fontSize <= EDITOR_FONT_SIZE_RANGE.min}
              onSelect={(e) => {
                e.preventDefault();
                changeEditorFontSize(fontSize, -1);
              }}
            >
              Smaller
              <Shortcut action="query.font-smaller" />
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuRadioGroup
              value={String(fontSize)}
              onValueChange={(v) => setPref("editorFontSize", Number(v))}
            >
              {EDITOR_FONT_SIZES.map((n) => (
                <DropdownMenuRadioItem key={n} value={String(n)} className="tabular-nums">
                  {n} px
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <Check pref="editorLineWrap" label="Wrap Lines to Editor Width" />
        <Check pref="editorHighlightCurrentQuery" label="Highlight Current Query" />
        <Check pref="editorShowInvisibles" label="Show Invisible Characters" />
        <DropdownMenuSeparator />
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>Autocomplete Options</DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-80">
            <Check pref="editorDisableSuggestions" label="Disable All Suggestions" />
            <Check pref="editorUppercaseKeywords" label="Automatically Uppercase Keywords" />
            <Check
              pref="editorCloseBrackets"
              label="Automatically Insert Closing Braces and Quotes"
            />
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>Query Params Options</DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-80">
            <Check pref="editorQueryParams" label="Enable Query Params" />
            <DropdownMenuSub>
              <DropdownMenuSubTrigger disabled={!paramsOn}>
                Change Query Params Regex
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-56">
                <DropdownMenuLabel className="text-muted-foreground text-[11px] font-normal">
                  Variables look like
                </DropdownMenuLabel>
                <DropdownMenuRadioGroup
                  value={syntax}
                  onValueChange={(v) => setPref("editorQueryParamsRegex", v as QueryParamSyntax)}
                >
                  {QUERY_PARAM_SYNTAXES.map((s) => (
                    <DropdownMenuRadioItem key={s} value={s} className="font-mono">
                      {s}
                      <DropdownMenuShortcut className="font-mono tracking-normal">
                        {QUERY_PARAM_EXAMPLES[s]}
                      </DropdownMenuShortcut>
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            <DropdownMenuSeparator />
            <Check
              pref="editorQueryParamsInQuotes"
              label="Allow Searching for Variables in Quoted String"
            />
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <Check pref="editorStopOnError" label="Return on Error" />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
