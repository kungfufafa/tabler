import { X } from "lucide-react";
import { useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/button";
import { AssistantPanel } from "@/features/assistant/AssistantPanel";
import { hideAssistant, showAssistant, useAssistantStore } from "@/features/assistant/store";
import { ColumnsPane } from "@/features/structure/ColumnsPane";
import { useColumnSelectionStore } from "@/features/structure/columnSelection";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import {
  cellAtColumn,
  useInspectorStore,
  type InspectedCell,
  type InspectorMode,
} from "@/stores/inspector";
import { usePrefs } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { CellView } from "./CellView";
import { DetailsView } from "./DetailsView";

/** Details | Cell, plus the AI Assistant tab (Settings ▸ AI Assistant). */
type SidebarView = InspectorMode | "assistant";

const MODES: Array<{ mode: SidebarView; label: string }> = [
  { mode: "row", label: "Details" },
  { mode: "cell", label: "Cell" },
  { mode: "assistant", label: "Assistant" },
];
const ROW_MODES = MODES.slice(0, 2);

/**
 * The right sidebar (⇧⌘I, Space in a grid), TablePlus's row detail: a Details | Cell switch and a
 * close button on top; Details lists every field of the focused row (editable on table tabs), Cell
 * shows one value in full.
 */
export function InspectorPanel() {
  const cell = useInspectorStore((s) => s.cell);
  const mode = useInspectorStore((s) => s.mode);
  const setMode = useInspectorStore((s) => s.setMode);
  const toggle = useUiStore((s) => s.toggleInspector);
  // "Show in Cell View" on a field other than the grid's focused one; dropped as soon as the grid
  // publishes another cell.
  const [cellCol, setCellCol] = useState<{ of: InspectedCell; col: number } | null>(null);
  const shown = cell && cellCol?.of === cell ? cellAtColumn(cell, cellCol.col) : cell;
  const closeKeys = shortcutFor("ui.toggle-inspector");
  const columns = useColumnSelectionStore((s) => s.selection);
  const assistantEnabled = usePrefs((p) => p.assistantEnabled);
  const assistantActive = useAssistantStore((s) => s.active) && assistantEnabled;

  const switchMode = (next: SidebarView) => {
    setCellCol(null);
    if (next === "assistant") return showAssistant();
    hideAssistant();
    setMode(next);
  };

  return (
    <aside
      aria-label="Row details"
      className="border-border bg-sidebar text-sidebar-foreground flex h-full min-w-0 flex-col border-l"
    >
      <div className="border-border flex h-7 shrink-0 items-center gap-1 border-b pr-1 pl-2">
        <ModeSwitch
          mode={assistantActive ? "assistant" : mode}
          modes={assistantEnabled ? MODES : ROW_MODES}
          onChange={switchMode}
        />
        <Button
          variant="ghost"
          size="icon-xs"
          className="size-5"
          onClick={toggle}
          aria-label="Close row details"
          title={closeKeys ? `Close (${formatCombo(closeKeys)})` : "Close"}
        >
          <X />
        </Button>
      </div>
      {assistantActive ? (
        <AssistantPanel />
      ) : columns ? (
        // Columns selected in a structure view: their attributes, edited together.
        <ColumnsPane selection={columns} />
      ) : !cell || !shown ? (
        <div className="text-muted-foreground flex flex-1 items-center justify-center p-6 text-xs">
          No row selected
        </div>
      ) : mode === "row" ? (
        <DetailsView
          cell={cell}
          onViewCell={(col) => {
            setCellCol({ of: cell, col });
            setMode("cell");
          }}
        />
      ) : (
        <CellView cell={shown} />
      )}
    </aside>
  );
}

/** Compact segmented Details | Cell (| Assistant) control; ←/→ switch. */
function ModeSwitch({
  mode,
  modes,
  onChange,
}: {
  mode: SidebarView;
  modes: typeof MODES;
  onChange: (mode: SidebarView) => void;
}) {
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const i = modes.findIndex((m) => m.mode === mode);
    const next = modes[(i + (e.key === "ArrowRight" ? 1 : modes.length - 1)) % modes.length]!;
    onChange(next.mode);
    e.currentTarget.parentElement
      ?.querySelector<HTMLButtonElement>(`[data-mode="${next.mode}"]`)
      ?.focus();
  };
  return (
    <div
      role="radiogroup"
      aria-label="Sidebar view"
      className="border-border bg-surface-2 dark:bg-background/40 flex h-5.5 min-w-0 flex-1 items-center rounded-md border p-px"
    >
      {modes.map((m) => {
        const checked = m.mode === mode;
        return (
          <button
            key={m.mode}
            type="button"
            role="radio"
            data-mode={m.mode}
            aria-checked={checked}
            tabIndex={checked ? 0 : -1}
            onClick={() => onChange(m.mode)}
            onKeyDown={onKeyDown}
            className={cn(
              "focus-visible:ring-ring/50 h-full flex-1 rounded-[5px] px-2 text-[11px] leading-none outline-none focus-visible:ring-2",
              checked
                ? "bg-surface text-foreground dark:bg-accent font-medium shadow-xs"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {m.label}
          </button>
        );
      })}
    </div>
  );
}
