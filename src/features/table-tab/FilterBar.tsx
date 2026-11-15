import { ChevronDown, Copy, FileDown, Minus, Plus, Settings } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { consoleSegments } from "@/features/console/highlight";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { useAction, type ActionId } from "@/features/palette/actions";
import type { ColumnMeta, Conjunction, Dialect, FilterOp, RawFilter } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import {
  FILTER_DEFAULT_COLUMNS,
  FILTER_DEFAULT_OPERATORS,
  FILTER_DEFAULT_STATES,
  setPref,
  usePrefs,
  type FilterDefaultColumn,
  type FilterDefaultOperator,
  type FilterDefaultState,
} from "@/stores/prefs";
import type { FilterRowSpec } from "@/stores/tableTabs";
import { BAR_HOVER } from "./BarParts";
import { focusFilterRow, type FilterPart } from "./filterFocus";
import { RawSqlInput } from "./RawSqlInput";
import {
  ANY_COLUMN,
  CORE_FILTERS,
  defaultFilterRow,
  isAnyColumnRow,
  isRawSqlRow,
  isTextValueOp,
  opAllowed,
  opDef,
  opLabel,
  opsFor,
  RAW_SQL_COLUMN,
  rowValueError,
  targetOf,
  whereOfPageSql,
  type FilterSession,
  type OpDef,
} from "./filters";

const RAW_SQL_PLACEHOLDER = "e.g. age > 30 AND name LIKE 'A%'";
/**
 * A field of the bar (docs/ui-conventions.md, Forms): h-6, 11px at every window width (the
 * primitive's `md:text-sm` would win otherwise); `data-[size=sm]:h-6` beats a small select's h-7.
 */
const FIELD_CLASS =
  "bg-surface h-6 rounded-md font-mono text-xs md:text-xs data-[size=sm]:h-6 data-[size=sm]:rounded-md";

/** How the raw filter row reads for each kind of raw filter a session takes. */
const RAW_FILTER: Record<RawFilter, { label: string; hint: string; placeholder: string }> = {
  sqlWhere: { label: "Raw SQL", hint: "WHERE …", placeholder: RAW_SQL_PLACEHOLDER },
  json: { label: "Raw query", hint: "{ … }", placeholder: 'e.g. {"age": {"$gt": 30}}' },
  queryString: { label: "Query string", hint: "q=…", placeholder: "e.g. age:>30 AND name:A*" },
};

/** The hint in a value field: a list, a pattern, JSON, hex (a blob column) or any column's text. */
function valuePlaceholder(def: OpDef, any: boolean, hex: boolean): string {
  if (hex) return def.arity === "list" ? "hex, hex, …" : "hex, e.g. 00ff";
  if (def.arity === "list") return "a, b, c";
  if (def.op === "like" || def.op === "notLike" || def.op === "ilike") return "%pattern%";
  if (def.op === "jsonContains" || def.op === "jsonNotContains") return '{"key": "value"}';
  return any ? "value in any column" : "value";
}

/** Display form of an action's primary shortcut ("" when it has none). */
const keysOf = (id: ActionId) => formatCombo(shortcutFor(id) ?? "");

const COLUMN_LABELS: Record<FilterDefaultColumn, string> = {
  primaryKey: "Primary key when available",
  anyColumn: "Any column",
  rawSql: "Raw SQL",
};
const OPERATOR_LABELS: Record<FilterDefaultOperator, string> = { eq: "=", contains: "Contains" };
const STATE_LABELS: Record<FilterDefaultState, string> = {
  restore: "Restore the last filter state",
  show: "Always show",
  hide: "Always hide",
};

/**
 * The "SQL" button: the SELECT the table shows now (the page's own, values inlined, highlighted
 * like the console's), or only its WHERE; Copy puts the shown text on the clipboard.
 */
