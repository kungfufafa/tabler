import {
  ChevronDown,
  ChevronRight,
  Download,
  ExternalLink,
  Loader2,
  PictureInPicture2,
  X,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
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
import { cellDisplay, decodeBytesText, editText, formatBytes } from "@/lib/values";
import { bytesHex } from "../cellText";
import { jsonError, reformatJson } from "../editors/json";
import { MapView } from "../geometry/GeometryView";
import { parseWkt } from "../geometry/wkt";
import type { GridColumn } from "../types";
import { sniffBase64Image } from "./bytes";
import { CodeView } from "./CodeView";
import { Seg, Toggle } from "./controls";
import { readSortKeys, writeSortKeys } from "./jsonPrefs";
import { nodeSummary, parseJsonTree, sortJsonKeys, type JsonNode } from "./jsonTree";
import { detectLanguage } from "./languages";

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
  /**
   * A floating panel instead of a dialog (TablePlus's detached Quick Look): it stays open while
   * the grid is used, and the grid hands it the newly focused cell.
   */
  detached?: boolean;
  /** Shows the Detach button (the dialog becomes the floating panel). */
  onDetach?: () => void;
  /**
   * Told whenever the editor holds an edit that was not saved (and `false` when it goes away), so
   * the grid does not hand a floating panel another cell while its edit would be lost.
   */
  onDirtyChange?: (dirty: boolean) => void;
}

/** Bytes shown in the hex view. */
const HEX_LIMIT = 64 * 1024;
/** Bytes decoded for the Text view. */
const TEXT_LIMIT = 64 * 1024;

type View = "value" | "referenced";
type JsonView = "json" | "tree";
type BytesView = "hex" | "base64" | "text";
type GeoView = "map" | "text";

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

function JsonTree({ text, sort }: { text: string; sort: boolean }) {
  const tree = useMemo(() => {
    try {
      // Sorted for display only: the text itself (and the staged value) stays as it is.
      return { node: parseJsonTree(sort ? sortJsonKeys(text) : text), error: null };
    } catch (e) {
      return { node: null, error: (e as Error).message };
    }
  }, [text, sort]);
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

/** An image blob (PNG, JPEG, GIF, WebP) shown as itself. */
function ImagePreview({ base64 }: { base64: string }) {
  const image = sniffBase64Image(base64);
  if (!image) return null;
  return (
    <div className="border-border bg-surface-2 flex max-h-64 shrink-0 items-center justify-center overflow-hidden rounded-md border p-2">
      <img
        src={`data:${image};base64,${base64}`}
        alt={`${image.slice(6).toUpperCase()} image preview`}
        className="max-h-60 max-w-full object-contain"
      />
    </div>
  );
}

function BytesBody({
  value,
  view,
  text,
  column,
}: {
  value: Extract<Value, { t: "bytes" }>;
  view: BytesView;
  /** The bytes decoded as text, when they are text (the Text view). */
  text: string | null;
  column: string;
}) {
  const { base64, len, truncated } = value.v;
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
      {!truncated && <ImagePreview base64={base64} />}
      {view === "hex" ? (
        <pre
          data-testid="quick-look-hex"
          className="border-border bg-surface selectable min-h-0 flex-1 overflow-auto rounded-md border p-2 font-mono text-[11px] leading-4 whitespace-pre"
        >
          {dump}
          {len > HEX_LIMIT && `\n… first ${formatBytes(HEX_LIMIT)} of ${formatBytes(len)}`}
        </pre>
      ) : view === "text" && text !== null ? (
        <>
          <CodeView
            text={text}
            readOnly
            wrap
            label={`Text of ${column}`}
            language={detectLanguage("text", "bytea", text)}
          />
          {(truncated || len > TEXT_LIMIT) && (
            <p className="text-muted-foreground text-[11px]">
              {truncated
                ? "Only a preview is loaded: the text stops where it does."
                : `The first ${formatBytes(TEXT_LIMIT)} of ${formatBytes(len)} as text.`}
            </p>
          )}
        </>
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
 * tab behind the dialog: while it is open (the floating panel: while it has the focus) its own
 * handlers sit on top — ⌘↩ saves, ⌘W closes, the rest do nothing.
 */
function useShieldShortcuts(save: () => void, close: () => void, enabled = true) {
  const latest = useRef({ save, close });
  latest.current = { save, close };
  useEffect(() => {
    if (!enabled) return;
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
  }, [enabled]);
}

/** Where the floating Quick Look sits (kept while it follows the focused cell, and between opens). */
let floatBox: { x: number; y: number; w: number; h: number } | null = null;

function initialFloatBox() {
  const w = 460;
  const h = 400;
  const vw = typeof window === "undefined" ? 1200 : window.innerWidth;
  return { x: Math.max(8, vw - w - 24), y: 84, w, h };
}

/** The detached Quick Look: a movable (drag its title bar), resizable, non-modal panel. */
function FloatingPanel({
  label,
  title,
  onClose,
  onKeyDown,
  onFocusChange,
  children,
}: {
  label: string;
  title: ReactNode;
  onClose: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLDivElement>) => void;
  onFocusChange: (focused: boolean) => void;
  children: ReactNode;
}) {
  const [box, setBox] = useState(() => (floatBox ??= initialFloatBox()));
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; bx: number; by: number } | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (!floatBox) return;
      floatBox = { ...floatBox, w: el.offsetWidth || floatBox.w, h: el.offsetHeight || floatBox.h };
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || (e.target as Element).closest("button")) return;
    drag.current = { x: e.clientX, y: e.clientY, bx: box.x, by: box.y };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    const x = Math.max(0, Math.min(window.innerWidth - 80, d.bx + e.clientX - d.x));
    const y = Math.max(0, Math.min(window.innerHeight - 40, d.by + e.clientY - d.y));
    setBox((b) => ({ ...b, x, y }));
    floatBox = { ...(floatBox ?? box), x, y };
  };
  const endDrag = () => {
    drag.current = null;
  };
  return createPortal(
    <div
      ref={ref}
      role="dialog"
      aria-modal={false}
      aria-label={label}
      data-scope="grid-edit"
      data-shortcut-scope="editor"
      data-quick-look="detached"
      className="border-border bg-popover text-popover-foreground fixed z-40 flex min-h-56 min-w-80 resize flex-col gap-2 overflow-hidden rounded-lg border p-2.5 shadow-xl"
      style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === "Escape" && !e.defaultPrevented) {
          e.preventDefault();
          onClose();
          return;
        }
        onKeyDown(e);
      }}
      onFocus={() => onFocusChange(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) onFocusChange(false);
      }}
    >
      <div
        data-testid="quick-look-handle"
        className="-mx-2.5 -mt-2.5 flex cursor-move items-center gap-2 border-b px-2.5 py-1.5 text-sm select-none"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
      >
        <div className="min-w-0 flex-1 truncate">{title}</div>
        <Button variant="ghost" size="icon-xs" aria-label="Close Quick Look" onClick={onClose}>
          <X />
        </Button>
      </div>
      {children}
    </div>,
    document.body,
  );
}

