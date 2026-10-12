import { Lock, RotateCcw, Search, TriangleAlert } from "lucide-react";
import { memo, useEffect, useMemo, useState, type KeyboardEvent, type Ref } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { comboFromEvent, formatCombo, isMac } from "@/lib/keys/combo";
import { DEFAULT_KEYMAP, KEYMAP, useKeymapVersion } from "@/lib/keys/keymap";
import {
  captureResult,
  comboProblem,
  EMPTY_OVERRIDES,
  resetBindings,
  setBinding,
  type KeymapOverrides,
} from "@/lib/keys/overrides";
import { beginShortcutCapture } from "@/lib/keys/useShortcuts";
import { cn } from "@/lib/utils";
import { getPrefs, setPref, usePrefs } from "@/stores/prefs";
import { SectionShell, SettingGroup } from "./controls";
import { keymapRows, rowConflicts, rowKey, type KeymapRow, type KeymapSlot } from "./keymapRows";
import {
  groupShortcuts,
  haystack,
  isReference,
  queryTerms,
  SCOPE_ORDER,
  SCOPE_TITLES,
  type ShortcutRow,
} from "./shortcuts";

const LEGEND = isMac ? "⌘ Command  ·  ⌥ Option  ·  ⇧ Shift  ·  ⌃ Control" : null;

const PROBLEM_TEXT = {
  reserved: (combo: string) => `${formatCombo(combo)} is reserved and can't be assigned.`,
  invalid: (combo: string) => `${formatCombo(combo)} can't be used as a shortcut.`,
  "needs-modifier": () => `Include ${isMac ? "⌘, ⌃ or ⌥" : "Ctrl or Alt"}: plain keys type text.`,
} as const;

interface Group {
  scope: KeymapRow["scope"];
  title: string;
  rows: KeymapRow[];
  /** Keys the app handles itself (read-only). */
  refs: ShortcutRow[];
}

function buildGroups(rows: readonly KeymapRow[], query: string): Group[] {
  const terms = queryTerms(query);
  const refsByScope = new Map(
    groupShortcuts(KEYMAP, query).map((g) => [g.scope, g.rows.filter(isReference)]),
  );
  const matches = (row: KeymapRow) => {
    if (terms.length === 0) return true;
    const keys = row.slots.map((s) => s.keys).filter(Boolean);
    const text = haystack({ id: row.id, label: row.label, keys }, row.scope, isMac);
    return terms.every((t) => text.includes(t));
  };
  return SCOPE_ORDER.flatMap((scope) => {
    const own = rows.filter((r) => r.scope === scope && matches(r));
    const refs = refsByScope.get(scope) ?? [];
    return own.length + refs.length > 0
      ? [{ scope, title: SCOPE_TITLES[scope], rows: own, refs }]
      : [];
  });
}

interface Recording {
  row: string;
  slot: number;
}

function SlotKey({
  row,
  slot,
  recording,
  onRecord,
  onKey,
  onCancel,
}: {
  row: KeymapRow;
  slot: KeymapSlot | null;
  recording: boolean;
  onRecord: () => void;
  onKey: (e: KeyboardEvent<HTMLButtonElement>) => void;
  onCancel: () => void;
}) {
  if (slot?.fixed) {
    return (
      <span
        title={`${formatCombo(slot.keys)} can't be changed`}
        className="text-muted-foreground inline-flex items-center gap-1"
      >
        <Lock className="size-2.5" aria-hidden />
        <Kbd combo={slot.keys} className="bg-surface-2 text-foreground/60 h-5 min-w-5 px-1.5" />
      </span>
    );
  }
  const keys = slot?.keys ?? "";
  return (
    <button
      type="button"
      aria-label={
        recording
          ? `${row.label}: type a shortcut`
          : `${row.label}: ${keys ? formatCombo(keys) : "no shortcut"}. Change shortcut`
      }
      aria-pressed={recording}
      onClick={recording ? undefined : onRecord}
      onKeyDown={recording ? onKey : undefined}
      onBlur={recording ? onCancel : undefined}
      ref={(el) => {
        if (recording) el?.focus();
      }}
      className={cn(
        "focus-visible:ring-ring/50 rounded outline-none focus-visible:ring-2",
        recording && "ring-brand ring-2",
      )}
    >
      {recording ? (
        <span className="bg-brand-soft text-brand inline-flex h-5 items-center rounded px-2 text-[11px]">
          Type a shortcut…
        </span>
      ) : keys ? (
        <Kbd
          combo={keys}
          className="bg-surface-2 text-foreground/80 hover:border-foreground/30 h-5 min-w-5 cursor-pointer px-1.5 text-[11px]"
        />
      ) : (
        <span className="border-border text-muted-foreground hover:text-foreground inline-flex h-5 items-center rounded border border-dashed px-2 text-[11px]">
          None
        </span>
      )}
    </button>
  );
}

