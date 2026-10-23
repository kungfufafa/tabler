import {
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Regex,
  Search,
  WholeWord,
  X,
} from "lucide-react";
import {
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Toggle } from "@/components/ui/toggle";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { CellPos, GridColumn, ViewRow } from "@/features/datagrid/types";
import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import {
  compileQuery,
  counterLabel,
  findMatches,
  planReplace,
  rematch,
  stepMatch,
  type FindOptions,
  type ReplaceEdit,
} from "./model";
import { closeFind, setFind, useFind } from "./store";

export interface FindBarProps {
  tabId: string;
  /** The grid's rows, in its order (staged changes applied, new rows last). */
  rowCount: number;
  getRow: (r: number) => ViewRow | undefined;
  columns: readonly GridColumn[];
  /**
   * The SOURCE indexes of the columns the grid shows, in display order: only those are searched
   * (and replaced). Hidden columns are left alone.
   */
  visibleColumns: readonly number[];
  /** Why Replace is unavailable (a read-only result); null when the result can be edited. */
  readOnlyReason: string | null;
  /** Selects and scrolls to a match in the grid (`c` a SOURCE column index). */
  onReveal: (cell: CellPos) => void;
  /**
   * Stages the replacements (one undo step); false when nothing was staged (the result refused
   * them, and said why).
   */
  onReplace: (edits: ReplaceEdit[]) => boolean;
  /** Escape: the bar closed; the grid takes the focus back. */
  onClose: () => void;
}

