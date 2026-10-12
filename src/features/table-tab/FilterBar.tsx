import { Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ColumnMeta, Conjunction, FilterOp } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { newFilterRow, type FilterRowSpec } from "@/stores/tableTabs";
import { isRawSqlRow, opDef, OPS, RAW_SQL_COLUMN } from "./filters";

const RAW_SQL_PLACEHOLDER = "e.g. age > 30 AND name LIKE 'A%'";

export function FilterBar({
  columns,
  rows,
  conjunction,
  dirty,
  onChange,
  onApply,
  onClear,
}: {
  columns: ColumnMeta[];
  rows: FilterRowSpec[];
  conjunction: Conjunction;
  dirty: boolean;
  onChange: (rows: FilterRowSpec[], conjunction: Conjunction) => void;
  onApply: () => void;
  onClear: () => void;
}) {
  const update = (id: string, patch: Partial<FilterRowSpec>) =>
    onChange(
      rows.map((r) => (r.id === id ? { ...r, ...patch } : r)),
      conjunction,
    );
  const add = () => onChange([...rows, newFilterRow(columns[0]?.name ?? "")], conjunction);
  const remove = (id: string) =>
    onChange(
      rows.filter((r) => r.id !== id),
      conjunction,
    );
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      onApply();
    }
  };
  return (
    <div
      className="border-border bg-surface-2 flex shrink-0 flex-col gap-1 border-b px-2 py-1.5"
      data-scope="global"
    >
      {rows.map((row, i) => {
        const def = opDef(row.op);
        const raw = isRawSqlRow(row);
        return (
          <div key={row.id} className="flex items-center gap-1.5">
            <div className="w-12 shrink-0 text-right">
              {i === 0 ? (
                <span className="text-muted-foreground text-[11px]">WHERE</span>
              ) : (
                <button
                  type="button"
                  className="text-brand hover:bg-brand-soft rounded px-1 font-mono text-[11px]"
                  title="Toggle AND / OR for all rows"
                  onClick={() => onChange(rows, conjunction === "and" ? "or" : "and")}
                >
                  {conjunction.toUpperCase()}
                </button>
              )}
            </div>
            <Checkbox
              checked={row.enabled}
              onCheckedChange={(v) => update(row.id, { enabled: v === true })}
              aria-label="Enable filter"
            />
            <Select value={row.column} onValueChange={(v) => update(row.id, { column: v })}>
              <SelectTrigger
                size="sm"
                aria-label="Filter column"
                className="bg-surface h-7 w-44 font-mono text-xs"
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
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                className="bg-surface h-7 min-w-0 flex-1 font-mono text-xs"
                autoFocus={i === rows.length - 1 && row.value === ""}
              />
            ) : (
              <Select value={row.op} onValueChange={(v) => update(row.id, { op: v as FilterOp })}>
                <SelectTrigger size="sm" className="bg-surface h-7 w-28 font-mono text-xs">
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
                placeholder={
                  def.arity === "list"
                    ? "a, b, c"
                    : def.op === "like" || def.op === "notLike"
                      ? "%pattern%"
                      : "value"
                }
                className={cn(
                  "bg-surface h-7 font-mono text-xs",
                  def.arity === 2 ? "w-40" : "w-64",
                )}
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
                  className="bg-surface h-7 w-40 font-mono text-xs"
                />
              </>
            )}
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Remove filter"
              onClick={() => remove(row.id)}
            >
              <X />
            </Button>
          </div>
        );
      })}
      <div className="flex items-center gap-1.5 pl-12">
        <Button variant="ghost" size="xs" onClick={add}>
          <Plus /> Add condition
        </Button>
        <span className="flex-1" />
        <Button variant="ghost" size="xs" onClick={onClear} disabled={rows.length === 0}>
          Clear
        </Button>
        <Button size="xs" onClick={onApply} variant={dirty ? "default" : "outline"}>
          Apply
          <kbd className="ml-1 text-[10px] opacity-70">↩</kbd>
        </Button>
      </div>
    </div>
  );
}
