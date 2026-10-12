import { ChevronDown, ChevronRight, Download, ExternalLink, Loader2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { base64Head, hexDump } from "@/features/inspector/hex";
import { parseFieldText } from "@/features/inspector/fields";
import { actions, type ActionId } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { cellDisplay, editText, formatBytes } from "@/lib/values";
import { jsonError, reformatJson } from "../editors/json";
import type { GridColumn } from "../types";
import { sniffBase64Image } from "./bytes";
import { CodeView } from "./CodeView";
import { nodeSummary, parseJsonTree, type JsonNode } from "./jsonTree";

/** The rows an FK cell points at (Quick Look's foreign-key viewer). */
export interface ReferencedRows {
  /** `schema.table` (or the table name). */
  table: string;
  columns: Array<{ name: string; dataType: string }>;
  rows: Value[][];
}

export interface QuickLookProps {
  column: GridColumn;
  value: Value;
  /** 1-based row number, shown in the title. */
  rowNumber?: number;
  /** Stages an edited value (editable tables only); absent = read-only. */
  onSave?: (value: Value) => void;
  onClose: () => void;
  /** Loads the referenced row of an FK cell (`=` filter on the referenced column). */
  loadReferenced?: () => Promise<ReferencedRows>;
  /** Opens the referenced table filtered to the value. */
  onOpenReferenced?: () => void;
  /** Loads a blob whose preview was cut off in full. */
  loadFull?: () => Promise<Value>;
}

/** Bytes shown in the hex view. */
const HEX_LIMIT = 64 * 1024;

type View = "value" | "referenced";
type JsonView = "json" | "tree";
type BytesView = "hex" | "base64";

function Seg<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="border-border bg-surface-2 dark:bg-background/40 flex h-6 items-center rounded-md border p-px"
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          onClick={() => onChange(o.value)}
          className={cn(
            "h-full rounded-[5px] px-2 text-[11px] leading-none",
            o.value === value
              ? "bg-surface text-foreground dark:bg-accent font-medium shadow-xs"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function Toggle({
  pressed,
  onChange,
  children,
  title,
}: {
  pressed: boolean;
  onChange: (pressed: boolean) => void;
  children: ReactNode;
  title?: string;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      size="xs"
      aria-pressed={pressed}
      title={title}
      onClick={() => onChange(!pressed)}
      className={cn(
        "text-muted-foreground hover:text-foreground h-6 text-[11px]",
        pressed && "bg-muted text-foreground",
      )}
    >
      {children}
    </Button>
  );
}

/** One collapsible node of the JSON tree (objects and arrays open two levels deep). */
function TreeNode({ name, node, depth }: { name: string | null; node: JsonNode; depth: number }) {
  const [open, setOpen] = useState(depth < 2);
  const container = node.type === "object" || node.type === "array";
  const scalar = cn(
    node.type === "string" && "text-[color:var(--syn-string)]",
    node.type === "number" && "text-[color:var(--syn-number)]",
    (node.type === "boolean" || node.type === "null") && "text-[color:var(--syn-keyword)]",
  );
  const label = name !== null && <span className="text-foreground font-medium">{name}: </span>;
  if (!container)
    return (
      <div role="treeitem" aria-selected={false} className="pl-4 leading-5 whitespace-pre-wrap">
        {label}
        <span className={scalar}>{nodeSummary(node)}</span>
      </div>
    );
  const children =
    node.type === "object"
      ? node.entries.map((e, i) => ({ key: `${i}`, name: e.key, node: e.value }))
      : node.items.map((item, i) => ({ key: `${i}`, name: String(i), node: item }));
  return (
    <div role="treeitem" aria-expanded={open} aria-selected={false}>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="hover:bg-accent flex items-center rounded leading-5"
      >
        {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        {label}
        <span className="text-muted-foreground">{nodeSummary(node)}</span>
      </button>
      {open && (
        <div role="group" className="border-border/60 ml-[7px] border-l pl-1">
          {children.map((c) => (
            <TreeNode key={c.key} name={c.name} node={c.node} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

function JsonTree({ text }: { text: string }) {
  const tree = useMemo(() => {
    try {
      return { node: parseJsonTree(text), error: null };
    } catch (e) {
      return { node: null, error: (e as Error).message };
    }
  }, [text]);
  if (!tree.node) return <div className="text-error p-3 text-xs">Not valid JSON: {tree.error}</div>;
  return (
    <div
      role="tree"
      aria-label="JSON tree"
      className="border-border bg-surface selectable min-h-0 flex-1 overflow-auto rounded-md border p-2 font-mono text-[12px]"
    >
      <TreeNode name={null} node={tree.node} depth={0} />
    </div>
  );
}

function BytesBody({ value, view }: { value: Extract<Value, { t: "bytes" }>; view: BytesView }) {
  const { base64, len, truncated } = value.v;
  const image = !truncated ? sniffBase64Image(base64) : null;
  const dump = useMemo(() => {
    if (view !== "hex") return "";
    try {
      return hexDump(base64Head(base64, HEX_LIMIT));
    } catch {
      return "";
    }
  }, [base64, view]);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      {image && (
        <div className="border-border bg-surface-2 flex max-h-64 shrink-0 items-center justify-center overflow-hidden rounded-md border p-2">
          <img
            src={`data:${image};base64,${base64}`}
            alt={`${image.slice(6).toUpperCase()} image preview`}
            className="max-h-60 max-w-full object-contain"
          />
        </div>
      )}
      {view === "hex" ? (
        <pre
          data-testid="quick-look-hex"
          className="border-border bg-surface selectable min-h-0 flex-1 overflow-auto rounded-md border p-2 font-mono text-[11px] leading-4 whitespace-pre"
        >
          {dump}
          {len > HEX_LIMIT && `\n… first ${formatBytes(HEX_LIMIT)} of ${formatBytes(len)}`}
        </pre>
      ) : (
        <CodeView text={base64} readOnly wrap label="Base64 value" />
      )}
    </div>
  );
}

type Load<T> =
  { status: "loading" } | { status: "done"; data: T } | { status: "error"; error: string };

function ReferencedBody({
  load,
  onOpen,
}: {
  load: () => Promise<ReferencedRows>;
  onOpen?: () => void;
}) {
  const [state, setState] = useState<Load<ReferencedRows>>({ status: "loading" });
  useEffect(() => {
    let alive = true;
    load().then(
      (data) => alive && setState({ status: "done", data }),
      (e: unknown) => alive && setState({ status: "error", error: (e as Error).message }),
    );
    return () => {
      alive = false;
    };
    // Loaded once per open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  if (state.status === "loading")
    return (
      <div className="text-muted-foreground flex flex-1 items-center justify-center gap-2 text-xs">
        <Loader2 className="size-3.5 animate-spin" /> Loading the referenced row…
      </div>
    );
  if (state.status === "error")
    return <div className="text-error p-3 text-xs">Could not load it: {state.error}</div>;
  const { table, columns, rows } = state.data;
  const first = rows[0];
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="text-muted-foreground flex items-center gap-2 text-[11px]">
        <span className="text-foreground font-mono">{table}</span>
        <span>
          {rows.length === 0
            ? "· no matching row"
            : rows.length > 1
              ? `· ${rows.length} matching rows (first shown)`
              : "· 1 row"}
        </span>
        <span className="flex-1" />
        {onOpen && (
          <Button variant="outline" size="xs" onClick={onOpen}>
            <ExternalLink /> Open {table}
          </Button>
        )}
      </div>
      {first && (
        <div
          role="list"
          aria-label="Referenced row"
          className="border-border bg-surface min-h-0 flex-1 divide-y overflow-auto rounded-md border"
        >
          {columns.map((c, i) => {
            const d = cellDisplay(first[i] ?? { t: "null" });
            return (
              <div
                key={`${i}\u0000${c.name}`}
                role="listitem"
                className="border-border/60 grid grid-cols-[minmax(8rem,30%)_1fr] gap-3 px-2.5 py-1 text-xs"
              >
                <span className="text-muted-foreground truncate" title={c.dataType}>
                  {c.name}
                </span>
                <span
                  className={cn(
                    "selectable font-mono break-all",
                    d.placeholder && "text-null italic",
                  )}
                >
                  {d.text}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/**
 * Keeps the app's shortcuts out of the Quick Look editor. Its CodeMirror opts in to the SQL
 * editor's keyboard scope (`data-shortcut-scope`), so ⌘↩ (Run current), ⌘S, ⌘W… would reach the
 * tab behind the dialog: while it is open its own handlers sit on top — ⌘↩ saves, ⌘W closes, the
 * rest do nothing.
 */
function useShieldShortcuts(save: () => void, close: () => void) {
  const latest = useRef({ save, close });
  latest.current = { save, close };
  useEffect(() => {
    const ids = new Set<ActionId>();
    for (const d of KEYMAP)
      if (d.scope === "editor" || (d.scope === "global" && (d.allowInEditor || d.allowInDialog)))
        ids.add(d.id);
    const off = [...ids].map((id) =>
      actions.register(id, () => {
        if (id === "query.run-current") latest.current.save();
        else if (id === "tab.close") latest.current.close();
      }),
    );
    return () => off.forEach((f) => f());
  }, []);
}

/**
 * TablePlus's Quick Look Editor (⌘↩ on a cell, middle-click, or the cell menu): the value in a
 * large editor. Text gets a monospace editor with a wrap toggle; JSON a Json | Tree switch with
 * Format / Raw; blobs a hex or base64 view and an image preview (PNG, JPEG, GIF, WebP); a
 * foreign-key cell also shows the row it references. Save (⌘↩) stages the edit on editable
 * tables; ⎋ closes.
 */
export function QuickLook({
  column,
  value,
  rowNumber,
  onSave,
  onClose,
  loadReferenced,
  onOpenReferenced,
  loadFull,
}: QuickLookProps) {
  const [shown, setShown] = useState(value);
  const [fullState, setFullState] = useState<Load<null> | null>(null);
  const bytes = shown.t === "bytes" || column.kind === "bytes";
  const json = !bytes && (column.kind === "json" || shown.t === "json");
  const stored = editText(shown);
  const storedValid = json && jsonError(stored) === null;
  const [pretty, setPretty] = useState(storedValid);
  const base = json && pretty && storedValid ? reformatJson(stored, 2) : stored;
  const [text, setText] = useState(base);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>(loadReferenced ? "referenced" : "value");
  const [jsonView, setJsonView] = useState<JsonView>("json");
  const [bytesView, setBytesView] = useState<BytesView>("hex");
  const [wrap, setWrap] = useState(json ? false : true);
  const editable = !!onSave && !bytes;
  const dirty = text !== base;

  const save = () => {
    if (!editable || !dirty) return onClose();
    const result = parseFieldText(column, text, shown);
    if (!result.ok) return setError(result.error);
    onSave?.(result.value);
  };
  useShieldShortcuts(save, onClose);

  const togglePretty = (next: boolean) => {
    if (!dirty) {
      setPretty(next);
      setText(next && storedValid ? reformatJson(stored, 2) : stored);
      return;
    }
    if (jsonError(text) !== null) return setError("Fix the JSON first");
    setPretty(next);
    setText(reformatJson(text, next ? 2 : null));
  };

  const loadWhole = async () => {
    if (!loadFull) return;
    setFullState({ status: "loading" });
    try {
      const v = await loadFull();
      if (v.t === "bytes" && v.v.truncated) throw new Error("Only a preview could be loaded.");
      setShown(v);
      setFullState({ status: "done", data: null });
    } catch (e) {
      setFullState({ status: "error", error: (e as Error).message });
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // Outside CodeMirror (buttons): ⌘↩ saves. Inside it the shield above does.
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.defaultPrevented) {
      e.preventDefault();
      save();
    }
    e.stopPropagation();
  };

  const truncated = shown.t === "bytes" && shown.v.truncated;
  const isNull = shown.t === "null";
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        data-scope="grid-edit"
        // Its editor keeps the SQL editor's keyboard scope: the shield above takes those keys.
        data-shortcut-scope="editor"
        aria-label={`Quick Look: ${column.name}`}
        className="flex h-[min(42rem,85vh)] flex-col gap-2.5 sm:max-w-3xl"
        onCloseAutoFocus={(e) => e.preventDefault()}
        onPointerDown={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        onContextMenu={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <DialogHeader className="gap-1">
          <DialogTitle className="flex items-center gap-2 text-sm">
            <span className="font-mono">{column.name}</span>
            <span className="text-muted-foreground font-mono text-[11px] font-normal">
              {column.dataType}
              {rowNumber !== undefined && ` · row ${rowNumber}`}
            </span>
          </DialogTitle>
          <DialogDescription className="sr-only">
            The cell value in a larger editor. Save with Command-Return, close with Escape.
          </DialogDescription>
        </DialogHeader>
        <div className="flex min-h-6 flex-wrap items-center gap-2">
          {loadReferenced && (
            <Seg
              label="Quick Look view"
              value={view}
              options={[
                { value: "referenced", label: "Referenced row" },
                { value: "value", label: "Value" },
              ]}
              onChange={setView}
            />
          )}
          {view === "value" && json && (
            <Seg
              label="JSON view"
              value={jsonView}
              options={[
                { value: "json", label: "Json" },
                { value: "tree", label: "Tree" },
              ]}
              onChange={setJsonView}
            />
          )}
          {view === "value" && json && jsonView === "json" && (
            <Seg
              label="JSON text"
              value={pretty ? "format" : "raw"}
              options={[
                { value: "format", label: "Format" },
                { value: "raw", label: "Raw" },
              ]}
              onChange={(v) => togglePretty(v === "format")}
            />
          )}
          {view === "value" && bytes && shown.t === "bytes" && (
            <Seg
              label="Binary view"
              value={bytesView}
              options={[
                { value: "hex", label: "Hex" },
                { value: "base64", label: "Base64" },
              ]}
              onChange={setBytesView}
            />
          )}
          <span className="flex-1" />
          {view === "value" && shown.t === "bytes" && (
            <span className="text-muted-foreground text-[11px]">
              {formatBytes(shown.v.len)}
              {truncated && " (preview)"}
            </span>
          )}
          {view === "value" && truncated && loadFull && (
            <Button
              variant="outline"
              size="xs"
              disabled={fullState?.status === "loading"}
              onClick={() => void loadWhole()}
            >
              {fullState?.status === "loading" ? (
                <Loader2 className="animate-spin" />
              ) : (
                <Download />
              )}
              Load full value
            </Button>
          )}
          {view === "value" && !bytes && !(json && jsonView === "tree") && (
            <Toggle pressed={wrap} onChange={setWrap} title="Wrap long lines">
              Wrap
            </Toggle>
          )}
        </div>
        {view === "referenced" && loadReferenced ? (
          <ReferencedBody load={loadReferenced} onOpen={onOpenReferenced} />
        ) : bytes && shown.t === "bytes" ? (
          <BytesBody value={shown} view={bytesView} />
        ) : bytes ? (
          <div className="text-null flex flex-1 items-center justify-center text-xs italic">
            {cellDisplay(shown).text}
          </div>
        ) : json && jsonView === "tree" ? (
          <JsonTree text={text} />
        ) : (
          <CodeView
            text={text}
            onChange={(t) => {
              setText(t);
              setError(null);
            }}
            readOnly={!editable}
            wrap={wrap}
            label={`Value of ${column.name}`}
            autoFocus
          />
        )}
        {fullState?.status === "error" && (
          <p role="alert" className="text-error text-[11px]">
            {fullState.error}
          </p>
        )}
        <DialogFooter className="items-center sm:justify-between">
          <p
            role={error ? "alert" : undefined}
            className={cn(
              "min-w-0 truncate text-[11px]",
              error ? "text-error" : "text-muted-foreground",
            )}
          >
            {error ??
              (isNull && view === "value" && !bytes
                ? "NULL"
                : dirty
                  ? "Edited — not staged yet"
                  : editable
                    ? ""
                    : "Read-only")}
          </p>
          <div className="flex gap-1.5">
            <Button variant="ghost" size="sm" onClick={onClose}>
              {editable ? "Cancel" : "Close"}
            </Button>
            {editable && (
              <Button size="sm" onClick={save} disabled={!dirty}>
                Save
                <kbd className="ml-1 text-[10px] opacity-70">{formatCombo("Mod+Enter")}</kbd>
              </Button>
            )}
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
