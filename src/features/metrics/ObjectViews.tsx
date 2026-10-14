import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, Loader2, Minus } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { MetricsResult } from "@/lib/ipc";
import { cellDisplay, displayValue, isNumericKind } from "@/lib/values";
import { cn } from "@/lib/utils";
import { inputValue, type BoardObject } from "./model";
import {
  dropdownOptions,
  formatChange,
  formatScore,
  scoreNumber,
  scoreValue,
  valueKey,
} from "./results";
import { compareValues } from "./sort";
import { setInputText, setVariable, useMetricsStore, runKey } from "./store";

/**
 * A scoreboard (stat tile): the label, one number (grouped, compact from 10,000), and its change
 * since the previous refresh, coloured by whether that change is good news (with an arrow and a
 * sign, never by colour alone).
 */
export function ScoreboardView({
  object,
  result,
  previous,
}: {
  object: BoardObject;
  result: MetricsResult | null;
  previous: number | null;
}) {
  const value = scoreValue(result, object.valueColumn);
  const n = scoreNumber(result, object.valueColumn);
  const text = n !== null ? formatScore(n) : value ? displayValue(value).text : "—";
  const diff = n !== null && previous !== null ? n - previous : null;
  const good = diff !== null && diff !== 0 && diff > 0 === object.higherIsBetter;
  return (
    <div className="@container flex h-full min-h-0 flex-col justify-center gap-1 px-3 py-2">
      <p className="text-muted-foreground truncate text-[12px] font-medium">{object.title}</p>
      <p
        className="truncate text-2xl leading-tight font-semibold @[12rem]:text-3xl @[18rem]:text-4xl"
        title={value ? displayValue(value).text : undefined}
      >
        {text}
      </p>
      {diff !== null && previous !== null && n !== null && (
        <p
          className={cn(
            "flex min-w-0 items-center gap-1 text-[11px]",
            diff === 0 ? "text-muted-foreground" : good ? "text-success" : "text-error",
          )}
        >
          {diff > 0 ? (
            <ArrowUp className="size-3 shrink-0" aria-label="Up" />
          ) : diff < 0 ? (
            <ArrowDown className="size-3 shrink-0" aria-label="Down" />
          ) : (
            <Minus className="size-3 shrink-0" aria-label="No change" />
          )}
          <span className="truncate tabular-nums">{formatChange(n, previous)}</span>
          <span className="text-muted-foreground truncate">vs previous refresh</span>
        </p>
      )}
    </div>
  );
}

const ROW_H = 24;

type Sort = { col: number; desc: boolean } | null;

