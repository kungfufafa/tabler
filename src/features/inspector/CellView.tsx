import { Copy, Download, Loader2 } from "lucide-react";
import { useMemo, useState } from "react";
import { ValueKindIcon } from "@/components/TypeIcon";
import { Button } from "@/components/ui/button";
import { jsonError } from "@/features/datagrid/editors/json";
import { Seg, Toggle } from "@/features/datagrid/quicklook/controls";
import { readSortKeys, writeSortKeys } from "@/features/datagrid/quicklook/jsonPrefs";
import { sortJsonKeys } from "@/features/datagrid/quicklook/jsonTree";
import { ipc, type Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { decodeBytesText, formatBytes } from "@/lib/values";
import type { InspectedCell } from "@/stores/inspector";
import { copy } from "./copy";
import { copyText, textStats, viewText } from "./fields";
import { base64Head, hexDump } from "./hex";

const HEX_PREVIEW = 4096;

type FullValue =
  | { id: string; status: "loading" }
  | { id: string; status: "done"; value: Value }
  | { id: string; status: "error"; error: string };

/**
 * The Cell view: one cell's full value (re-indented JSON, optionally with its keys sorted; a hex
 * dump or the text of a blob that is text; full blob load).
 */
export function CellView({ cell }: { cell: InspectedCell }) {
  const { column, value, fetch } = cell;
  const fetchId = fetch
    ? JSON.stringify([fetch.connectionId, fetch.table, fetch.key, fetch.column])
    : null;
  const [full, setFull] = useState<FullValue | null>(null);
  const current = full && full.id === fetchId ? full : null;
  const shown = current?.status === "done" ? current.value : value;
  const stats = textStats(shown);
  // "Sort keys" (display only, remembered): the JSON's object entries in key order.
  const [sortKeys, setSortKeys] = useState(() => readSortKeys("cell"));
  const sortable = shown.t === "json" && jsonError(shown.v) === null;

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
      <div className="border-border flex h-8 shrink-0 items-center gap-2 border-b px-3 font-mono text-xs">
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
      <div className="border-border text-muted-foreground flex min-h-7 shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b px-3 py-1 text-[11px]">
        <span className="font-mono">{column.dataType}</span>
        {stats && <span>· {stats}</span>}
        {sortable && (
          <Toggle
            pressed={sortKeys}
            onChange={(next) => {
              setSortKeys(next);
              writeSortKeys("cell", next);
            }}
            title="List the keys of every object in order (display only)"
          >
            Sort keys
          </Toggle>
        )}
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
        <div className="text-error border-border shrink-0 border-b px-3 py-1.5 text-[11px]">
          {current.error}
        </div>
      )}
      {shown.t === "bytes" ? (
        <BytesView value={shown} />
      ) : (
        <pre
          className={cn(
            "selectable bg-surface min-h-0 flex-1 overflow-auto p-3 font-mono text-xs leading-5 break-words whitespace-pre-wrap",
            (shown.t === "null" || shown.t === "default") && "text-null italic",
          )}
        >
          {sortable && sortKeys ? sortJsonKeys(viewText(shown)) : viewText(shown)}
        </pre>
      )}
    </div>
  );
}

function BytesView({ value }: { value: Extract<Value, { t: "bytes" }> }) {
  const { base64, len, truncated } = value.v;
  const dump = useMemo(() => {
    try {
      return hexDump(base64Head(base64, HEX_PREVIEW));
    } catch {
      return null;
    }
  }, [base64]);
  // A blob that is text can be read as text (the default); anything else only as hex.
  const text = useMemo(() => decodeBytesText(base64, HEX_PREVIEW, truncated), [base64, truncated]);
  const [pick, setPick] = useState<"hex" | "text" | null>(null);
  const view =
    pick === "text" && text === null ? "hex" : (pick ?? (text !== null ? "text" : "hex"));
  const shownBytes = Math.min(HEX_PREVIEW, len);
  return (
    <div className="bg-surface flex min-h-0 flex-1 flex-col">
      {text !== null && (
        <div className="border-border flex shrink-0 items-center border-b px-3 py-1">
          <Seg
            label="Binary view"
            value={view}
            options={[
              { value: "hex", label: "Hex" },
              { value: "text", label: "Text" },
            ]}
            onChange={setPick}
          />
        </div>
      )}
      <pre
        data-testid={view === "text" ? "cell-view-text" : "cell-view-hex"}
        className={cn(
          "selectable min-h-0 flex-1 overflow-auto p-3 font-mono",
          view === "text"
            ? "text-xs leading-5 break-words whitespace-pre-wrap"
            : "text-[11px] leading-4 whitespace-pre",
        )}
      >
        {view === "text" ? text : (dump ?? `base64: ${base64.slice(0, 2000)}`)}
      </pre>
      {len > shownBytes && (
        <div className="border-border text-muted-foreground border-t px-3 py-1 text-[11px]">
          Showing the first {formatBytes(shownBytes)} of {formatBytes(len)}
        </div>
      )}
    </div>
  );
}
