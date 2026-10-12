import { ChevronDown, ChevronRight, Copy, Download, KeyRound, Loader2, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { ValueKindIcon } from "@/components/TypeIcon";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { jsonError, reformatJson } from "@/features/datagrid/editors/json";
import type { GridColumn } from "@/features/datagrid/types";
import { ipc, type Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { displayValue, formatBytes, inspectText } from "@/lib/values";
import { useInspectorStore, type InspectedCell, type InspectorMode } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import { base64Head, hexDump } from "./hex";

const HEX_PREVIEW = 4096;

function copy(text: string, what = "Copied") {
  void writeClipboard(text).then(
    () => toast.success(what),
    (e: unknown) => toast.error("Could not copy", { description: (e as Error).message }),
  );
}

/** What "Copy" puts on the clipboard: the raw text (JSON verbatim), or a blob's base64. */
function copyText(v: Value): string {
  return v.t === "bytes" ? v.v.base64 : inspectText(v);
}

/**
 * What the Cell / Row views show: like `copyText`, but valid JSON is re-indented. Only whitespace
 * changes — no `JSON.parse` round trip — so big integers, `1.10`, duplicate keys and escapes read
 * exactly as stored (and as the grid and the JSON editor show them).
 */
function viewText(v: Value): string {
  if (v.t === "json" && jsonError(v.v) === null) return reformatJson(v.v, 2);
  return inspectText(v);
}

function textStats(v: Value): string | null {
  if (v.t !== "text" && v.t !== "enum" && v.t !== "json") return null;
  const chars = Array.from(v.v).length;
  const lines = v.v === "" ? 1 : v.v.split(/\r\n|\r|\n/).length;
  return `${chars.toLocaleString()} char${chars === 1 ? "" : "s"} · ${lines.toLocaleString()} line${lines === 1 ? "" : "s"}`;
}

export function InspectorPanel() {
  const cell = useInspectorStore((s) => s.cell);
  const mode = useInspectorStore((s) => s.mode);
  const setMode = useInspectorStore((s) => s.setMode);
  const toggle = useUiStore((s) => s.toggleInspector);
  return (
    <aside className="border-border bg-surface flex h-full flex-col border-l">
      <div className="border-border text-muted-foreground flex h-9 shrink-0 items-center gap-2 border-b px-3 text-xs font-semibold">
        Inspector
        <span className="flex-1" />
        <Button variant="ghost" size="icon-xs" onClick={toggle} aria-label="Close inspector">
          <X />
        </Button>
      </div>
      {cell ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="border-border shrink-0 border-b px-2 py-1.5">
            <Tabs value={mode} onValueChange={(v) => setMode(v as InspectorMode)}>
              <TabsList className="h-6 w-full p-0.5">
                <TabsTrigger value="cell" className="h-5 text-[11px]">
                  Cell
                </TabsTrigger>
                <TabsTrigger value="row" className="h-5 text-[11px]">
                  Row
                </TabsTrigger>
              </TabsList>
            </Tabs>
          </div>
          {mode === "row" ? <RowView cell={cell} /> : <CellView cell={cell} />}
        </div>
      ) : (
        <EmptyState
          title="No cell selected"
          description="Select a cell in the grid to see its full value."
        />
      )}
    </aside>
  );
}

type FullValue =
  | { id: string; status: "loading" }
  | { id: string; status: "done"; value: Value }
  | { id: string; status: "error"; error: string };

function CellView({ cell }: { cell: InspectedCell }) {
  const { column, value, fetch } = cell;
  const fetchId = fetch
    ? JSON.stringify([fetch.connectionId, fetch.table, fetch.key, fetch.column])
    : null;
  const [full, setFull] = useState<FullValue | null>(null);
  const current = full && full.id === fetchId ? full : null;
  const shown = current?.status === "done" ? current.value : value;
  const stats = textStats(shown);

  const loadFull = async () => {
    if (!fetch || !fetchId) return;
    setFull({ id: fetchId, status: "loading" });
    try {
      const v = await ipc.browse.cell(fetch.connectionId, fetch.table, fetch.key, fetch.column);
      // Never present a preview as the full value (Copy would silently copy only part of it).
      if (v.t === "bytes" && v.v.truncated) {
        setFull({
          id: fetchId,
          status: "error",
          error: "Only a preview of this value could be loaded. Export it to get all of it.",
        });
        return;
      }
      setFull({ id: fetchId, status: "done", value: v });
    } catch (e) {
      setFull({ id: fetchId, status: "error", error: (e as Error).message });
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex items-center gap-2 border-b px-3 py-2 font-mono text-xs">
        <ValueKindIcon kind={column.kind} />
        <span className="truncate font-semibold">{column.name}</span>
        <span className="flex-1" />
        <span className="text-muted-foreground shrink-0 text-[10px]">row {cell.rowIndex + 1}</span>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Copy value"
          title={shown.t === "bytes" ? "Copy as base64" : "Copy value"}
          onClick={() => copy(copyText(shown))}
        >
          <Copy />
        </Button>
      </div>
      <div className="border-border text-muted-foreground flex min-h-7 flex-wrap items-center gap-x-2 gap-y-1 border-b px-3 py-1 text-[11px]">
        <span className="font-mono">{column.dataType}</span>
        {stats && <span>· {stats}</span>}
        {shown.t === "bytes" && (
          <span>
            · {formatBytes(shown.v.len)}
            {shown.v.truncated && " (preview)"}
          </span>
        )}
        {value.t === "bytes" && value.v.truncated && fetch && current?.status !== "done" && (
          <Button
            variant="outline"
            size="xs"
            className="ml-auto h-5 text-[11px]"
            disabled={current?.status === "loading"}
            onClick={() => void loadFull()}
          >
            {current?.status === "loading" ? <Loader2 className="animate-spin" /> : <Download />}
            Load full value
          </Button>
        )}
      </div>
      {current?.status === "error" && (
        <div className="text-error border-border border-b px-3 py-1.5 text-[11px]">
          {current.error}
        </div>
      )}
      {shown.t === "bytes" ? (
        <BytesView value={shown} />
      ) : (
        <pre
          className={cn(
            "selectable min-h-0 flex-1 overflow-auto p-3 font-mono text-xs leading-5 break-words whitespace-pre-wrap",
            shown.t === "null" && "text-null italic",
          )}
        >
          {viewText(shown)}
        </pre>
      )}
    </div>
  );
}

function BytesView({ value }: { value: Extract<Value, { t: "bytes" }> }) {
  const dump = useMemo(() => {
    try {
      return hexDump(base64Head(value.v.base64, HEX_PREVIEW));
    } catch {
      return null;
    }
  }, [value.v.base64]);
  const shownBytes = Math.min(HEX_PREVIEW, value.v.len);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <pre className="selectable min-h-0 flex-1 overflow-auto p-3 font-mono text-[11px] leading-4 whitespace-pre">
        {dump ?? `base64: ${value.v.base64.slice(0, 2000)}`}
      </pre>
      {value.v.len > shownBytes && (
        <div className="border-border text-muted-foreground border-t px-3 py-1 text-[11px]">
          Showing the first {formatBytes(shownBytes)} of {formatBytes(value.v.len)}
        </div>
      )}
    </div>
  );
}

function RowView({ cell }: { cell: InspectedCell }) {
  const row = cell.row ?? {
    columns: [cell.column],
    cells: [cell.value],
    focused: 0,
    hidden: [],
  };
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const focusedRef = useRef<HTMLDivElement>(null);
  const hidden = useMemo(() => new Set(row.hidden ?? []), [row.hidden]);
  const q = query.trim().toLowerCase();
  const lines = row.columns
    .map((col, i) => ({ col, i, value: row.cells[i] ?? ({ t: "null" } as Value) }))
    .filter(({ col }) => q === "" || col.name.toLowerCase().includes(q));

  useEffect(() => {
    focusedRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [row.focused, cell.rowIndex]);

  const toggle = (id: string) =>
    setExpanded((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex shrink-0 items-center gap-2 border-b px-2 py-1.5">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter fields"
          aria-label="Filter fields"
          spellCheck={false}
          className="h-6 rounded-md px-2 text-xs md:text-xs"
        />
        <span className="text-muted-foreground shrink-0 text-[10px]">row {cell.rowIndex + 1}</span>
      </div>
      <div className="min-h-0 flex-1 overflow-auto py-0.5" role="list" aria-label="Row fields">
        {lines.length === 0 && (
          <div className="text-muted-foreground px-3 py-2 text-xs">No matching fields</div>
        )}
        {lines.map(({ col, i, value }) => (
          <RowLine
            key={col.id}
            col={col}
            value={value}
            focused={i === row.focused}
            hidden={hidden.has(col.id)}
            open={expanded.has(col.id)}
            onToggle={() => toggle(col.id)}
            lineRef={i === row.focused ? focusedRef : undefined}
          />
        ))}
      </div>
    </div>
  );
}

function RowLine({
  col,
  value,
  focused,
  hidden,
  open,
  onToggle,
  lineRef,
}: {
  col: GridColumn;
  value: Value;
  focused: boolean;
  hidden: boolean;
  open: boolean;
  onToggle: () => void;
  lineRef?: React.Ref<HTMLDivElement>;
}) {
  const d = displayValue(value);
  const isNull = value.t === "null";
  return (
    <div ref={lineRef} role="listitem" className={cn(focused && "bg-brand-soft")}>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        title={`${col.name} · ${col.dataType}`}
        className="hover:bg-row-hover flex h-7 w-full items-center gap-1.5 px-2 text-left text-xs"
      >
        {open ? (
          <ChevronDown className="text-muted-foreground size-3 shrink-0" />
        ) : (
          <ChevronRight className="text-muted-foreground size-3 shrink-0" />
        )}
        {col.isPk ? (
          <KeyRound className="text-warning size-3 shrink-0" />
        ) : (
          <ValueKindIcon kind={col.kind} />
        )}
        <span className="text-muted-foreground w-[38%] shrink-0 truncate font-mono text-[11px]">
          {col.name}
        </span>
        <span
          className={cn(
            "min-w-0 flex-1 truncate",
            isNull || value.t === "default" ? "text-null italic" : "font-mono text-[11.5px]",
          )}
        >
          {d.text}
        </span>
        {hidden && (
          <span className="text-muted-foreground border-border shrink-0 rounded border px-1 text-[9px] tracking-wide uppercase">
            hidden
          </span>
        )}
      </button>
      {open && (
        <div className="group/val relative px-2 pb-1.5 pl-[26px]">
          <pre
            className={cn(
              "selectable bg-surface-2 border-border max-h-64 overflow-auto rounded border p-2 pr-7 font-mono text-[11px] leading-4 break-words whitespace-pre-wrap",
              isNull && "text-null italic",
            )}
          >
            {value.t === "bytes"
              ? `${formatBytes(value.v.len)} · base64\n${value.v.base64.slice(0, 2000)}`
              : viewText(value)}
          </pre>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={`Copy ${col.name}`}
            className="absolute top-1 right-3"
            onClick={() => copy(copyText(value))}
          >
            <Copy />
          </Button>
        </div>
      )}
    </div>
  );
}
