import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CaseSensitive,
  CircleCheck,
  CircleX,
  History,
  Loader2,
  Play,
  Regex,
  Search,
  Trash2,
  WholeWord,
} from "lucide-react";
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Toggle } from "@/components/ui/toggle";
import { McpBadge } from "@/features/sidebar/McpBadge";
import {
  historyMatcher,
  isMcpEntry,
  readHistorySearchOptions,
  writeHistorySearchOptions,
  type HistorySearchOptions,
} from "@/features/sidebar/historyEntry";
import { formatDuration } from "@/lib/format/time";
import { ipc, toIpcError, type HistoryEntry } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import { focusEditor } from "./editorStates";
import { groupByDay, HISTORY_KEY, timeOfDay } from "./historyGroups";
import { plural, rowsLabel } from "./labels";

const SEARCH_DEBOUNCE_MS = 200;

function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

function EntryMeta({ e }: { e: HistoryEntry }) {
  const parts = [
    timeOfDay(e.startedAt),
    e.durationMs !== null ? formatDuration(e.durationMs) : null,
    e.database ? `${e.connectionName} / ${e.database}` : e.connectionName,
    e.affected !== null
      ? `${rowsLabel(e.affected)} affected`
      : e.rowCount !== null
        ? rowsLabel(e.rowCount)
        : null,
  ].filter(Boolean);
  return <span className="min-w-0 truncate tabular-nums">{parts.join(" · ")}</span>;
}

/** One of the search options (Match Case / Whole Word / Regex), shown as a toggle in the field. */
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
    <Toggle
      size="sm"
      pressed={pressed}
      onPressedChange={onPressedChange}
      aria-label={label}
      title={label}
      className="text-muted-foreground data-[state=on]:bg-brand-soft data-[state=on]:text-foreground h-5 min-w-5 rounded px-0.5 [&_svg:not([class*='size-'])]:size-3.5"
    >
      {children}
    </Toggle>
  );
}

const isTextField = (el: HTMLElement) =>
  el.closest("input, textarea, [contenteditable='true']") !== null;

/**
 * ⌘H: every statement the backend recorded, newest first, grouped by day. A click only selects the
 * entry; a double-click (or Enter) inserts the SQL at the cursor and NEVER runs it; ⌘↩ (or the
 * entry's Run button) runs it. The search field drives the list as a combobox (arrows, Enter, ⌘↩)
 * and has the History pane's options (Match Case, Whole Word, Regex; remembered and shared with
 * the pane); ⌘⌫ deletes the active entry while the list itself has focus.
 */
