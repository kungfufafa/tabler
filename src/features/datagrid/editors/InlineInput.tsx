import { format, isValid, parseISO } from "date-fns";
import { CalendarDays } from "lucide-react";
import { useRef, useState, type RefObject } from "react";
import { Calendar } from "@/components/ui/calendar";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { mergeDatePart, splitDatePart } from "@/lib/values";
import type { GridColumn } from "../types";
import type { EditorExit } from "./registry";
import { useGrabFocus, type CaretPlacement } from "./useGrabFocus";

/** Single-line inline input (text, numbers, typed JSON) — with a calendar button for dates. */
export function InlineInput({
  col,
  text,
  error,
  caret,
  calendar,
  onTextChange,
  onCommit,
  onCancel,
  onAltEnter,
}: {
  col: GridColumn;
  text: string;
  error: string | null;
  caret: CaretPlacement;
  /** Shows the date picker (date / timestamp columns). */
  calendar?: boolean;
  onTextChange: (text: string) => void;
  onCommit: (exit: EditorExit) => void;
  onCancel: () => void;
  /** ⌥↩ — switch to a richer editor; receives the selection so a newline can go at the caret. */
  onAltEnter?: (selStart: number, selEnd: number) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useGrabFocus(ref, caret);
  // While the calendar popover holds focus, the input's blur must not commit.
  const pickerOpen = useRef(false);

  return (
    <>
      <input
        ref={ref}
        value={text}
        title={error ?? undefined}
        aria-label={`Edit ${col.name}`}
        aria-invalid={error ? true : undefined}
        onChange={(e) => onTextChange(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.nativeEvent.isComposing) return;
          if (e.key === "Enter" && e.altKey) {
            e.preventDefault();
            const el = e.currentTarget;
            onAltEnter?.(el.selectionStart ?? text.length, el.selectionEnd ?? text.length);
          } else if (e.key === "Enter") {
            e.preventDefault();
            onCommit("enter");
          } else if (e.key === "Tab") {
            e.preventDefault();
            onCommit(e.shiftKey ? "shiftTab" : "tab");
          } else if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
        }}
        onBlur={() => {
          if (!pickerOpen.current) onCommit("blur");
        }}
        spellCheck={false}
        autoComplete="off"
        className={cn(
          "bg-surface h-full w-full border-2 px-1.5 text-[12.5px] outline-none",
          col.align === "right" && "text-right font-mono tabular-nums",
          calendar && "pr-6 font-mono",
          error ? "border-error" : "border-brand",
        )}
      />
      {calendar && (
        <DatePicker
          text={text}
          inputRef={ref}
          openRef={pickerOpen}
          onPick={(date) => onTextChange(mergeDatePart(text, date))}
        />
      )}
    </>
  );
}

function DatePicker({
  text,
  inputRef,
  openRef,
  onPick,
}: {
  text: string;
  inputRef: RefObject<HTMLInputElement | null>;
  openRef: { current: boolean };
  onPick: (yyyyMmDd: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const { date } = splitDatePart(text);
  const parsed = date ? parseISO(date) : null;
  const selected = parsed && isValid(parsed) ? parsed : undefined;
  const setBoth = (o: boolean) => {
    if (o) openRef.current = true;
    setOpen(o);
  };
  return (
    <Popover open={open} onOpenChange={setBoth}>
      <PopoverTrigger asChild>
        <button
          type="button"
          tabIndex={-1}
          aria-label="Pick a date"
          title="Pick a date"
          // Keep focus in the input while clicking (its blur would commit).
          onMouseDown={(e) => e.preventDefault()}
          className="text-muted-foreground hover:text-foreground hover:bg-accent absolute top-1/2 right-1 flex size-5 -translate-y-1/2 items-center justify-center rounded"
        >
          <CalendarDays className="size-3.5" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-auto p-0"
        data-scope="grid-edit"
        onPointerDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
        onOpenAutoFocus={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          inputRef.current?.focus({ preventScroll: true });
          openRef.current = false;
        }}
      >
        <Calendar
          mode="single"
          autoFocus
          selected={selected}
          defaultMonth={selected}
          onSelect={(d) => {
            if (d) onPick(format(d, "yyyy-MM-dd"));
            setOpen(false);
          }}
        />
      </PopoverContent>
    </Popover>
  );
}