/** A data table: the result's rows, read-only; a header click sorts (asc, desc, off). */
export function DataTableView({ result }: { result: MetricsResult | null }) {
  const [sort, setSort] = useState<Sort>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => {
    const list = result?.rows ?? [];
    if (!sort) return list;
    return [...list].sort((a, b) => compareValues(a[sort.col], b[sort.col], sort.desc));
  }, [result, sort]);
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW_H,
    overscan: 12,
  });
  if (!result) return null;
  const columns = result.columns;
  const numeric = columns.map((c) => isNumericKind(c.kind));
  const next = (col: number): Sort =>
    sort?.col !== col ? { col, desc: false } : sort.desc ? null : { col, desc: true };
  const template = `repeat(${Math.max(1, columns.length)}, minmax(6rem, 1fr))`;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div ref={scroller} className="min-h-0 flex-1 overflow-auto text-xs">
        <div className="min-w-fit">
          <div
            role="row"
            className="bg-surface-2 text-muted-foreground sticky top-0 z-10 grid h-6 shadow-[inset_0_-1px_0_var(--color-border)]"
            style={{ gridTemplateColumns: template }}
          >
            {columns.map((c, i) => (
              <button
                key={i}
                type="button"
                role="columnheader"
                aria-sort={sort?.col === i ? (sort.desc ? "descending" : "ascending") : "none"}
                onClick={() => setSort(next(i))}
                className={cn(
                  "hover:text-foreground flex min-w-0 items-center gap-1 px-2 text-left text-[11px] font-medium",
                  numeric[i] && "justify-end",
                  sort?.col === i && "text-foreground",
                )}
              >
                <span className="truncate font-mono">{c.name}</span>
                {sort?.col === i &&
                  (sort.desc ? (
                    <ArrowDown className="size-3 shrink-0" />
                  ) : (
                    <ArrowUp className="size-3 shrink-0" />
                  ))}
              </button>
            ))}
          </div>
          <div className="relative" style={{ height: virtual.getTotalSize() }}>
            {virtual.getVirtualItems().map((item) => {
              const row = rows[item.index] ?? [];
              return (
                <div
                  key={item.key}
                  role="row"
                  className="border-gridline absolute inset-x-0 grid border-b hover:bg-black/[0.035] dark:hover:bg-white/[0.05]"
                  style={{
                    gridTemplateColumns: template,
                    height: ROW_H,
                    transform: `translateY(${item.start}px)`,
                  }}
                >
                  {columns.map((_, i) => {
                    const v = row[i];
                    const d = v ? cellDisplay(v) : null;
                    return (
                      <span
                        key={i}
                        role="cell"
                        className={cn(
                          "truncate px-2 leading-6",
                          numeric[i] && "text-right font-mono tabular-nums",
                          d?.placeholder && "text-muted-foreground italic",
                        )}
                        title={d?.text}
                      >
                        {d?.text}
                      </span>
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </div>
      <p className="text-muted-foreground border-border shrink-0 border-t px-2 py-0.5 text-[10px]">
        {rows.length.toLocaleString("en-US")} row{rows.length === 1 ? "" : "s"}
        {result.truncated && " (first rows only)"}
      </p>
    </div>
  );
}

/** Debounce of a typed input before its variable changes (ms). */
const TYPE_DELAY = 400;

/** An input field (text / number / date) that sets its variable and fires its event. */
export function InputView({
  boardId,
  object,
  disabled,
  onChange,
}: {
  boardId: string;
  object: BoardObject;
  disabled: boolean;
  onChange: () => void;
}) {
  const stored = useMetricsStore((s) => s.texts[runKey(boardId, object.id)]);
  const text = stored ?? object.defaultValue;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const commit = (value: string) => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (!object.variable) return;
    setVariable(boardId, object.variable, inputValue(object.inputType, value));
    onChange();
  };
  return (
    <label className="flex h-full min-h-0 flex-col justify-center gap-1 px-3">
      <span className="text-muted-foreground truncate text-[11px] font-medium">{object.title}</span>
      <Input
        type={
          object.inputType === "number" ? "number" : object.inputType === "date" ? "date" : "text"
        }
        value={text}
        disabled={disabled}
        placeholder={object.variable ? `{{${object.variable}}}` : undefined}
        className="h-7 text-xs md:text-xs"
        onChange={(e) => {
          const value = e.target.value;
          setInputText(boardId, object.id, value);
          if (object.inputType === "date") return commit(value);
          if (timer.current) clearTimeout(timer.current);
          timer.current = setTimeout(() => commit(value), TYPE_DELAY);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit(e.currentTarget.value);
        }}
      />
    </label>
  );
}

/** A dropdown: its options (static or from its SQL) set its variable and fire its event. */
export function DropdownView({
  boardId,
  object,
  result,
  running,
  disabled,
  onChange,
}: {
  boardId: string;
  object: BoardObject;
  result: MetricsResult | null;
  running: boolean;
  disabled: boolean;
  onChange: () => void;
}) {
  const current = useMetricsStore((s) =>
    object.variable ? s.vars[boardId]?.[object.variable] : undefined,
  );
  const options = useMemo(() => dropdownOptions(object, result), [object, result]);
  const byKey = useMemo(() => new Map(options.map((o) => [valueKey(o.value), o])), [options]);
  const selected = current ? valueKey(current) : undefined;
  return (
    <div className="flex h-full min-h-0 flex-col justify-center gap-1 px-3">
      <span className="text-muted-foreground flex items-center gap-1 truncate text-[11px] font-medium">
        {object.title}
        {running && <Loader2 className="size-3 animate-spin" />}
      </span>
      <Select
        value={selected !== undefined && byKey.has(selected) ? selected : ""}
        disabled={disabled || options.length === 0}
        onValueChange={(key) => {
          const option = byKey.get(key);
          if (!option || !object.variable) return;
          setVariable(boardId, object.variable, option.value);
          onChange();
        }}
      >
        <SelectTrigger size="sm" className="w-full text-xs" aria-label={object.title}>
          <SelectValue placeholder={options.length === 0 ? "No options" : "Choose…"} />
        </SelectTrigger>
        <SelectContent>
          {options.map((o) => {
            const key = valueKey(o.value);
            return (
              <SelectItem key={key} value={key} className="text-xs">
                {o.label}
              </SelectItem>
            );
          })}
        </SelectContent>
      </Select>
    </div>
  );
}

/** A button: runs its SQL (if any) and refreshes the objects wired to it. */
export function ButtonView({
  object,
  running,
  disabled,
  onClick,
}: {
  object: BoardObject;
  running: boolean;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <div className="flex h-full items-center justify-center p-1.5">
      <Button
        size="sm"
        className="h-full max-h-9 w-full text-xs"
        disabled={disabled || running}
        onClick={onClick}
      >
        {running && <Loader2 className="animate-spin" />}
        <span className="truncate">{object.title}</span>
      </Button>
    </div>
  );
}
