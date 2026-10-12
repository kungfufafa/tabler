import { ChevronDown, FileDown, Minus, Plus, Settings } from "lucide-react";
import { useEffect, useRef, useState } from "react";
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
import { useAction, type ActionId } from "@/features/palette/actions";
import type { ColumnMeta, Conjunction, Dialect, FilterOp } from "@/lib/ipc";
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
import {
  ANY_COLUMN,
  defaultFilterRow,
  isAnyColumnRow,
  isRawSqlRow,
  opAllowed,
  opDef,
  opLabel,
  opsFor,
  RAW_SQL_COLUMN,
  targetOf,
  whereOfPageSql,
} from "./filters";

const RAW_SQL_PLACEHOLDER = "e.g. age > 30 AND name LIKE 'A%'";

/** Display form of an action's primary shortcut ("" when it has none). */
const keysOf = (id: ActionId) => formatCombo(shortcutFor(id) ?? "");

const COLUMN_LABELS: Record<FilterDefaultColumn, string> = {
  primaryKey: "Primary key (if exist)",
  anyColumn: "Any column",
  rawSql: "Raw SQL",
};
const OPERATOR_LABELS: Record<FilterDefaultOperator, string> = { eq: "=", contains: "Contains" };
const STATE_LABELS: Record<FilterDefaultState, string> = {
  restore: "Restore the last filter state",
  show: "Always show",
  hide: "Always hide",
};