interface RowActions {
  record: (row: KeymapRow, slot: number) => void;
  key: (row: KeymapRow, slot: KeymapSlot, e: KeyboardEvent<HTMLButtonElement>) => void;
  stop: () => void;
  reset: (row: KeymapRow) => void;
}

/** One editable row; memoised so recording in one row does not re-render the whole list. */
const KeymapRowView = memo(function KeymapRowView({
  row,
  recordingSlot,
  message,
  conflicts,
  actions,
}: {
  row: KeymapRow;
  /** The slot being recorded in this row, if any. */
  recordingSlot: number | null;
  message: string | null;
  /** Labels of the other actions sharing a combo, already joined. */
  conflicts: string;
  actions: RowActions;
}) {
  const shown = row.slots.filter((s) => s.keys !== "" || recordingSlot === s.slot);
  const empty = row.slots.find((s) => s.keys === "");
  return (
    <div
      data-row={row.id}
      className="flex min-h-7 flex-col justify-center gap-0.5 px-3 py-1 text-[13px]"
    >
      <div className="flex items-center gap-3">
        <span className="min-w-0 flex-1 truncate">{row.label}</span>
        {row.modified && (
          <button
            type="button"
            aria-label={`Reset ${row.label}`}
            title="Reset to the default"
            onClick={() => actions.reset(row)}
            className="text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 inline-flex items-center gap-1 rounded px-1 text-[11px] outline-none focus-visible:ring-2"
          >
            <RotateCcw className="size-3" aria-hidden />
            Reset
          </button>
        )}
        <span className="flex shrink-0 items-center gap-1">
          {shown.map((s) => (
            <SlotKey
              key={s.slot}
              row={row}
              slot={s}
              recording={recordingSlot === s.slot}
              onRecord={() => actions.record(row, s.slot)}
              onKey={(e) => actions.key(row, s, e)}
              onCancel={actions.stop}
            />
          ))}
          {shown.length === 0 && empty && (
            <SlotKey
              row={row}
              slot={null}
              recording={false}
              onRecord={() => actions.record(row, empty.slot)}
              onKey={() => undefined}
              onCancel={actions.stop}
            />
          )}
        </span>
      </div>
      {(message || conflicts) && (
        <p className="text-warning flex items-center gap-1 text-[11px] leading-4">
          <TriangleAlert className="size-3 shrink-0" aria-hidden />
          {message ?? `Also used by ${conflicts}.`}
        </p>
      )}
    </div>
  );
});

/** A key the app handles itself (read-only). */
const ReferenceRowView = memo(function ReferenceRowView({ row }: { row: ShortcutRow }) {
  return (
    <div className="flex h-7 items-center gap-3 px-3 text-[13px]">
      <span className="text-foreground/80 min-w-0 flex-1 truncate">{row.label}</span>
      <span className="flex shrink-0 items-center gap-1">
        {row.keys.map((k) => (
          <Kbd
            key={k}
            combo={k}
            className="bg-surface-2 text-foreground/60 h-5 min-w-5 px-1.5 text-[11px]"
          />
        ))}
      </span>
    </div>
  );
});

/**
 * Settings ▸ Shortcuts (TablePlus Preferences ▸ Keymap): click a shortcut and type the new one
 * (⎋ cancels, ⌫ removes it); "Reset" puts a row back, "Reset All" every row. A combo another
 * action of the same scope already uses is flagged. ⌘R and ⌘0 cannot be changed. Keys the app
 * handles itself (grid navigation, cell editors, CodeMirror) are listed read-only.
 */
