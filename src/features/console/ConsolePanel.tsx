import { Copy, Trash2, X } from "lucide-react";
import { memo, useLayoutEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { formatCount, formatDuration } from "@/lib/format/time";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import {
  filterEntries,
  useConsoleStore,
  type ConsoleCategory,
  type ConsoleEntry,
  type ConsoleFilter,
} from "./store";

const FILTERS: Array<{ value: ConsoleFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "data", label: "Data" },
  { value: "structure", label: "Structure" },
  { value: "query", label: "Query" },
];

const CATEGORY: Record<ConsoleCategory, { label: string; className: string }> = {
  data: { label: "Data", className: "bg-brand-soft text-brand" },
  structure: { label: "Structure", className: "bg-warning/15 text-warning" },
  query: { label: "Query", className: "bg-success/15 text-success" },
};

/** SQL longer than this starts collapsed. */
const LONG_LINES = 6;
const LONG_CHARS = 600;

/** Local wall-clock time, `HH:MM:SS`. */
function clockTime(at: number): string {
  const d = new Date(at);
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
}

function stats(e: ConsoleEntry): string {
  const parts: string[] = [];
  if (e.durationMs !== undefined) parts.push(formatDuration(e.durationMs));
  if (e.rows !== undefined) parts.push(`${formatCount(e.rows)} row${e.rows === 1 ? "" : "s"}`);
  if (e.affected !== undefined) parts.push(`${formatCount(e.affected)} affected`);
  return parts.join(" · ");
}

function copySql(sql: string) {
  void writeClipboard(sql).then(
    () => toast.success("SQL copied"),
    (e: unknown) => toast.error("Could not copy", { description: (e as Error).message }),
  );
}

/**
 * TablePlus's console log (⇧⌘C): the SQL the app ran, oldest first, filtered by category. Sticks
 * to the newest entry while scrolled to the bottom, like a terminal.
 */
export function ConsolePanel() {
  const entries = useConsoleStore((s) => s.entries);
  const filter = useConsoleStore((s) => s.filter);
  const setFilter = useConsoleStore((s) => s.setFilter);
  const clear = useConsoleStore((s) => s.clear);
  const close = useUiStore((s) => s.toggleConsole);
  const shown = useMemo(() => filterEntries(entries, filter), [entries, filter]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [shown]);

  const keys = shortcutFor("ui.toggle-console");
  const filterLabel = FILTERS.find((f) => f.value === filter)?.label ?? "";

  return (
    <section aria-label="Console" className="bg-surface flex h-full min-h-0 flex-col">
      <div className="border-border flex h-8 shrink-0 items-center gap-2 overflow-hidden border-b px-3 text-xs whitespace-nowrap">
        <span className="text-muted-foreground font-semibold">Console</span>
        <div
          role="radiogroup"
          aria-label="Console filter"
          className="border-border bg-surface-2 flex h-5.5 shrink-0 items-center rounded-md border p-px"
        >
          {FILTERS.map((f) => {
            const checked = f.value === filter;
            return (
              <button
                key={f.value}
                type="button"
                role="radio"
                aria-checked={checked}
                onClick={() => {
                  atBottom.current = true;
                  setFilter(f.value);
                }}
                className={cn(
                  "focus-visible:ring-ring/50 h-full rounded-[5px] px-2 text-[11px] leading-none outline-none focus-visible:ring-2",
                  checked
                    ? "bg-background text-foreground font-medium shadow-xs"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {f.label}
              </button>
            );
          })}
        </div>
        <span className="text-muted-foreground tabular-nums">
          {formatCount(shown.length)} {shown.length === 1 ? "entry" : "entries"}
        </span>
        <span className="flex-1" />
        <Button variant="ghost" size="xs" onClick={clear} disabled={entries.length === 0}>
          <Trash2 />
          Clear
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={close}
          aria-label="Close console"
          title={keys ? `Close console (${formatCombo(keys)})` : "Close console"}
        >
          <X />
        </Button>
      </div>
      {shown.length === 0 ? (
        entries.length === 0 ? (
          <EmptyState
            title="Nothing logged yet"
            description="The SQL Tabler runs (browsing, edits, structure changes and the SQL editor) shows up here."
          />
        ) : (
          <EmptyState
            title={`No ${filterLabel.toLowerCase()} entries`}
            description="Choose All to see every entry."
          />
        )
      ) : (
        <div
          ref={scrollRef}
          role="log"
          aria-label="Console entries"
          aria-live="off"
          className="min-h-0 flex-1 overflow-auto"
          onScroll={(e) => {
            const el = e.currentTarget;
            atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          }}
        >
          {shown.map((entry) => (
            <ConsoleRow key={entry.id} entry={entry} />
          ))}
        </div>
      )}
    </section>
  );
}

const ConsoleRow = memo(function ConsoleRow({ entry }: { entry: ConsoleEntry }) {
  const [expanded, setExpanded] = useState(false);
  const long = entry.sql.length > LONG_CHARS || entry.sql.split("\n").length > LONG_LINES;
  const category = CATEGORY[entry.category];
  const source = [entry.connectionName, entry.database].filter(Boolean).join(" · ");
  const info = stats(entry);
  return (
    <div
      data-testid="console-entry"
      className="group/entry border-border/60 hover:bg-row-hover border-b px-3 py-1.5"
    >
      <div className="text-muted-foreground flex h-5 items-center gap-2 text-[11px]">
        <time dateTime={new Date(entry.at).toISOString()} className="font-mono tabular-nums">
          {clockTime(entry.at)}
        </time>
        <span
          className={cn("rounded px-1.5 text-[10px] leading-4 font-medium", category.className)}
        >
          {category.label}
        </span>
        {entry.label && <span className="font-mono text-[10px]">{entry.label}</span>}
        {source && <span className="min-w-0 truncate">{source}</span>}
        <span className="flex-1" />
        {info && <span className="shrink-0 tabular-nums">{info}</span>}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Copy SQL"
          title="Copy SQL"
          className="size-5 opacity-0 group-hover/entry:opacity-100 focus-visible:opacity-100"
          onClick={() => copySql(entry.sql)}
        >
          <Copy />
        </Button>
      </div>
      <pre
        className={cn(
          "selectable font-mono text-xs break-words whitespace-pre-wrap",
          entry.meta && "text-muted-foreground italic",
          long && !expanded && "line-clamp-6",
        )}
      >
        {entry.sql}
      </pre>
      {long && (
        <button
          type="button"
          className="text-muted-foreground hover:text-foreground text-[11px]"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
      {entry.error && (
        <p className="selectable text-error mt-0.5 font-mono text-[11px] break-words whitespace-pre-wrap">
          {entry.error}
        </p>
      )}
    </div>
  );
});
