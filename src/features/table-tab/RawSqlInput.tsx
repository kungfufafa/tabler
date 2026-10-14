import { useLayoutEffect, useRef, useState, type ComponentProps, type KeyboardEvent } from "react";
import { Input } from "@/components/ui/input";
import type { Dialect } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { applySuggestion, rawSqlSuggestions, type RawSqlSuggestions } from "./rawSqlCompletion";

/**
 * The filter bar's Raw SQL field (`WHERE …` text) with autocomplete: column names and SQL
 * keywords for the word being typed (`rawSqlSuggestions`). ↑ / ↓ pick, ↩ / ⇥ insert, ⎋ closes the
 * list; with no list open ↩ applies the filter (`onEnter`).
 */
export function RawSqlInput({
  value,
  onValueChange,
  onEnter,
  dialect,
  table,
  columns,
  className,
  ...props
}: Omit<ComponentProps<typeof Input>, "value" | "onChange"> & {
  value: string;
  onValueChange: (value: string) => void;
  onEnter: () => void;
  dialect: Dialect;
  table: string;
  columns: readonly string[];
}) {
  const ref = useRef<HTMLInputElement>(null);
  const [list, setList] = useState<(RawSqlSuggestions & { index: number }) | null>(null);
  const caretAfter = useRef<number | null>(null);
  useLayoutEffect(() => {
    const at = caretAfter.current;
    if (at === null) return;
    caretAfter.current = null;
    ref.current?.setSelectionRange(at, at);
  }, [value]);

  const suggest = (text: string, caret: number) => {
    const s = rawSqlSuggestions(text, caret, dialect, table, columns);
    setList(s ? { ...s, index: 0 } : null);
  };
  const accept = (label: string) => {
    const el = ref.current;
    if (!list || !el) return;
    const next = applySuggestion(value, list.from, el.selectionStart ?? value.length, label);
    caretAfter.current = next.caret;
    setList(null);
    onValueChange(next.text);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing) return;
    const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
    if (list && plain) {
      const n = list.options.length;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const step = e.key === "ArrowDown" ? 1 : -1;
        setList({ ...list, index: (list.index + step + n) % n });
        return;
      }
      if ((e.key === "Enter" || e.key === "Tab") && !e.shiftKey) {
        e.preventDefault();
        accept(list.options[list.index]!.label);
        return;
      }
      if (e.key === "Escape") {
        // Closes the list, not the filter bar.
        e.preventDefault();
        setList(null);
        return;
      }
    }
    if (e.key === "Enter" && plain && !e.shiftKey) {
      e.preventDefault();
      onEnter();
    }
  };

  return (
    <div className="relative min-w-0 flex-1">
      <Input
        {...props}
        ref={ref}
        value={value}
        role="combobox"
        aria-expanded={list !== null}
        aria-autocomplete="list"
        onChange={(e) => {
          onValueChange(e.target.value);
          suggest(e.target.value, e.target.selectionStart ?? e.target.value.length);
        }}
        onKeyDown={onKeyDown}
        onBlur={() => setList(null)}
        className={cn("w-full", className)}
      />
      {list && (
        <ul
          role="listbox"
          aria-label="Suggestions"
          className="border-border bg-popover text-popover-foreground absolute top-full left-0 z-50 mt-0.5 max-h-56 min-w-48 overflow-auto rounded-md border py-0.5 font-mono text-xs shadow-md"
        >
          {list.options.map((o, i) => (
            <li
              key={o.label}
              role="option"
              aria-selected={i === list.index}
              // Keeps the focus (and the caret) in the field.
              onMouseDown={(e) => {
                e.preventDefault();
                accept(o.label);
              }}
              className={cn(
                "flex cursor-default items-center gap-2 px-2 py-0.5",
                i === list.index && "bg-accent text-accent-foreground",
              )}
            >
              <span className="min-w-0 flex-1 truncate">{o.label}</span>
              <span className="text-muted-foreground font-sans text-[10px]">
                {columns.includes(o.label) ? "column" : (o.type ?? "")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
