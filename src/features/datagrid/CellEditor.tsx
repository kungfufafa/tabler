import { useEffect, useRef, useState } from "react";
import type { Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { parseEditText } from "@/lib/values";
import type { GridColumn } from "./types";

export type EditorExit = "enter" | "tab" | "shiftTab" | "blur";

export interface CellEditorProps {
  col: GridColumn;
  initialText: string;
  previous: Value;
  left: number;
  top: number;
  width: number;
  height: number;
  onCommit: (value: Value, exit: EditorExit) => void;
  onCancel: () => void;
}

/** Inline editor mounted over the focused cell. Validates per column kind; Enter commits, Esc cancels. */
export function CellEditor({
  col,
  initialText,
  previous,
  left,
  top,
  width,
  height,
  onCommit,
  onCancel,
}: CellEditorProps) {
  const [text, setText] = useState(initialText);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLInputElement>(null);
  const committed = useRef(false);

  useEffect(() => {
    const el = ref.current;
    const grab = () => {
      if (!el || document.activeElement === el) return;
      el.focus({ preventScroll: true });
      if ("select" in el && typeof el.select === "function") el.select();
    };
    grab();
    // A double-click's trailing click can refocus the grid after we mount; grab focus again next frame.
    const id = requestAnimationFrame(grab);
    const id2 = window.setTimeout(grab, 50);
    return () => {
      cancelAnimationFrame(id);
      window.clearTimeout(id2);
    };
  }, []);

  const commit = (exit: EditorExit) => {
    if (committed.current) return;
    const result = parseEditText(col.kind, text, previous);
    if (!result.ok) {
      setError(result.error);
      if (exit === "blur") onCancel();
      return;
    }
    committed.current = true;
    onCommit(result.value, exit);
  };

  const isBool = col.kind === "bool";
  const isEnum = col.kind === "enum" && (col.enumValues?.length ?? 0) > 0;
  const options = isBool ? ["true", "false"] : (col.enumValues ?? []);

  return (
    <div
      data-scope="grid-edit"
      className="absolute z-[6]"
      style={{ left, top, width: Math.max(width, 120), height }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      {isBool || isEnum ? (
        <select
          ref={ref as unknown as React.RefObject<HTMLSelectElement>}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            const result = parseEditText(col.kind, e.target.value, previous);
            if (result.ok) {
              committed.current = true;
              onCommit(result.value, "enter");
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") onCancel();
          }}
          onBlur={onCancel}
          className="border-brand bg-surface h-full w-full border-2 px-1.5 font-mono text-[12.5px] outline-none"
        >
          {!options.includes(text) && <option value={text}>{text || "—"}</option>}
          {options.map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
      ) : (
        <input
          ref={ref}
          value={text}
          title={error ?? undefined}
          onChange={(e) => {
            setText(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit("enter");
            } else if (e.key === "Tab") {
              e.preventDefault();
              commit(e.shiftKey ? "shiftTab" : "tab");
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
            }
            e.stopPropagation();
          }}
          onBlur={() => commit("blur")}
          spellCheck={false}
          className={cn(
            "bg-surface h-full w-full border-2 px-1.5 text-[12.5px] outline-none",
            col.align === "right" ? "text-right font-mono" : "",
            error ? "border-error" : "border-brand",
          )}
        />
      )}
      {error && (
        <div className="bg-error absolute top-full left-0 z-10 mt-0.5 rounded px-1.5 py-0.5 text-[11px] whitespace-nowrap text-white shadow">
          {error}
        </div>
      )}
    </div>
  );
}