export function ShortcutsSection({
  query,
  onQueryChange,
  searchRef,
}: {
  query: string;
  onQueryChange: (query: string) => void;
  searchRef?: Ref<HTMLInputElement>;
}) {
  // KEYMAP changes in place: re-render (and regroup) whenever it does.
  useKeymapVersion();
  const overrides = usePrefs((p) => p.keymapOverrides);
  const [recording, setRecording] = useState<Recording | null>(null);
  const [problem, setProblem] = useState<{ row: string; message: string } | null>(null);
  const rows = useMemo(() => keymapRows(DEFAULT_KEYMAP, overrides), [overrides]);
  const groups = buildGroups(rows, query);
  const modified = Object.keys(overrides).length > 0;

  // While recording, keys reach the recorder instead of running their actions.
  useEffect(() => (recording ? beginShortcutCapture() : undefined), [recording]);

  // Stable, so the memoised rows only re-render when their own props change. They read the
  // overrides at call time.
  const actions = useMemo<RowActions>(() => {
    const stop = () => setRecording(null);
    const update = (fn: (o: KeymapOverrides) => KeymapOverrides) =>
      setPref("keymapOverrides", fn(getPrefs().keymapOverrides));
    return {
      stop,
      record: (row, slot) => {
        setProblem(null);
        setRecording({ row: rowKey(row.id, row.label), slot });
      },
      reset: (row) => {
        setProblem(null);
        update((o) =>
          resetBindings(
            DEFAULT_KEYMAP,
            o,
            row.id,
            row.slots.map((s) => s.slot),
          ),
        );
      },
      key: (row, slot, e) => {
        // Nothing else may act on a key being recorded (the dialog's ⌘1…4 / ⌘F, the native menu).
        e.preventDefault();
        e.stopPropagation();
        const result = captureResult(e.nativeEvent, comboFromEvent(e.nativeEvent));
        if (result.type === "pending") return;
        if (result.type === "cancel") return stop();
        const next = result.type === "clear" ? "" : result.combo;
        if (next !== "") {
          const why = comboProblem(next, { id: row.id, keys: slot.defaultKeys });
          if (why) {
            setProblem({ row: rowKey(row.id, row.label), message: PROBLEM_TEXT[why](next) });
            return;
          }
        }
        setProblem(null);
        update((o) => setBinding(DEFAULT_KEYMAP, o, row.id, slot.slot, next));
        stop();
      },
    };
  }, []);

  return (
    <SectionShell
      title="Shortcuts"
      actions={
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
            <Input
              ref={searchRef}
              type="search"
              value={query}
              onChange={(e) => onQueryChange(e.target.value)}
              placeholder="Search shortcuts"
              aria-label="Search shortcuts"
              spellCheck={false}
              autoComplete="off"
              className="h-7 w-52 rounded-md pl-7 text-xs md:text-xs [&::-webkit-search-cancel-button]:hidden"
            />
          </div>
          <Button
            variant="outline"
            size="xs"
            disabled={!modified}
            onClick={() => {
              setProblem(null);
              setPref("keymapOverrides", EMPTY_OVERRIDES);
            }}
          >
            Reset All
          </Button>
        </div>
      }
    >
      {groups.length === 0 ? (
        <p className="text-muted-foreground py-10 text-center text-xs">
          No shortcuts match “{query.trim()}”.
        </p>
      ) : (
        groups.map((g) => (
          <SettingGroup key={g.scope} title={g.title}>
            {g.rows.map((row) => {
              const key = rowKey(row.id, row.label);
              return (
                <KeymapRowView
                  key={key}
                  row={row}
                  recordingSlot={recording?.row === key ? recording.slot : null}
                  message={problem?.row === key ? problem.message : null}
                  conflicts={rowConflicts(row, KEYMAP)
                    .map((l) => `“${l}”`)
                    .join(", ")}
                  actions={actions}
                />
              );
            })}
            {g.refs.map((row) => (
              <ReferenceRowView key={`${row.id}:${row.label}`} row={row} />
            ))}
          </SettingGroup>
        ))
      )}
      <p className="text-muted-foreground mt-auto px-1 text-[11px] leading-4">
        Click a shortcut and type the new one: {formatCombo("Escape")} cancels,{" "}
        {formatCombo("Backspace")} removes it. The native menu follows your changes.
        {LEGEND && <span className="block whitespace-pre">{LEGEND}</span>}
      </p>
    </SectionShell>
  );
}