export function HistorySheet({
  tabId,
  open,
  onOpenChange,
  onInsert,
  onRun,
}: {
  tabId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onInsert: (sql: string) => void;
  onRun: (sql: string) => void;
}) {
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const [options, setOptionsState] = useState(readHistorySearchOptions);
  const term = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const needle = search.trim();
  const typedMatcher = historyMatcher(needle, options);
  const invalidRegex = typedMatcher === "invalid";
  const history = useQuery({
    queryKey: [...HISTORY_KEY, term, options],
    queryFn: () => ipc.history.list({ search: term || null, limit: 200, ...options }),
    enabled: open && historyMatcher(term, options) !== "invalid",
    placeholderData: (prev) => prev,
  });
  // The fetch is debounced and keeps showing the previous list until the new one lands. Filter by
  // what is typed right now, so the rows on screen (and the one Enter / ⌘↩ acts on) always match.
  const entries = useMemo(
    () =>
      typedMatcher === "invalid"
        ? []
        : typeof typedMatcher === "function"
          ? (history.data ?? []).filter((e) => typedMatcher(e.sql))
          : (history.data ?? []),
    // The matcher is rebuilt each render; what it depends on is listed instead.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [history.data, needle, options],
  );
  const setOptions = (patch: Partial<HistorySearchOptions>) =>
    setOptionsState((o) => {
      const next = { ...o, ...patch };
      writeHistorySearchOptions(next);
      return next;
    });
  // The History pane may have changed them since the sheet last opened.
  useEffect(() => {
    if (open) setOptionsState(readHistorySearchOptions());
  }, [open]);
  const settled = term === search.trim() && !history.isPlaceholderData;
  const groups = useMemo(() => groupByDay(entries), [entries]);
  const [activeIndex, setActiveIndex] = useState(0);
  const active = Math.max(0, Math.min(activeIndex, entries.length - 1));
  const listRef = useRef<HTMLDivElement>(null);
  /** An entry was picked and the sheet is closing: ignore the clicks that trail behind. */
  const closing = useRef(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const listboxId = useId();
  const optionId = (e: HistoryEntry) => `${listboxId}-entry-${e.id}`;
  const activeEntry = entries[active];
  const activeDescendant = activeEntry ? optionId(activeEntry) : undefined;

  useEffect(() => setActiveIndex(0), [needle, options, open]);
  useEffect(() => {
    if (open) closing.current = false;
  }, [open]);
  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const invalidate = () => void qc.invalidateQueries({ queryKey: HISTORY_KEY });
  const remove = useMutation({
    mutationFn: (ids: number[]) => ipc.history.delete(ids),
    onSuccess: invalidate,
    onError: (e) =>
      toast.error("Could not delete the entry", { description: toIpcError(e).message }),
  });
  const clear = useMutation({
    mutationFn: () => ipc.history.clear(null),
    onSuccess: () => {
      invalidate();
      toast.message("Query history cleared");
    },
    onError: (e) => toast.error("Could not clear history", { description: toIpcError(e).message }),
  });

  /** Closes the sheet and inserts / runs the entry, once: later clicks of the gesture are ignored. */
  const pick = (e: HistoryEntry | undefined, then: (sql: string) => void) => {
    if (!e || closing.current) return;
    closing.current = true;
    onOpenChange(false);
    then(e.sql);
  };
  const insert = (e: HistoryEntry | undefined) => pick(e, onInsert);
  const run = (e: HistoryEntry | undefined) => pick(e, onRun);

  const onKeyDown = (ev: KeyboardEvent<HTMLElement>) => {
    const target = ev.target as HTMLElement;
    // Buttons (delete, clear, close) handle their own keys.
    if (target.closest("button")) return;
    const mod = ev.metaKey || ev.ctrlKey;
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const delta = ev.key === "ArrowDown" ? 1 : -1;
      setActiveIndex(Math.max(0, Math.min(entries.length - 1, active + delta)));
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      if (mod) run(activeEntry);
      else insert(activeEntry);
    } else if (ev.key === "Backspace" && mod) {
      // In the search field ⌘⌫ deletes the typed text; only the focused list deletes entries.
      if (isTextField(target) || !activeEntry) return;
      ev.preventDefault();
      remove.mutate([activeEntry.id]);
    }
  };

  let index = 0;
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="flex w-[460px] max-w-full flex-col gap-0 p-0 sm:max-w-[460px]"
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          focusEditor(tabId);
        }}
        onKeyDown={onKeyDown}
      >
        <SheetHeader className="border-border gap-2 border-b px-4 pt-4 pb-3">
          <SheetTitle className="flex items-center gap-2 text-sm">
            <History className="text-muted-foreground size-4" /> Query history
          </SheetTitle>
          <SheetDescription className="text-xs">
            Double-click or <Kbd combo="Enter" className="align-middle" /> inserts at the cursor,{" "}
            <Kbd combo="Mod+Enter" className="align-middle" /> runs.
          </SheetDescription>
          <div className="relative">
            <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
            <Input
              autoFocus
              role="combobox"
              aria-controls={listboxId}
              aria-expanded={entries.length > 0}
              aria-autocomplete="list"
              aria-activedescendant={activeDescendant}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={options.regex ? "Search SQL (regex)…" : "Search SQL…"}
              aria-label="Search history"
              aria-invalid={invalidRegex || undefined}
              className="h-7 pr-20 pl-7 text-xs md:text-xs"
            />
            <div className="absolute top-1/2 right-1 flex -translate-y-1/2 items-center gap-px">
              {history.isFetching && (
                <Loader2 className="text-muted-foreground mr-1 size-3.5 animate-spin" />
              )}
              <OptionToggle
                pressed={options.matchCase}
                onPressedChange={(on) => setOptions({ matchCase: on })}
                label="Match Case"
              >
                <CaseSensitive />
              </OptionToggle>
              <OptionToggle
                pressed={options.wholeWord}
                onPressedChange={(on) => setOptions({ wholeWord: on })}
                label="Whole Word"
              >
                <WholeWord />
              </OptionToggle>
              <OptionToggle
                pressed={options.regex}
                onPressedChange={(on) => setOptions({ regex: on })}
                label="Regex"
              >
                <Regex />
              </OptionToggle>
            </div>
          </div>
        </SheetHeader>

        <div
          ref={listRef}
          id={listboxId}
          className="focus-visible:ring-brand/40 min-h-0 flex-1 overflow-auto outline-none focus-visible:ring-1 focus-visible:ring-inset"
          role="listbox"
          aria-label="History"
          tabIndex={0}
          aria-activedescendant={activeDescendant}
        >
          {invalidRegex ? (
            <EmptyState
              title="Invalid regular expression"
              description="Fix the pattern, or turn Regex off."
            />
          ) : history.isError ? (
            <EmptyState title="Could not load history" description={history.error.message} />
          ) : entries.length === 0 && settled && !history.isPending ? (
            <EmptyState
              icon={History}
              title={term ? "No matches" : "No history yet"}
              description={
                term ? `Nothing you ran contains “${term}”.` : "Statements you run appear here."
              }
            />
          ) : (
            groups.map((g) => (
              <div key={g.key} role="group" aria-labelledby={`${listboxId}-day-${g.key}`}>
                <div
                  id={`${listboxId}-day-${g.key}`}
                  aria-hidden
                  className="bg-surface-2/95 text-muted-foreground border-border sticky top-0 z-[1] border-b px-4 py-1 text-[10px] font-semibold tracking-wider uppercase backdrop-blur"
                >
                  {g.label}
                </div>
                {g.entries.map((e) => {
                  const i = index++;
                  const selected = i === active;
                  return (
                    <div
                      key={e.id}
                      id={optionId(e)}
                      role="option"
                      aria-selected={selected}
                      data-index={i}
                      className={cn(
                        "group border-gridline cursor-pointer border-b px-4 py-2",
                        selected ? "bg-brand-soft" : "hover:bg-row-hover",
                      )}
                      // A click selects; the double-click inserts (it never runs the statement).
                      onClick={() => {
                        if (!closing.current) setActiveIndex(i);
                      }}
                      onDoubleClick={() => insert(e)}
                    >
                      <div className="text-muted-foreground flex items-center gap-1.5 text-[11px]">
                        {e.success ? (
                          <CircleCheck
                            className="text-success size-3 shrink-0"
                            aria-label="Succeeded"
                          />
                        ) : (
                          <CircleX className="text-error size-3 shrink-0" aria-label="Failed" />
                        )}
                        <EntryMeta e={e} />
                        {e.source === "explain" && (
                          <span className="border-border shrink-0 rounded border px-1 text-[9px] uppercase">
                            explain
                          </span>
                        )}
                        {isMcpEntry(e) && <McpBadge />}
                        {/* Pointer only: from the keyboard, ⌘↩ runs and ⌘⌫ deletes the active entry. */}
                        <button
                          type="button"
                          tabIndex={-1}
                          aria-hidden
                          data-testid="history-run"
                          title={`Run (${formatCombo("Mod+Enter")})`}
                          className="hover:text-foreground ml-auto shrink-0 opacity-0 group-hover:opacity-100"
                          onClick={(ev) => {
                            ev.stopPropagation();
                            run(e);
                          }}
                        >
                          <Play className="size-3.5" />
                        </button>
                        <button
                          type="button"
                          tabIndex={-1}
                          aria-hidden
                          title={`Delete entry (${formatCombo("Mod+Backspace")})`}
                          className="hover:text-error shrink-0 opacity-0 group-hover:opacity-100"
                          onClick={(ev) => {
                            ev.stopPropagation();
                            remove.mutate([e.id]);
                          }}
                        >
                          <Trash2 className="size-3.5" />
                        </button>
                      </div>
                      <pre className="text-foreground/90 mt-1 line-clamp-3 font-mono text-[11px] leading-4 break-words whitespace-pre-wrap">
                        {e.sql}
                      </pre>
                      {!e.success && e.error && (
                        <div className="text-error mt-0.5 truncate text-[11px]" title={e.error}>
                          {e.error}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            ))
          )}
        </div>

        <div className="border-border text-muted-foreground flex h-10 shrink-0 items-center gap-2 border-t px-4 text-[11px]">
          <span className="tabular-nums">
            {entries.length === 200
              ? "Latest 200 entries"
              : plural(entries.length, "entry", "entries")}
          </span>
          <span className="flex-1" />
          <Button
            variant="ghost"
            size="xs"
            className="hover:text-error"
            disabled={entries.length === 0 || clear.isPending}
            onClick={() => setConfirmClear(true)}
          >
            <Trash2 /> Clear all
          </Button>
        </div>
      </SheetContent>

      <AlertDialog open={confirmClear} onOpenChange={setConfirmClear}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Clear query history?</AlertDialogTitle>
            <AlertDialogDescription>
              Every recorded statement, for all connections, is deleted. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => clear.mutate()}>
              Clear history
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Sheet>
  );
}