function OptionToggle({
  pressed,
  onPressedChange,
  label,
  children,
}: {
  pressed: boolean;
  onPressedChange: (on: boolean) => void;
  label: string;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Toggle
          size="sm"
          pressed={pressed}
          onPressedChange={onPressedChange}
          aria-label={label}
          className="text-muted-foreground data-[state=on]:text-foreground h-6 min-w-6 rounded-md px-1 [&_svg:not([class*='size-'])]:size-3.5"
        >
          {children}
        </Toggle>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

const INPUT = "h-6 rounded-md px-2 text-xs md:text-xs";

/**
 * Find (and replace) in a SQL editor result, above its grid: ↩ / ⇧↩ step through the matches
 * (the grid selects and scrolls to each), ⎋ closes. Matching runs over the loaded rows as the grid
 * shows them; NULL never matches. Replace stages cell edits like typing them would, so it needs an
 * editable result; ⌘S commits them, ⌘Z undoes a Replace All in one step.
 */
export function FindBar({
  tabId,
  rowCount,
  getRow,
  columns,
  visibleColumns,
  readOnlyReason,
  onReveal,
  onReplace,
  onClose,
}: FindBarProps) {
  const find = useFind(tabId);
  const findRef = useRef<HTMLInputElement>(null);
  const text = useDeferredValue(find.text);
  const options: FindOptions = {
    matchCase: find.matchCase,
    wholeWord: find.wholeWord,
    regex: find.regex,
  };
  const compiled = useMemo(
    () =>
      compileQuery({
        text,
        matchCase: find.matchCase,
        wholeWord: find.wholeWord,
        regex: find.regex,
      }),
    [text, find.matchCase, find.wholeWord, find.regex],
  );
  const matches = useMemo(
    () =>
      compiled.kind === "ok" ? findMatches(rowCount, getRow, visibleColumns, compiled.matcher) : [],
    [compiled, rowCount, getRow, visibleColumns],
  );

  // The current match follows the cell it was on as rows stream in or edits change the matches.
  const [anchor, setAnchor] = useState<{ cell: CellPos | null; after: boolean }>({
    cell: null,
    after: false,
  });
  const [anchorQuery, setAnchorQuery] = useState(compiled);
  if (anchorQuery !== compiled) {
    // A new search starts at its first match.
    setAnchorQuery(compiled);
    setAnchor({ cell: null, after: false });
  }
  const current = useMemo(
    () => rematch(anchor.cell, matches, anchor.after, visibleColumns),
    [anchor, matches, visibleColumns],
  );

  // Typing (or a changed option) jumps to the first match.
  const matchesRef = useRef(matches);
  matchesRef.current = matches;
  useEffect(() => {
    const first = matchesRef.current[0];
    if (first) onReveal(first);
  }, [compiled, onReveal]);

  // ⌘F again (or the search icon) focuses the field with its text selected.
  useEffect(() => {
    findRef.current?.focus();
    findRef.current?.select();
  }, [find.focusSeq]);

  const step = (dir: 1 | -1) => {
    const i = stepMatch(current, matches.length, dir);
    const cell = i === null ? undefined : matches[i];
    if (!cell) return;
    setAnchor({ cell, after: false });
    onReveal(cell);
  };

  const canReplace = readOnlyReason === null && compiled.kind === "ok" && matches.length > 0;
  const replaceOne = () => {
    if (compiled.kind !== "ok" || current === null) return;
    const cell = matches[current];
    if (!cell) return;
    const plan = planReplace([cell], getRow, columns, compiled.matcher, find.replacement);
    if (plan.edits.length === 0) {
      if (plan.skipped > 0)
        toast.message("This value can't be replaced", {
          description: "Its column is read-only, or its type does not accept the new text.",
        });
      step(1);
      return;
    }
    if (!onReplace(plan.edits)) return;
    setAnchor({ cell, after: true });
    const next = matches.length > 1 ? matches[(current + 1) % matches.length] : undefined;
    if (next) onReveal(next);
  };
  const replaceAll = () => {
    if (compiled.kind !== "ok") return;
    const plan = planReplace(matches, getRow, columns, compiled.matcher, find.replacement);
    // Refused (another result holds changes, the edit target changed): its toast says so.
    if (plan.edits.length > 0 && !onReplace(plan.edits)) return;
    const n = plan.edits.length;
    toast.message(
      n === 0 ? "Nothing was replaced" : `Replaced ${n} value${n === 1 ? "" : "s"}`,
      plan.skipped > 0
        ? {
            description: `${plan.skipped} matching value${plan.skipped === 1 ? " was" : "s were"} left alone (read-only column, or a type that does not accept the new text).`,
          }
        : {},
    );
  };

  const close = () => {
    closeFind(tabId);
    onClose();
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "g" && (e.metaKey || e.ctrlKey) && !e.altKey) {
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    } else if (e.key === "f" && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      findRef.current?.focus();
      findRef.current?.select();
    }
  };
  const onFindKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Enter" || e.metaKey || e.ctrlKey || e.altKey) return;
    e.preventDefault();
    step(e.shiftKey ? -1 : 1);
  };
  const onReplaceKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Enter" || e.metaKey || e.ctrlKey || !canReplace) return;
    e.preventDefault();
    if (e.altKey) replaceAll();
    else replaceOne();
  };

  const counter =
    compiled.kind === "invalid"
      ? "Invalid regex"
      : compiled.kind === "empty"
        ? ""
        : counterLabel(current, matches.length);

  return (
    <div
      role="search"
      aria-label="Find in result"
      className="border-border bg-surface-2 flex shrink-0 flex-col gap-1 border-b px-1.5 py-1 text-xs"
      onKeyDown={onKeyDown}
    >
      <div className="flex min-w-0 items-center gap-1">
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={find.replacing ? "Hide Replace" : "Show Replace"}
          aria-expanded={find.replacing}
          title={find.replacing ? "Hide Replace" : "Show Replace"}
          className="text-muted-foreground hover:text-foreground"
          onClick={() => setFind(tabId, { replacing: !find.replacing })}
        >
          {find.replacing ? <ChevronDown /> : <ChevronRight />}
        </Button>
        <div className="relative w-64 min-w-32 shrink">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-1.5 size-3 -translate-y-1/2" />
          <Input
            ref={findRef}
            value={find.text}
            onChange={(e) => setFind(tabId, { text: e.target.value })}
            onKeyDown={onFindKey}
            placeholder="Find"
            aria-label="Find"
            aria-invalid={compiled.kind === "invalid" || undefined}
            spellCheck={false}
            autoComplete="off"
            className={cn(INPUT, "pl-6")}
          />
        </div>
        <OptionToggle
          pressed={options.matchCase}
          onPressedChange={(on) => setFind(tabId, { matchCase: on })}
          label="Match Case"
        >
          <CaseSensitive />
        </OptionToggle>
        <OptionToggle
          pressed={options.wholeWord}
          onPressedChange={(on) => setFind(tabId, { wholeWord: on })}
          label="Whole Word"
        >
          <WholeWord />
        </OptionToggle>
        <OptionToggle
          pressed={options.regex}
          onPressedChange={(on) => setFind(tabId, { regex: on })}
          label="Regular Expression"
        >
          <Regex />
        </OptionToggle>
        <span
          className={cn(
            "min-w-20 shrink-0 px-1 tabular-nums",
            compiled.kind === "invalid" ? "text-error" : "text-muted-foreground",
          )}
          role="status"
          aria-live="polite"
          title={compiled.kind === "invalid" ? compiled.error : undefined}
        >
          {counter}
        </span>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Previous Match"
          title={`Previous Match (${formatCombo("Shift+Enter")})`}
          disabled={matches.length === 0}
          className="text-muted-foreground hover:text-foreground"
          onClick={() => step(-1)}
        >
          <ChevronUp />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Next Match"
          title="Next Match (↩)"
          disabled={matches.length === 0}
          className="text-muted-foreground hover:text-foreground"
          onClick={() => step(1)}
        >
          <ChevronDown />
        </Button>
        <span className="flex-1" />
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Close Find"
          title="Close (⎋)"
          className="text-muted-foreground hover:text-foreground"
          onClick={close}
        >
          <X />
        </Button>
      </div>
      {find.replacing && (
        <div className="flex min-w-0 items-center gap-1 pl-7">
          <Input
            value={find.replacement}
            onChange={(e) => setFind(tabId, { replacement: e.target.value })}
            onKeyDown={onReplaceKey}
            placeholder="Replace"
            aria-label="Replace"
            spellCheck={false}
            autoComplete="off"
            className={cn(INPUT, "w-64 min-w-32 shrink")}
          />
          <Button
            variant="outline"
            size="xs"
            className="h-6 text-[11px]"
            disabled={!canReplace || current === null}
            title={readOnlyReason ?? "Replace this match and go to the next one (↩)"}
            onClick={replaceOne}
          >
            Replace
          </Button>
          <Button
            variant="outline"
            size="xs"
            className="h-6 text-[11px]"
            disabled={!canReplace}
            title={readOnlyReason ?? `Replace every match (${formatCombo("Alt+Enter")})`}
            onClick={replaceAll}
          >
            Replace All
          </Button>
          {readOnlyReason !== null && (
            <span className="text-muted-foreground min-w-0 truncate" title={readOnlyReason}>
              Read-only result
            </span>
          )}
        </div>
      )}
    </div>
  );
}
