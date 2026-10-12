import { FileDown, Minus, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import type { ColumnMeta, Conjunction, FilterOp } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { newFilterRow, type FilterRowSpec } from "@/stores/tableTabs";
import { isRawSqlRow, opDef, OPS, RAW_SQL_COLUMN, whereOfPageSql } from "./filters";

const RAW_SQL_PLACEHOLDER = "e.g. age > 30 AND name LIKE 'A%'";

/** Display form of an action's primary shortcut ("" when it has none). */
const keysOf = (id: ActionId) => formatCombo(shortcutFor(id) ?? "");

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

/**
 * The filter rows above the grid (TablePlus): `[✓] [column] [operator] [value] [Apply] [−] [+]`,
 * then a strip with the applied SQL, the keys, Clear and Apply All. Every Apply applies all rows.
 * ↩ in a value (or ⌘↩ anywhere) applies, ⌘I duplicates the focused row, ⇧⌘I removes it and ⎋
 * closes the bar.
 */
export function FilterBar({
  columns,
  rows,
  conjunction,
  dirty,
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
  /** The current page's SELECT (the "SQL" button shows its WHERE). */
  sql?: string | null;
  onChange: (rows: FilterRowSpec[], conjunction: Conjunction) => void;
  onApply: () => void;
  onClear: () => void;
  /** ⎋ with the focus in the bar (and no popover open). */
  onClose?: () => void;
  /** Exports the rows the applied filters select. */
  onExport?: () => void;
}) {
  const barRef = useRef<HTMLDivElement>(null);
  /** Index of the row whose value field takes the focus once `rows` changes (⌘I / ⇧⌘I / +). */
  const focusAfter = useRef<number | null>(null);
  useEffect(() => {
    const i = focusAfter.current;
    if (i === null) return;
    focusAfter.current = null;
    const els = barRef.current?.querySelectorAll<HTMLElement>("[data-filter-row]");
    const row = els?.[Math.min(i, els.length - 1)];
    (
      row?.querySelector<HTMLElement>("[data-filter-value]") ??
      row?.querySelector<HTMLElement>("input, button")
    )?.focus();
  }, [rows]);

  const update = (id: string, patch: Partial<FilterRowSpec>) =>
    onChange(
      rows.map((r) => (r.id === id ? { ...r, ...patch } : r)),
      conjunction,
    );
  const add = () => onChange([...rows, newFilterRow(columns[0]?.name ?? "")], conjunction);
  /** `+`: a blank condition on the same column, right under `id`. */
  const addAfter = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return add();
    const next = [...rows];
    next.splice(at + 1, 0, newFilterRow(rows[at]!.column));
    focusAfter.current = at + 1;
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
    focusAfter.current = at + 1;
    onChange(next, conjunction);
  };
  const removeFocused = (id: string) => {
    const at = rows.findIndex((r) => r.id === id);
    if (at < 0) return;
    focusAfter.current = Math.max(0, at === rows.length - 1 ? at - 1 : at);
    remove(id);
  };

  // ⌘↩ / ⌘I / ⇧⌘I are the keymap's "filter" scope (`data-scope="filter"` below): the keyboard
  // provider resolves them before any control sees the key (a select trigger would open on ↩),
  // and they act on the row that holds the focus.
  const rowOf = (target: Element | null) =>
    target?.closest<HTMLElement>("[data-filter-row]")?.dataset.filterRow ?? null;
  const [hasFocus, setHasFocus] = useState(false);
  useAction("filter.apply", hasFocus ? onApply : null);
  useAction(
    "filter.duplicate",
    hasFocus
      ? () => {
          const id = rowOf(document.activeElement);
          if (id) duplicate(id);
          else add();
        }
      : null,
  );
  useAction(
    "filter.remove",
    hasFocus
      ? () => {
          const id = rowOf(document.activeElement);
          if (id) removeFocused(id);
        }
      : null,
  );

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Escape" || e.defaultPrevented || !onClose) return;
    if (!e.currentTarget.contains(e.target as Node)) return;
    e.preventDefault();
    onClose();
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      onApply();
    }
  };
  const applyTitle = `Apply all filters (${formatCombo("Enter")} in a value, ${keysOf("filter.apply")} anywhere)`;
  // AND / OR sits in its own column once there is more than one condition to join.
  const joined = rows.length > 1;
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
          const hasValue = raw || def.arity !== 0;
          return (
            <div key={row.id} data-filter-row={row.id} className="flex items-center gap-1.5">
              <Checkbox
                checked={row.enabled}
                onCheckedChange={(v) => update(row.id, { enabled: v === true })}
                aria-label="Enable filter"
                title={row.enabled ? "Applied (uncheck to skip it)" : "Skipped (check to apply it)"}
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
              <Select value={row.column} onValueChange={(v) => update(row.id, { column: v })}>
                <SelectTrigger
                  size="sm"
                  aria-label="Filter column"
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
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  className="bg-surface h-7 min-w-0 flex-1 font-mono text-xs"
                  autoFocus={i === rows.length - 1 && row.value === ""}
                />
              ) : (
                <Select value={row.op} onValueChange={(v) => update(row.id, { op: v as FilterOp })}>
                  <SelectTrigger
                    size="sm"
                    aria-label="Filter operator"
                    className="bg-surface h-7 w-28 shrink-0 font-mono text-xs"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {OPS.map((o) => (
                      <SelectItem key={o.op} value={o.op} className="font-mono text-xs">
                        {o.label}
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
                  placeholder={
                    def.arity === "list"
                      ? "a, b, c"
                      : def.op === "like" || def.op === "notLike"
                        ? "%pattern%"
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
                    className="bg-surface h-7 min-w-20 flex-1 font-mono text-xs"
                  />
                </>
              )}
              {!hasValue && <span className="flex-1" />}
              <Button
                variant="outline"
                size="xs"
                className="h-7 shrink-0"
                onClick={onApply}
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
          {keysOf("filter.apply")} apply all · {keysOf("filter.duplicate")} duplicate ·{" "}
          {keysOf("filter.remove")} remove · {formatCombo("Escape")} close
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
        <Button
          size="xs"
          onClick={onApply}
          variant={dirty ? "default" : "outline"}
          title={applyTitle}
        >
          Apply All
        </Button>
      </div>
    </div>
  );
}