/** The applied filter's WHERE, from the page's SELECT (values inlined). */
function SqlButton({ sql, dirty }: { sql: string | null; dirty: boolean }) {
  const where = whereOfPageSql(sql);
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="xs"
          aria-label="Show the filter's SQL"
          title="Show the WHERE clause of the applied filters"
          className="text-muted-foreground hover:text-foreground font-mono text-[11px]"
        >
          SQL
        </Button>
      </PopoverTrigger>
      <PopoverContent side="bottom" align="start" className="w-[28rem] max-w-[90vw] gap-1.5 p-2.5">
        {where ? (
          <pre
            data-testid="filter-where"
            className="bg-surface border-border max-h-60 overflow-auto rounded-md border px-2 py-1.5 font-mono text-[11px] whitespace-pre-wrap select-text"
          >
            <span className="text-brand font-semibold">WHERE</span> {where}
          </pre>
        ) : (
          <p className="text-muted-foreground text-xs">No filter is applied.</p>
        )}
        {dirty && (
          <p className="text-muted-foreground text-[11px]">
            The bar has changes that are not applied yet ({keysOf("filter.apply")} applies them).
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
  const label = "text-muted-foreground py-1 text-[11px] font-medium";
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Filter settings"
          title="Filter settings"
          className="text-muted-foreground hover:text-foreground size-6"
        >
          <Settings />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="bottom" className="w-60 text-xs">
        <DropdownMenuLabel className={label}>Default filter column</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={column}
          onValueChange={(v) => setPref("filterDefaultColumn", v as FilterDefaultColumn)}
        >
          {FILTER_DEFAULT_COLUMNS.map((v) => (
            <DropdownMenuRadioItem key={v} value={v} className="text-xs">
              {COLUMN_LABELS[v]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel className={label}>Default filter operator</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={operator}
          onValueChange={(v) => setPref("filterDefaultOperator", v as FilterDefaultOperator)}
        >
          {FILTER_DEFAULT_OPERATORS.map((v) => (
            <DropdownMenuRadioItem key={v} value={v} className="font-mono text-xs">
              {OPERATOR_LABELS[v]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel className={label}>Default filter state</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={state}
          onValueChange={(v) => setPref("filterDefaultState", v as FilterDefaultState)}
        >
          {FILTER_DEFAULT_STATES.map((v) => (
            <DropdownMenuRadioItem key={v} value={v} className="text-xs">
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
  return (
    <div className="flex items-center">
      <Button
        size="xs"
        onClick={() => onApply("and")}
        variant={dirty ? "default" : "outline"}
        title={title}
        className="rounded-r-none"
      >
        Apply All
      </Button>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            size="xs"
            variant={dirty ? "default" : "outline"}
            aria-label="Apply All options"
            className="-ml-px rounded-l-none px-1"
          >
            <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-auto min-w-72 text-xs">
          <DropdownMenuItem className="text-xs" onSelect={() => onApply("and")}>
            Apply All Checked Filters with AND – Default
            <DropdownMenuShortcut>{keysOf("filter.apply")}</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem className="text-xs" onSelect={() => onApply("or")}>
            Apply All Checked Filters with OR
            <DropdownMenuShortcut>{keysOf("filter.apply-or")}</DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

type Popup = { id: string; part: "column" | "op" };
/** The control kinds a filter row has; ⌘↑ / ⌘↓ keep the focus on the same kind. */
type Part = "check" | "column" | "op" | "value" | "value2" | "apply";

/**
 * The filter rows above the grid (TablePlus): `[✓] [column] [operator] [value] [Apply] [−] [+]`,
 * then a strip with ⚙, Export, the applied SQL, the keys, Clear and Apply All ▾. The column list
 * starts with Raw SQL and Any column; the operators follow the driver. Every Apply applies all
 * checked rows. ↩ in a value (or ⌘↩ anywhere) applies with AND, ⇧⌘↩ with OR; ⌘I duplicates the
 * focused row and ⇧⌘I removes it; ⌘↑ / ⌘↓ move between rows, ⌘B checks / unchecks one, ⌘← / ⌘→
 * open its column / operator list, and ⎋ closes the bar.
 */
export function FilterBar({
  columns,
  rows,
  conjunction,
  dirty,
  dialect = "postgres",
  sql = null,
  onChange,
  onApply,
  onClear,
  onClose,
  onExport,
}: {
  columns: ColumnMeta[];
  rows: FilterRowSpec[];
  conjunction: Conjunction;
  /** The rows differ from the applied filters. */
  dirty: boolean;
  /** Decides the operator list (ILIKE, JSON contains) and its "– Case …" labels. */
  dialect?: Dialect;
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
}) {
  const barRef = useRef<HTMLDivElement>(null);
  const defaultColumn = usePrefs((p) => p.filterDefaultColumn);
  const defaultOperator = usePrefs((p) => p.filterDefaultOperator);
  const fresh = () =>
    defaultFilterRow(columns, { column: defaultColumn, operator: defaultOperator });
  const defaultOp: FilterOp = defaultOperator === "contains" ? "contains" : "eq";
  /** Where the focus goes once `rows` changes (⌘I / ⇧⌘I / + / ⌘↑ / ⌘↓). */
  const focusAfter = useRef<{ index: number; part: Part } | null>(null);
  const focusRow = (index: number, part: Part) => {
    const els = barRef.current?.querySelectorAll<HTMLElement>("[data-filter-row]");
    const row = els?.[Math.max(0, Math.min(index, (els?.length ?? 1) - 1))];
    (
      row?.querySelector<HTMLElement>(`[data-filter-part="${part}"]`) ??
      row?.querySelector<HTMLElement>("[data-filter-value]") ??
      row?.querySelector<HTMLElement>("input, button")
    )?.focus();
  };
  useEffect(() => {
    const f = focusAfter.current;
    if (f === null) return;
    focusAfter.current = null;
    focusRow(f.index, f.part);
  }, [rows]);
  const [popup, setPopup] = useState<Popup | null>(null);

  const update = (id: string, patch: Partial<FilterRowSpec>) =>
    onChange(
      rows.map((r) => (r.id === id ? { ...r, ...patch } : r)),
      conjunction,
    );
  /** A new column: an operator it does not offer goes back to the default one. */
  const changeColumn = (row: FilterRowSpec, column: string) => {
    const keep = column === RAW_SQL_COLUMN || opAllowed(row.op, dialect, targetOf(column, columns));
    update(row.id, keep ? { column } : { column, op: defaultOp });
  };
  const add = () => onChange([...rows, fresh()], conjunction);
  /** `+`: a blank condition on the same column, right under `id`. */
  const addAfter = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return add();
    const next = [...rows];
    const blank = { ...fresh(), column: rows[at]!.column };
    if (!opAllowed(blank.op, dialect, targetOf(blank.column, columns))) blank.op = "eq";
    next.splice(at + 1, 0, blank);
    focusAfter.current = { index: at + 1, part: "value" };
    onChange(next, conjunction);
  };
  const remove = (id: string) =>
    onChange(
      rows.filter((r) => r.id !== id),
      conjunction,
    );
  const duplicate = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return;
    const next = [...rows];
    next.splice(at + 1, 0, { ...rows[at]!, id: crypto.randomUUID() });
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
  const partOf = (target: Element | null): Part =>
    (target?.closest<HTMLElement>("[data-filter-part]")?.dataset.filterPart as Part | undefined) ??
    "value";
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
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      onApply();
    }
  };
  const applyTitle = `Apply all checked filters (${formatCombo("Enter")} in a value, ${keysOf("filter.apply")} with AND, ${keysOf("filter.apply-or")} with OR)`;
  // AND / OR sits in its own column once there is more than one condition to join.
  const joined = rows.length > 1;
  const popupProps = (row: FilterRowSpec, part: Popup["part"]) => ({
    open: popup?.id === row.id && popup.part === part,
    onOpenChange: (open: boolean) => setPopup(open ? { id: row.id, part } : null),
  });
  return (
    <div
      ref={barRef}
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
      <div className="flex flex-col gap-1 px-2 pt-1.5 pb-1">
        {rows.map((row, i) => {
          const def = opDef(row.op);
          const raw = isRawSqlRow(row);
          const any = isAnyColumnRow(row);
          const hasValue = raw || def.arity !== 0;
          const ops = opsFor(dialect, targetOf(row.column, columns));
          return (
            <div key={row.id} data-filter-row={row.id} className="flex items-center gap-1.5">
              <Checkbox
                checked={row.enabled}
                onCheckedChange={(v) => update(row.id, { enabled: v === true })}
                aria-label="Enable filter"
                data-filter-part="check"
                title={`${row.enabled ? "Applied (uncheck to skip it)" : "Skipped (check to apply it)"} · ${keysOf("filter.toggle")}`}
                className="mx-0.5"
              />
              {joined && (
                <div className="flex w-10 shrink-0 justify-center">
                  {i > 0 && (
                    <button
                      type="button"
                      className="text-brand hover:bg-brand-soft rounded px-1 font-mono text-[11px] leading-5"
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
                    "bg-surface h-7 shrink-0 font-mono text-xs",
                    raw ? "w-28" : "w-40 @max-xl:w-32",
                  )}
                >
                  <SelectValue placeholder="Column" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={RAW_SQL_COLUMN} className="text-xs">
                    Raw SQL
                    <span className="text-muted-foreground ml-2 font-mono">WHERE …</span>
                  </SelectItem>
                  <SelectItem value={ANY_COLUMN} className="text-xs">
                    Any column
                  </SelectItem>
                  <SelectSeparator />
                  {columns.map((c) => (
                    <SelectItem key={c.name} value={c.name} className="font-mono text-xs">
                      {c.name}
                      <span className="text-muted-foreground ml-2">{c.dataType}</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {raw ? (
                <Input
                  value={row.value}
                  onChange={(e) => update(row.id, { value: e.target.value })}
                  onKeyDown={onKey}
                  placeholder={RAW_SQL_PLACEHOLDER}
                  aria-label="Raw SQL condition"
                  data-filter-value
                  data-filter-part="value"
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  className="bg-surface h-7 min-w-0 flex-1 font-mono text-xs"
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
                    className="bg-surface h-7 w-28 shrink-0 font-mono text-xs @max-xl:w-24"
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
                  data-filter-value
                  data-filter-part="value"
                  placeholder={
                    def.arity === "list"
                      ? "a, b, c"
                      : def.op === "like" || def.op === "notLike" || def.op === "ilike"
                        ? "%pattern%"
                        : def.op === "jsonContains" || def.op === "jsonNotContains"
                          ? '{"key": "value"}'
                          : any
                            ? "value in any column"
                            : "value"
                  }
                  className="bg-surface h-7 min-w-20 flex-1 font-mono text-xs"
                  autoFocus={i === rows.length - 1 && row.value === ""}
                />
              )}
              {!raw && def.arity === 2 && (
                <>
                  <span className="text-muted-foreground text-[11px]">AND</span>
                  <Input
                    value={row.value2}
                    onChange={(e) => update(row.id, { value2: e.target.value })}
                    onKeyDown={onKey}
                    aria-label="Second filter value"
                    data-filter-part="value2"
                    className="bg-surface h-7 min-w-20 flex-1 font-mono text-xs"
                  />
                </>
              )}
              {!hasValue && <span className="flex-1" />}
              <Button
                variant="outline"
                size="xs"
                className="h-7 shrink-0"
                data-filter-part="apply"
                onClick={() => onApply()}
                title={applyTitle}
              >
                Apply
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground hover:text-foreground size-7 shrink-0"
                aria-label="Remove filter"
                title={`Remove this condition (${keysOf("filter.remove")})`}
                onClick={() => remove(row.id)}
              >
                <Minus />
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground hover:text-foreground -ml-1 size-7 shrink-0"
                aria-label="Add filter"
                title={`Add a condition below (${keysOf("filter.duplicate")} duplicates this one)`}
                onClick={() => addAfter(row.id)}
              >
                <Plus />
              </Button>
            </div>
          );
        })}
      </div>
      <div className="border-border/60 flex h-7 items-center gap-1 border-t px-2">
        <FilterSettings />
        {onExport && (
          <Button
            variant="ghost"
            size="xs"
            onClick={onExport}
            className="text-muted-foreground hover:text-foreground text-[11px]"
            title={`Export the filtered rows (${keysOf("export.open")})`}
          >
            <FileDown />
            Export
          </Button>
        )}
        <SqlButton sql={sql} dirty={dirty} />
        <span className="text-muted-foreground/80 min-w-0 truncate px-1 text-[11px] @max-2xl:hidden">
          {keysOf("filter.apply")} apply all · {keysOf("filter.apply-or")} with OR ·{" "}
          {keysOf("filter.duplicate")} duplicate · {keysOf("filter.remove")} remove ·{" "}
          {keysOf("filter.prev")}
          {keysOf("filter.next")} move · {keysOf("filter.toggle")} check · {formatCombo("Escape")}{" "}
          close
        </span>
        <span className="flex-1" />
        <Button
          variant="ghost"
          size="xs"
          onClick={onClear}
          disabled={rows.length === 0}
          className="text-muted-foreground hover:text-foreground"
        >
          Clear
        </Button>
        <ApplyAll dirty={dirty} title={applyTitle} onApply={onApply} />
      </div>
    </div>
  );
}
