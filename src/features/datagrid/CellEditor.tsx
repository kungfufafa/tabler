import { useRef, useState } from "react";
import type { Value } from "@/lib/ipc";
import { parseCellText } from "./cellText";
import { useLacksFields } from "./lacksFields";
import { ChoiceSelect } from "./editors/ChoiceSelect";
import { InlineInput } from "./editors/InlineInput";
import { reformatJson } from "./editors/json";
import { JsonEditorDialog } from "./editors/JsonEditorDialog";
import { MultilinePanel } from "./editors/MultilinePanel";
import { editorFor, NULL_CHOICE, type EditorExit } from "./editors/registry";
import type { CaretPlacement } from "./editors/useGrabFocus";
import { emptyBecomesNull } from "./setValue";
import type { GridColumn } from "./types";

export type { EditorExit } from "./editors/registry";

export interface CellEditorProps {
  col: GridColumn;
  initialText: string;
  previous: Value;
  /**
   * The editor was opened by typing (`initialText` is the typed character, or "" for ⌫): JSON then
   * starts inline instead of in the dialog, and the caret goes to the end instead of selecting all.
   */
  typed?: boolean;
  left: number;
  top: number;
  width: number;
  height: number;
  onCommit: (value: Value, exit: EditorExit) => void;
  onCancel: () => void;
}

type Mode = "text" | "multiline" | "json" | "choice" | "date";

/** Kinds whose values may span lines (⌥↩ in the inline input switches to the multi-line editor). */
const MULTILINE_KINDS: ReadonlySet<GridColumn["kind"]> = new Set([
  "text",
  "enum",
  "set",
  "array",
  "other",
  "geometry",
]);

function initialMode(col: GridColumn, previous: Value, text: string, typed: boolean): Mode {
  const kind = editorFor(col, previous, text);
  if (kind === null) return "text";
  if (kind === "json" && typed) return "text";
  return kind;
}

/** Pretty JSON for the dialog when it opens on a stored value (whitespace only; numbers stay exact). */
function jsonDialogText(text: string): string {
  try {
    JSON.parse(text);
    return reformatJson(text, 2);
  } catch {
    return text;
  }
}

/**
 * Inline editor mounted over the focused cell. Picks an editor per column kind (see `editorFor`):
 * single-line input, multi-line panel, JSON dialog, bool/enum picker, or date input with a calendar.
 * ↩ commits and moves down, ⇥/⇧⇥ commit and move sideways, ⎋ cancels, blur commits.
 */
export function CellEditor({
  col,
  initialText,
  previous,
  typed = false,
  left,
  top,
  width,
  height,
  onCommit,
  onCancel,
}: CellEditorProps) {
  const [mode, setMode] = useState<Mode>(() => initialMode(col, previous, initialText, typed));
  const [text, setText] = useState(() =>
    mode === "json" && previous.t === "json" ? jsonDialogText(initialText) : initialText,
  );
  // The dialog's opening text: saving it unchanged keeps the stored value byte-for-byte.
  const [jsonBase, setJsonBase] = useState<string | null>(() => (mode === "json" ? text : null));
  const [caret, setCaret] = useState<CaretPlacement>(typed ? "end" : "all");
  const [error, setError] = useState<string | null>(null);
  const done = useRef(false);
  const lacksFields = useLacksFields();
  const anchorRef = useRef<HTMLDivElement>(null);

  const commitValue = (value: Value, exit: EditorExit) => {
    if (done.current) return;
    done.current = true;
    onCommit(value, exit);
  };
  const cancel = () => {
    if (done.current) return;
    done.current = true;
    onCancel();
  };
  /**
   * Stages `t` as the cell's value and leaves the editor by `exit`; false when it does not parse
   * (the error shows; a blur drops the edit, as it always did, while `keepOpen` — ⌘S — leaves the
   * editor open to fix it).
   */
  const commitText = (exit: EditorExit, t: string = text, keepOpen = false): boolean => {
    if (done.current) return false;
    // "Auto convert empty Numeric, UUID, JSON or Array to NULL".
    if (emptyBecomesNull(col, t)) {
      commitValue({ t: "null" }, exit);
      return true;
    }
    // NOW() / CURRENT_TIMESTAMP in a date or time cell, hex in a binary one.
    const result = parseCellText(col, t, previous, lacksFields);
    if (!result.ok) {
      setError(result.error);
      if (exit === "blur" && !keepOpen) cancel();
      return false;
    }
    commitValue(result.value, exit);
    return true;
  };
  /** ⌘S: stages the text where the cell is (no move), or says it cannot (the editor stays). */
  const saveText = (t: string = text) => commitText("blur", t, true);
  const changeText = (t: string) => {
    setText(t);
    setError(null);
  };

  let body: React.ReactNode;
  switch (mode) {
    case "choice":
      body = (
        <ChoiceSelect
          col={col}
          previous={previous}
          typed={typed ? initialText : undefined}
          onCancel={cancel}
          onPick={(key, exit) =>
            key === NULL_CHOICE ? commitValue({ t: "null" }, exit) : commitText(exit, key)
          }
        />
      );
      break;
    case "multiline":
      body = (
        <>
          <div className="ring-brand bg-brand-soft h-full w-full ring-2 ring-inset" />
          <MultilinePanel
            anchorRef={anchorRef}
            label={`Edit ${col.name}`}
            text={text}
            caret={caret === "all" ? "end" : caret}
            mono={col.align === "right"}
            error={error}
            onTextChange={changeText}
            onCommit={(exit) => commitText(exit)}
            onSave={() => saveText()}
            onCancel={cancel}
          />
        </>
      );
      break;
    case "json":
      body = (
        <>
          <div className="ring-brand bg-brand-soft h-full w-full ring-2 ring-inset" />
          <JsonEditorDialog
            col={col}
            initialText={text}
            onCancel={cancel}
            onSetNull={col.nullable ? () => commitValue({ t: "null" }, "enter") : undefined}
            onSave={(t) =>
              t === jsonBase && previous.t === "json"
                ? commitValue(previous, "enter")
                : commitText("enter", t)
            }
            onSaveAndCommit={(t) => {
              if (t !== jsonBase || previous.t !== "json") return saveText(t);
              commitValue(previous, "blur");
              return true;
            }}
          />
        </>
      );
      break;
    default:
      body = (
        <InlineInput
          col={col}
          text={text}
          error={error}
          caret={caret}
          calendar={mode === "date"}
          onTextChange={changeText}
          onCommit={(exit) => commitText(exit)}
          onSave={() => saveText()}
          onCancel={cancel}
          onAltEnter={(start, end) => {
            if (col.kind === "json") {
              setJsonBase(null);
              setMode("json");
            } else if (mode === "text" && MULTILINE_KINDS.has(col.kind)) {
              setText(`${text.slice(0, start)}\n${text.slice(end)}`);
              setCaret(start + 1);
              setMode("multiline");
            }
          }}
        />
      );
  }

  return (
    <div
      ref={anchorRef}
      data-scope="grid-edit"
      className="absolute z-[6]"
      style={{ left, top, width: Math.max(width, 120), height }}
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      {body}
      {error && mode !== "multiline" && mode !== "json" && (
        <div className="bg-error absolute top-full left-0 z-10 mt-0.5 rounded px-1.5 py-0.5 text-[11px] whitespace-nowrap text-white shadow">
          {error}
        </div>
      )}
    </div>
  );
}
