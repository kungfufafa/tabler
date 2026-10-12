import { useEffect, useMemo, useRef, useState } from "react";
import type { Value } from "@/lib/ipc";
import type { GridColumn } from "../types";
import { choiceKeyOf, choicesFor, type EditorExit } from "./registry";
import { useGrabFocus } from "./useGrabFocus";

/**
 * bool / enum picker (a native select). Choosing with the mouse commits at once; with the keyboard
 * the choice is confirmed with ↩ or ⇥ (arrow keys would otherwise commit on the first press).
 */
export function ChoiceSelect({
  col,
  previous,
  typed,
  onPick,
  onCancel,
}: {
  col: GridColumn;
  previous: Value;
  /** The character that opened the editor, to preselect the first matching option. */
  typed?: string;
  /** `key` is a choice key (`NULL_CHOICE` for NULL). */
  onPick: (key: string, exit: EditorExit) => void;
  onCancel: () => void;
}) {
  const choices = useMemo(() => choicesFor(col), [col]);
  const initial = useMemo(() => {
    const t = typed?.toLowerCase();
    if (t) {
      const hit = choices.find((c) => c.label.toLowerCase().startsWith(t));
      if (hit) return hit.key;
    }
    return choiceKeyOf(previous, choices);
  }, [choices, previous, typed]);
  const [value, setValue] = useState(initial ?? "");
  const keyboard = useRef(false);
  const ref = useRef<HTMLSelectElement>(null);
  useGrabFocus(ref);
  useEffect(() => {
    // Open the option list right away when the browser allows it (needs a recent user gesture).
    if (typed) return;
    const el = ref.current as (HTMLSelectElement & { showPicker?: () => void }) | null;
    try {
      el?.showPicker?.();
    } catch {
      // Not allowed here; the select is focused and opens with ↓ or a click.
    }
  }, [typed]);

  const pick = (key: string, exit: EditorExit) => (key ? onPick(key, exit) : onCancel());

  return (
    <select
      ref={ref}
      value={value}
      aria-label={`Edit ${col.name}`}
      onPointerDown={() => {
        keyboard.current = false;
      }}
      onChange={(e) => {
        setValue(e.target.value);
        if (!keyboard.current) pick(e.target.value, "enter");
      }}
      onKeyDown={(e) => {
        e.stopPropagation();
        keyboard.current = true;
        if (e.key === "Enter") {
          e.preventDefault();
          pick(value, "enter");
        } else if (e.key === "Tab") {
          e.preventDefault();
          pick(value, e.shiftKey ? "shiftTab" : "tab");
        } else if (e.key === "Escape") {
          e.preventDefault();
          onCancel();
        }
      }}
      onBlur={onCancel}
      className="border-brand bg-surface h-full w-full border-2 px-1.5 text-[12px] outline-none"
    >
      {initial === null && (
        <option value="" disabled>
          {previous.t === "default" ? "DEFAULT" : "—"}
        </option>
      )}
      {choices.map((c) => (
        <option key={c.key} value={c.key}>
          {c.label}
        </option>
      ))}
    </select>
  );
}