function SqlButton({
  sql,
  dirty,
  dialect,
}: {
  sql: string | null;
  dirty: boolean;
  /** Highlights the query; null: another engine's query, shown plain. */
  dialect: Dialect | null;
}) {
  const [view, setView] = useState<"full" | "where">("full");
  const where = whereOfPageSql(sql);
  const applyKeys = shortcutFor("filter.apply");
  const shown = view === "full" ? sql : where ? `WHERE ${where}` : null;
  const copy = async () => {
    if (!shown) return;
    try {
      await writeClipboard(shown);
      toast.success("Copied");
    } catch (e) {
      toast.error("Could not copy to the clipboard", { description: (e as Error).message });
    }
  };
  const tab = (id: typeof view, label: string) => (
    <button
      type="button"
      role="radio"
      aria-checked={view === id}
      onClick={() => setView(id)}
      className={cn(
        "focus-visible:ring-ring/50 rounded px-1.5 text-xs leading-5 transition-colors outline-none focus-visible:ring-2",
        view === id
          ? "bg-brand-soft text-brand"
          : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {label}
    </button>
  );
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="xs"
          aria-label="Show the filter's SQL"
          title="Show the query of the rows shown"
          className={cn("text-muted-foreground hover:text-foreground font-mono", BAR_HOVER)}
        >
          SQL
        </Button>
      </PopoverTrigger>
      <PopoverContent
        side="bottom"
        align="start"
        className="w-[32rem] max-w-[90vw] gap-2 p-3 text-xs"
      >
        <div className="flex items-center gap-1">
          <div role="radiogroup" aria-label="SQL to show" className="flex items-center gap-0.5">
            {tab("full", "Full query")}
            {tab("where", "WHERE only")}
          </div>
          <span className="flex-1" />
          <Button
            variant="ghost"
            size="xs"
            disabled={!shown}
            onClick={() => void copy()}
            className="text-muted-foreground hover:text-foreground"
          >
            <Copy />
            Copy
          </Button>
        </div>
        {shown ? (
          <pre
            data-testid={view === "full" ? "filter-sql" : "filter-where"}
            className="bg-surface border-border max-h-60 overflow-auto rounded-md border px-2 py-1.5 font-mono text-[11.5px] whitespace-pre-wrap select-text"
          >
            {consoleSegments(shown, dialect).map((seg, i) => (
              <span key={i} className={seg.cls || undefined}>
                {seg.text}
              </span>
            ))}
          </pre>
        ) : (
          <p className="text-muted-foreground text-xs">
            {sql === null ? "The query shows once the page has loaded." : "No filter is applied."}
          </p>
        )}
        {dirty && (
          <p className="text-muted-foreground text-xs">
            The bar has changes that are not applied yet
            {applyKeys ? (
              <>
                {" ("}
                <Kbd combo={applyKeys} className="mx-0.5 align-middle" />
                {" applies them)."}
              </>
            ) : (
              "."
            )}
          </p>
        )}
      </PopoverContent>
    </Popover>
  );
}

/** ⚙: the defaults new filter rows and newly opened tables start from (preferences). */
function FilterSettings() {
  const column = usePrefs((p) => p.filterDefaultColumn);
  const operator = usePrefs((p) => p.filterDefaultOperator);
  const state = usePrefs((p) => p.filterDefaultState);
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Filter settings"
          title="Filter settings"
          className={cn("text-muted-foreground hover:text-foreground", BAR_HOVER)}
        >
          <Settings />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="bottom" className="w-60">
        <DropdownMenuLabel>Default filter column</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={column}
          onValueChange={(v) => setPref("filterDefaultColumn", v as FilterDefaultColumn)}
        >
          {FILTER_DEFAULT_COLUMNS.map((v) => (
            <DropdownMenuRadioItem key={v} value={v}>
              {COLUMN_LABELS[v]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Default filter operator</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={operator}
          onValueChange={(v) => setPref("filterDefaultOperator", v as FilterDefaultOperator)}
        >
          {FILTER_DEFAULT_OPERATORS.map((v) => (
            <DropdownMenuRadioItem key={v} value={v} className="font-mono">
              {OPERATOR_LABELS[v]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Default filter state</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={state}
          onValueChange={(v) => setPref("filterDefaultState", v as FilterDefaultState)}
        >
          {FILTER_DEFAULT_STATES.map((v) => (
            <DropdownMenuRadioItem key={v} value={v}>
              {STATE_LABELS[v]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** `[Apply All ▾]`: AND (⌘↩, the default) or OR (⇧⌘↩) over the checked rows. */
function ApplyAll({
  dirty,
  title,
  onApply,
}: {
  dirty: boolean;
  title: string;
  onApply: (conjunction: Conjunction) => void;
}) {
  // Changes waiting to be applied make Apply All the bar's primary action (the brand fill).
  const variant = dirty ? "default" : "outline";
  const actionClass = "transition-colors";
  return (
    <div className="flex items-center">
      <Button
        size="xs"
        onClick={() => onApply("and")}
        variant={variant}
        title={title}
        className={cn(actionClass, "rounded-r-none")}
      >
        Apply All
      </Button>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            size="xs"
            variant={variant}
            aria-label="Apply All options"
            className={cn(
              actionClass,
              "border-l-border -ml-px rounded-l-none px-1",
              dirty && "border-l-primary-foreground/30",
            )}
          >
            <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-auto min-w-72">
          <DropdownMenuItem onSelect={() => onApply("and")}>
            Apply All Checked Filters with AND – Default
            <DropdownMenuShortcut>{keysOf("filter.apply")}</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => onApply("or")}>
            Apply All Checked Filters with OR
            <DropdownMenuShortcut>{keysOf("filter.apply-or")}</DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

type Popup = { id: string; part: "column" | "op" };

/**
 * The filter rows above the grid (TablePlus): `[✓] [column] [operator] [value] [Apply] [−] [+]`,
 * then a strip with ⚙, Export, the applied SQL, the keys, Clear and Apply All ▾. The column list
 * starts with Raw SQL (with column / keyword autocomplete) and Any column; the operators follow
 * the driver. A row's Apply (or ↩ in its value) applies that filter alone (TablePlus); Apply All
 * (⌘↩ anywhere) applies the checked rows with AND, ⇧⌘↩ with OR; ⌘I duplicates the
 * focused row (its column and operator, the value left blank) and ⇧⌘I removes it; ⌘↑ / ⌘↓ move
 * between rows (⌘↓ on the last one goes down to the data), ⌘B checks / unchecks one, ⌘← / ⌘→ open
 * its column / operator list, and ⎋ closes the bar.
 */
export function FilterBar({
  columns,
  rows,
  conjunction,
  dirty,
  dialect = "postgres",
  session = CORE_FILTERS,
  sql = null,
  onChange,
  onApply,
  onClear,
  onClose,
  onExport,
  focusRequest = 0,
  focusRowId = null,
  onApplyRow,
  appliedRows,
  table = "",
  onFocusData,
}: {
  columns: ColumnMeta[];
  rows: FilterRowSpec[];
  conjunction: Conjunction;
  /** The rows differ from the applied filters. */
  dirty: boolean;
  /** Decides the "– Case …" labels and highlights the SQL; null: another engine's. */
  dialect?: Dialect | null;
  /** What the session's filters offer (operators, raw filter, Any column): a core session's. */
  session?: FilterSession;
  /** The current page's SELECT (the "SQL" button shows its WHERE). */
  sql?: string | null;
  onChange: (rows: FilterRowSpec[], conjunction: Conjunction) => void;
  /** Applies the checked rows, joined with `conjunction` (the bar's own when not given). */
  onApply: (conjunction?: Conjunction) => void;
  onClear: () => void;
  /** ⎋ with the focus in the bar (and no popover open). */
  onClose?: () => void;
  /** Exports the rows the applied filters select. */
  onExport?: () => void;
  /**
   * Bumped by ⌘F (and the Filters button): the value field of `focusRowId` (the last applied
   * filter) takes the focus, else the last row's. 0 = no request yet.
   */
  focusRequest?: number;
  focusRowId?: string | null;
  /** A row's Apply / ↩: applies that filter alone; without it they apply all checked rows. */
  onApplyRow?: (id: string) => void;
  /** The applied filters: a row that matches its applied self reads "Applied". */
  appliedRows?: readonly FilterRowSpec[];
  /** The table's name (Raw SQL completes its columns). */
  table?: string;
  /** ⌘↓ on the last row: the focus goes down to the data grid. */
  onFocusData?: () => void;
}) {
  const barRef = useRef<HTMLDivElement>(null);
  const defaultColumn = usePrefs((p) => p.filterDefaultColumn);
  const defaultOperator = usePrefs((p) => p.filterDefaultOperator);
  const fresh = () =>
    defaultFilterRow(columns, { column: defaultColumn, operator: defaultOperator }, session);
  const defaultOp: FilterOp = fresh().op;
  const offered = session.ops;
  const rawFilter = session.raw === null ? null : RAW_FILTER[session.raw];
  /** Where the focus goes once `rows` changes (⌘I / ⇧⌘I / + / ⌘↑ / ⌘↓). */
  const focusAfter = useRef<{ index: number; part: FilterPart } | null>(null);
  const focusRow = (index: number, part: FilterPart) => focusFilterRow(barRef.current, index, part);
  useEffect(() => {
    const f = focusAfter.current;
    if (f === null) return;
    focusAfter.current = null;
    focusRow(f.index, f.part);
  }, [rows]);
  // ⌘F jumps to the last applied filter (TablePlus b190), its text selected to type over.
  useEffect(() => {
    if (focusRequest === 0) return;
    const at = focusRowId === null ? -1 : rows.findIndex((r) => r.id === focusRowId);
    focusRow(at >= 0 ? at : rows.length - 1, "value");
    const el = document.activeElement;
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) el.select();
    // Only a new request moves the focus, not edits to the rows.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusRequest]);
  const [popup, setPopup] = useState<Popup | null>(null);

  const update = (id: string, patch: Partial<FilterRowSpec>) =>
    onChange(
      rows.map((r) => (r.id === id ? { ...r, ...patch } : r)),
      conjunction,
    );
  /** A new column: an operator it does not offer goes back to the default one. */
  const changeColumn = (row: FilterRowSpec, column: string) => {
    const keep = column === RAW_SQL_COLUMN || opAllowed(row.op, offered, targetOf(column, columns));
    update(row.id, keep ? { column } : { column, op: defaultOp });
  };
  const add = () => onChange([...rows, fresh()], conjunction);
  /** A blank condition on the column and operator of `row` (the value left to type). */
  const likeRow = (row: FilterRowSpec): FilterRowSpec => {
    const blank = { ...fresh(), column: row.column, op: row.op };
    if (!opAllowed(blank.op, offered, targetOf(blank.column, columns)))
      blank.op = opAllowed("eq", offered) ? "eq" : defaultOp;
    return blank;
  };
  /** `+`: a blank condition on the same column and operator, right under `id`. */
  const addAfter = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return add();
    const next = [...rows];
    next.splice(at + 1, 0, likeRow(rows[at]!));
    focusAfter.current = { index: at + 1, part: "value" };
    onChange(next, conjunction);
  };
  const remove = (id: string) =>
    onChange(
      rows.filter((r) => r.id !== id),
      conjunction,
    );
  /** ⌘I: TablePlus clones the column and the operator only; the value is typed anew. */
  const duplicate = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return;
    const next = [...rows];
    next.splice(at + 1, 0, likeRow(rows[at]!));
    focusAfter.current = { index: at + 1, part: "value" };
    onChange(next, conjunction);
  };
  const removeFocused = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return;
    focusAfter.current = {
      index: Math.max(0, at === rows.length - 1 ? at - 1 : at),
      part: "value",
    };
    remove(id);
  };

  // The bar's keys are the keymap's "filter" scope (`data-scope="filter"` below): the keyboard
  // provider resolves them before any control sees the key (a select trigger would open on ↩),
  // and they act on the row that holds the focus.
  const rowOf = (target: Element | null) =>
    target?.closest<HTMLElement>("[data-filter-row]")?.dataset.filterRow ?? null;
  const partOf = (target: Element | null): FilterPart =>
    (target?.closest<HTMLElement>("[data-filter-part]")?.dataset.filterPart as
      FilterPart | undefined) ?? "value";
  const focusedRow = () => {
    const id = rowOf(document.activeElement);
    const at = id === null ? -1 : rows.findIndex((r) => r.id === id);
    return at < 0 ? null : { id: id!, at, row: rows[at]! };
  };
  const [hasFocus, setHasFocus] = useState(false);
  const on = <T,>(handler: T): T | null => (hasFocus ? handler : null);
  useAction(
    "filter.apply",
    on(() => onApply("and")),
  );
  useAction(
    "filter.apply-or",
    on(() => onApply("or")),
  );
  useAction(
    "filter.duplicate",
    on(() => {
      const f = focusedRow();
      if (f) duplicate(f.id);
      else add();
    }),
  );
  useAction(
    "filter.remove",
    on(() => {
      const f = focusedRow();
      if (f) removeFocused(f.id);
    }),
  );
  const step = (by: number) => {
    const f = focusedRow();
    if (!f) return;
    // ⌘↓ on the last row leaves the bar for the data.
    if (by > 0 && f.at === rows.length - 1 && onFocusData) return onFocusData();
    const part = partOf(document.activeElement);
    focusRow(Math.max(0, Math.min(rows.length - 1, f.at + by)), part);
  };
  useAction(
    "filter.prev",
    on(() => step(-1)),
  );
  useAction(
    "filter.next",
    on(() => step(1)),
  );
  useAction(
    "filter.toggle",
    on(() => {
      const f = focusedRow();
      if (f) update(f.id, { enabled: !f.row.enabled });
    }),
  );
  useAction(
    "filter.column",
    on(() => {
      const f = focusedRow();
      if (f) setPopup({ id: f.id, part: "column" });
    }),
  );
  useAction(
    "filter.operator",
    on(() => {
      const f = focusedRow();
      if (f && !isRawSqlRow(f.row)) setPopup({ id: f.id, part: "op" });
    }),
  );

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Escape" || e.defaultPrevented || !onClose) return;
    if (!e.currentTarget.contains(e.target as Node)) return;
    e.preventDefault();
    onClose();
  };
  const applyRow = (id: string) => (onApplyRow ? onApplyRow(id) : onApply());
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      const id = rowOf(e.target as Element);
      if (id) applyRow(id);
      else onApply();
    }
  };
  /** The row is applied as it reads now (its Apply button says "Applied"). */
  const isApplied = (row: FilterRowSpec) =>
    appliedRows?.some(
      (a) =>
        a.id === row.id &&
        a.enabled &&
        row.enabled &&
        a.column === row.column &&
        a.op === row.op &&
        a.value === row.value &&
        a.value2 === row.value2,
    ) ?? false;
  const columnNames = columns.map((c) => c.name);
  const applyTitle = `Apply all checked filters (${formatCombo("Enter")} in a value, ${keysOf("filter.apply")} with AND, ${keysOf("filter.apply-or")} with OR)`;
  const applyShortcut = shortcutFor("filter.apply");
  // AND / OR sits in its own column once there is more than one condition to join.
  const joined = rows.length > 1;
  const popupProps = (row: FilterRowSpec, part: Popup["part"]) => ({
    open: popup?.id === row.id && popup.part === part,
    onOpenChange: (open: boolean) => setPopup(open ? { id: row.id, part } : null),
  });
  return (
    <div
      ref={barRef}
      role="region"
      aria-label="Filters"
      className="border-border bg-surface-2 @container flex shrink-0 flex-col border-b"
      data-scope="filter"
      onKeyDown={onKeyDown}
      onFocus={(e) => {
        if (e.currentTarget.contains(e.target as Node)) setHasFocus(true);
      }}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setHasFocus(false);
      }}
    >
      <div className="flex flex-col gap-1 px-2 py-2">
        {rows.map((row, i) => {
          const def = opDef(row.op);
          const raw = isRawSqlRow(row);
          const any = isAnyColumnRow(row);
          const hasValue = raw || def.arity !== 0;
          const ops = opsFor(offered, targetOf(row.column, columns));
          // A blob column filters by hex: its message says so when the text is not hex.
          const valueError = rowValueError(row, columns);
          const hex = columns.find((c) => c.name === row.column)?.kind === "bytes";
          return (
            <div
              key={row.id}
              data-filter-row={row.id}
              className="flex min-w-0 flex-wrap items-center gap-1 @lg:flex-nowrap"
            >
              <Checkbox
                checked={row.enabled}
                onCheckedChange={(v) => update(row.id, { enabled: v === true })}
                aria-label="Enable filter"
                data-filter-part="check"
                title={`${row.enabled ? "Applied (uncheck to skip it)" : "Skipped (check to apply it)"} · ${keysOf("filter.toggle")}`}
              />
              {joined && (
                <div className="flex w-10 shrink-0 justify-center">
                  {i > 0 && (
                    <button
                      type="button"
                      className="text-brand hover:bg-accent focus-visible:ring-ring/50 rounded px-1 font-mono text-xs leading-5 transition-colors outline-none focus-visible:ring-2"
                      title="Toggle AND / OR for all conditions"
                      onClick={() => onChange(rows, conjunction === "and" ? "or" : "and")}
                    >
                      {conjunction.toUpperCase()}
                    </button>
                  )}
                </div>
              )}
              <Select
                value={row.column}
                onValueChange={(v) => changeColumn(row, v)}
                {...popupProps(row, "column")}
              >
                <SelectTrigger
                  size="sm"
                  aria-label="Filter column"
                  data-filter-part="column"
                  title={`Column (${keysOf("filter.column")})`}
                  className={cn(
                    FIELD_CLASS,
                    "shrink-0",
                    raw ? "w-28" : "w-40 @max-xl:w-32 @max-md:w-24",
                  )}
                >
                  <SelectValue placeholder="Column" />
                </SelectTrigger>
                <SelectContent>
                  {rawFilter && (
                    <SelectItem value={RAW_SQL_COLUMN} className="text-xs">
                      {rawFilter.label}
                      <span className="text-muted-foreground ml-2 font-mono">{rawFilter.hint}</span>
                    </SelectItem>
                  )}
                  {session.anyColumn && (
                    <SelectItem value={ANY_COLUMN} className="text-xs">
                      Any column
                    </SelectItem>
                  )}
                  {(rawFilter || session.anyColumn) && <SelectSeparator />}
                  {columns.map((c) => (
                    <SelectItem key={c.name} value={c.name} className="font-mono text-xs">
                      {c.name}
                      <span className="text-muted-foreground ml-2">{c.dataType}</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {raw ? (
                <RawSqlInput
                  value={row.value}
                  onValueChange={(value) => update(row.id, { value })}
                  onEnter={() => applyRow(row.id)}
                  dialect={session.raw === "sqlWhere" ? dialect : null}
                  table={table}
                  columns={columnNames}
                  placeholder={rawFilter?.placeholder ?? RAW_SQL_PLACEHOLDER}
                  aria-label={session.raw === "sqlWhere" ? "Raw SQL condition" : "Raw filter"}
                  data-filter-value
                  data-filter-part="value"
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  className={FIELD_CLASS}
                  autoFocus={i === rows.length - 1 && row.value === ""}
                />
              ) : (
                <Select
                  value={row.op}
                  onValueChange={(v) => update(row.id, { op: v as FilterOp })}
                  {...popupProps(row, "op")}
                >
                  <SelectTrigger
                    size="sm"
                    aria-label="Filter operator"
                    data-filter-part="op"
                    title={`Operator (${keysOf("filter.operator")})`}
                    className={cn(FIELD_CLASS, "w-28 shrink-0 @max-xl:w-24 @max-md:w-20")}
                  >
                    <SelectValue>{opLabel(row.op, dialect)}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {ops.map((o) => (
                      <SelectItem key={o.op} value={o.op} className="font-mono text-xs">
                        {opLabel(o.op, dialect)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              {!raw && def.arity !== 0 && (
                <Input
                  value={row.value}
                  onChange={(e) => update(row.id, { value: e.target.value })}
                  onKeyDown={onKey}
                  aria-label="Filter value"
                  aria-invalid={valueError ? true : undefined}
                  title={valueError ?? undefined}
                  data-filter-value
                  data-filter-part="value"
                  placeholder={valuePlaceholder(def, any, hex && !isTextValueOp(row.op))}
                  className={cn(FIELD_CLASS, "min-w-12 flex-1")}
                  autoFocus={i === rows.length - 1 && row.value === ""}
                />
              )}
              {!raw && def.arity === 2 && (
                <>
                  <span className="text-muted-foreground text-xs">AND</span>
                  <Input
                    value={row.value2}
                    onChange={(e) => update(row.id, { value2: e.target.value })}
                    onKeyDown={onKey}
                    aria-label="Second filter value"
                    aria-invalid={valueError ? true : undefined}
                    title={valueError ?? undefined}
                    placeholder={hex ? "hex, e.g. ffff" : undefined}
                    data-filter-part="value2"
                    className={cn(FIELD_CLASS, "min-w-12 flex-1")}
                  />
                </>
              )}
              {!hasValue && <span className="flex-1" />}
              <div className="ml-auto flex shrink-0 items-center">
                <Button
                  variant={isApplied(row) ? "ghost" : "outline"}
                  size="xs"
                  className={cn(
                    "mr-1 w-16 transition-colors",
                    isApplied(row) && cn("text-muted-foreground", BAR_HOVER),
                  )}
                  data-filter-part="apply"
                  onClick={() => applyRow(row.id)}
                  title={
                    onApplyRow
                      ? `Apply this filter alone (${formatCombo("Enter")} in its value); Apply All applies the checked ones`
                      : applyTitle
                  }
                >
                  {isApplied(row) ? "Applied" : "Apply"}
                </Button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className={cn("text-muted-foreground hover:text-foreground", BAR_HOVER)}
                  aria-label="Remove filter"
                  title={`Remove this condition (${keysOf("filter.remove")})`}
                  onClick={() => remove(row.id)}
                >
                  <Minus />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className={cn("text-muted-foreground hover:text-foreground", BAR_HOVER)}
                  aria-label="Add filter"
                  title={`Add a condition below (${keysOf("filter.duplicate")} duplicates this one)`}
                  onClick={() => addAfter(row.id)}
                >
                  <Plus />
                </Button>
              </div>
            </div>
          );
        })}
      </div>
      <div className="border-border flex h-8 items-center gap-1 border-t px-2">
        <FilterSettings />
        {onExport && (
          <Button
            variant="ghost"
            size="xs"
            onClick={onExport}
            className={cn("text-muted-foreground hover:text-foreground", BAR_HOVER)}
            title="Export the filtered rows"
          >
            <FileDown />
            Export
          </Button>
        )}
        <SqlButton sql={sql} dirty={dirty} dialect={dialect} />
        <span className="flex-1" />
        <Button
          variant="ghost"
          size="xs"
          onClick={onClear}
          disabled={rows.length === 0}
          className={cn("text-muted-foreground hover:text-foreground", BAR_HOVER)}
        >
          Clear
        </Button>
        {applyShortcut && <Kbd combo={applyShortcut} className="mr-1 @max-lg:hidden" />}
        <ApplyAll dirty={dirty} title={applyTitle} onApply={onApply} />
      </div>
    </div>
  );
}