/**
 * TablePlus's Quick Look Editor (⌘↩ on a cell, middle-click, or the cell menu): the value in a
 * large editor. Text gets a monospace editor (JSON and XML coloured) with a wrap toggle; JSON a
 * Json | Tree switch with Format / Raw; spatial values a Map; blobs a hex view (editable as hex)
 * or base64 and an image preview (PNG, JPEG, GIF, WebP); a foreign-key cell also shows the row
 * it references. Show raw shows the stored text as it is. Save (⌘↩) stages the edit on editable
 * tables; ⎋ closes. Detach turns it into a floating panel that follows the focused cell.
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
  detached = false,
  onDetach,
  onDirtyChange,
}: QuickLookProps) {
  const [shown, setShown] = useState(value);
  const [fullState, setFullState] = useState<Load<null> | null>(null);
  const bytes = shown.t === "bytes" || column.kind === "bytes";
  const json = !bytes && (column.kind === "json" || shown.t === "json");
  const wholeBlob = shown.t !== "bytes" || !shown.v.truncated;
  const editable = !!onSave && (!bytes || (column.kind === "bytes" && wholeBlob));
  const isJsonText = (t: string) => json && jsonError(t) === null;
  /**
   * The editor's text for `v`: blobs as grouped hex, valid JSON with its keys in order while
   * `sortOn` ("Sort keys") and indented while `prettyOn`.
   */
  const textOf = (v: Value, prettyOn: boolean, sortOn: boolean): string => {
    if (v.t === "bytes") return v.v.truncated ? "" : bytesHex(v.v.base64, true);
    let t = editText(v);
    if (!isJsonText(t)) return t;
    if (sortOn) t = sortJsonKeys(t);
    return prettyOn ? reformatJson(t, 2) : t;
  };
  const stored = shown.t === "bytes" ? textOf(shown, false, false) : editText(shown);
  const storedValid = isJsonText(stored);
  const [pretty, setPretty] = useState(storedValid);
  // "Sort keys" is remembered per viewer; it only changes what is shown (see `toggleSort`).
  const [sorted, setSorted] = useState(() => storedValid && readSortKeys("quicklook"));
  const base = textOf(shown, pretty, sorted);
  const [text, setText] = useState(base);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>(loadReferenced ? "referenced" : "value");
  const [jsonView, setJsonView] = useState<JsonView>("json");
  const [bytesPick, setBytesPick] = useState<BytesView | null>(null);
  const [geoView, setGeoView] = useState<GeoView>("map");
  const [raw, setRaw] = useState(false);
  const [wrap, setWrap] = useState(json ? false : true);
  const [focused, setFocused] = useState(false);
  const dirty = text !== base;
  useEffect(() => {
    onDirtyChange?.(dirty);
    return () => onDirtyChange?.(false);
  }, [dirty, onDirtyChange]);

  // A new value for the same cell (staged here or in the grid): shown unless being edited.
  const [seen, setSeen] = useState(value);
  if (seen !== value) {
    setSeen(value);
    if (!dirty) {
      setShown(value);
      setText(textOf(value, pretty, sorted));
      setError(null);
    }
  }

  // A blob that is text can be read as text (Text view, the default); otherwise hex.
  const bytesText = useMemo(
    () =>
      shown.t === "bytes" ? decodeBytesText(shown.v.base64, TEXT_LIMIT, shown.v.truncated) : null,
    [shown],
  );
  const bytesView: BytesView =
    bytesPick === "text" && bytesText === null
      ? "hex"
      : (bytesPick ?? (bytesText !== null ? "text" : "hex"));

  const geometry = useMemo(
    () => (column.kind === "geometry" && !bytes ? parseWkt(text) : null),
    [column.kind, bytes, text],
  );
  const language = raw || bytes ? null : detectLanguage(column.kind, column.dataType, text);

  const save = () => {
    if (!editable || !dirty) return onClose();
    const result = parseFieldText(column, text, shown);
    if (!result.ok) return setError(result.error);
    onSave?.(result.value);
    // What was saved is the value now: the edit is staged, no longer pending (the floating panel
    // stays open on its cell).
    setShown(result.value);
    setText(textOf(result.value, pretty, sorted));
    setError(null);
  };
  useShieldShortcuts(save, onClose, !detached || focused);

  const togglePretty = (next: boolean) => {
    if (!dirty) {
      setPretty(next);
      setText(textOf(shown, next, sorted));
      return setError(null);
    }
    if (jsonError(text) !== null) return setError("Fix the JSON first");
    setPretty(next);
    setText(reformatJson(text, next ? 2 : null));
    setError(null);
  };
  /**
   * Sort keys: the JSON's object entries in key order, in the Json text and the Tree. Display only,
   * like Format: nothing is staged until the sorted text is saved. An edit in progress is sorted
   * as it stands (and stays as it is when the toggle goes off: it is the user's text by then).
   */
  const toggleSort = (next: boolean) => {
    if (dirty) {
      if (jsonError(text) !== null) return setError("Fix the JSON first");
      if (next) setText(sortJsonKeys(text));
    } else setText(textOf(shown, pretty, next));
    setError(null);
    setSorted(next);
    writeSortKeys("quicklook", next);
  };
  /** Show raw: the stored text as it is (no formatting, sorting, colours, tree or map). */
  const toggleRaw = (next: boolean) => {
    setRaw(next);
    if (!next || !json || !(pretty || sorted)) return;
    if (!dirty) setText(textOf(shown, false, false));
    else if (jsonError(text) === null) setText(reformatJson(text, null));
    else return setError("Fix the JSON first");
    setError(null);
    setPretty(false);
    // Not remembered: raw is a look at the stored text, not a choice about sorting.
    setSorted(false);
  };

  const loadWhole = async () => {
    if (!loadFull) return;
    setFullState({ status: "loading" });
    try {
      const v = await loadFull();
      if (v.t === "bytes" && v.v.truncated) throw new Error("Only a preview could be loaded.");
      setShown(v);
      if (!dirty) {
        setText(textOf(v, pretty, sorted));
        setError(null);
      }
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
  const showsText =
    view === "value" &&
    !(bytes && !(editable && bytesView === "hex")) &&
    !(json && !raw && jsonView === "tree") &&
    !(geometry && !raw && geoView === "map");
  const changeText = (t: string) => {
    setText(t);
    setError(null);
  };

  const titleText = (
    <span className="flex min-w-0 items-center gap-2">
      <span className="truncate font-mono">{column.name}</span>
      <span className="text-muted-foreground shrink-0 font-mono text-[11px] font-normal">
        {column.dataType}
        {rowNumber !== undefined && ` · row ${rowNumber}`}
      </span>
    </span>
  );
  const toolbar = (
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
      {view === "value" && json && !raw && (
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
      {view === "value" && json && !raw && jsonView === "json" && (
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
      {view === "value" && json && !raw && (
        <Toggle
          pressed={sorted}
          onChange={toggleSort}
          title="List the keys of every object in order (display only: nothing is staged until you save)"
        >
          Sort keys
        </Toggle>
      )}
      {view === "value" && geometry && !raw && (
        <Seg
          label="Geometry view"
          value={geoView}
          options={[
            { value: "map", label: "Map" },
            { value: "text", label: "WKT" },
          ]}
          onChange={setGeoView}
        />
      )}
      {view === "value" && bytes && (shown.t === "bytes" || editable) && (
        <Seg
          label="Binary view"
          value={bytesView}
          options={[
            { value: "hex", label: "Hex" },
            { value: "base64", label: "Base64" },
            ...(bytesText !== null ? [{ value: "text" as const, label: "Text" }] : []),
          ]}
          onChange={setBytesPick}
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
          {fullState?.status === "loading" ? <Loader2 className="animate-spin" /> : <Download />}
          Load full value
        </Button>
      )}
      {showsText && !bytes && (
        <Toggle pressed={wrap} onChange={setWrap} title="Wrap long lines">
          Wrap
        </Toggle>
      )}
      {view === "value" && !bytes && (
        <Toggle
          pressed={raw}
          onChange={toggleRaw}
          title="Show the stored value as it is: no formatting, colours, tree or map"
        >
          Show raw
        </Toggle>
      )}
      {!detached && onDetach && (
        <Button
          variant="ghost"
          size="xs"
          title="Detach: a floating panel that stays open and follows the selected cell"
          onClick={onDetach}
          className="text-muted-foreground hover:text-foreground h-6 text-[11px]"
        >
          <PictureInPicture2 /> Detach
        </Button>
      )}
    </div>
  );
  const body =
    view === "referenced" && loadReferenced ? (
      <ReferencedBody load={loadReferenced} onOpen={onOpenReferenced} />
    ) : bytes && editable && bytesView === "hex" ? (
      <div className="flex min-h-0 flex-1 flex-col gap-2">
        {shown.t === "bytes" && <ImagePreview base64={shown.v.base64} />}
        <CodeView
          text={text}
          onChange={changeText}
          wrap
          label={`Hex bytes of ${column.name}`}
          autoFocus={!detached}
          className="font-mono"
        />
      </div>
    ) : bytes && shown.t === "bytes" ? (
      <BytesBody value={shown} view={bytesView} text={bytesText} column={column.name} />
    ) : bytes ? (
      <div className="text-null flex flex-1 items-center justify-center text-xs italic">
        {cellDisplay(shown).text}
      </div>
    ) : json && !raw && jsonView === "tree" ? (
      <JsonTree text={text} sort={sorted} />
    ) : geometry && !raw && geoView === "map" ? (
      <MapView g={geometry} typeName={column.dataType} />
    ) : (
      <CodeView
        text={text}
        onChange={changeText}
        readOnly={!editable}
        wrap={wrap}
        language={language}
        label={`Value of ${column.name}`}
        autoFocus={!detached}
      />
    );
  const status = (
    <p
      role={error ? "alert" : undefined}
      className={cn("min-w-0 truncate text-[11px]", error ? "text-error" : "text-muted-foreground")}
    >
      {error ??
        (isNull && view === "value" && !(bytes && !editable)
          ? "NULL"
          : dirty
            ? "Edited — not staged yet"
            : editable
              ? ""
              : "Read-only")}
    </p>
  );
  const buttons = (
    <div className="flex shrink-0 gap-1.5">
      {!detached && (
        <Button variant="ghost" size="sm" onClick={onClose}>
          {editable ? "Cancel" : "Close"}
        </Button>
      )}
      {detached && dirty && (
        <Button
          variant="ghost"
          size="sm"
          title="Drop the edit; the panel follows the focused cell again"
          onClick={() => {
            setText(base);
            setError(null);
          }}
        >
          Discard
        </Button>
      )}
      {editable && (
        <Button size="sm" onClick={save} disabled={!dirty}>
          Save
          <kbd className="ml-1 text-[10px] opacity-70">{formatCombo("Mod+Enter")}</kbd>
        </Button>
      )}
    </div>
  );
  const fullError = fullState?.status === "error" && (
    <p role="alert" className="text-error text-[11px]">
      {fullState.error}
    </p>
  );

  if (detached)
    return (
      <FloatingPanel
        label={`Quick Look: ${column.name}`}
        title={titleText}
        onClose={onClose}
        onKeyDown={onKeyDown}
        onFocusChange={setFocused}
      >
        {toolbar}
        {body}
        {fullError}
        <div className="flex items-center justify-between gap-2">
          {status}
          {buttons}
        </div>
      </FloatingPanel>
    );

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
          <DialogTitle className="flex items-center gap-2 text-sm">{titleText}</DialogTitle>
          <DialogDescription className="sr-only">
            The cell value in a larger editor. Save with Command-Return, close with Escape.
          </DialogDescription>
        </DialogHeader>
        {toolbar}
        {body}
        {fullError}
        <DialogFooter className="items-center sm:justify-between">
          {status}
          {buttons}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
