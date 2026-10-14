import { AlertCircle, Loader2 } from "lucide-react";
import { useRef, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { copyChartSvg, exportChartPng } from "@/features/charts/export";
import type { Scene } from "@/features/charts/scene";
import { cn } from "@/lib/utils";
import { BoardChart } from "./BoardChart";
import { isChart, KIND_INFO, runsQuery, type BoardObject } from "./model";
import { ButtonView, DataTableView, DropdownView, InputView, ScoreboardView } from "./ObjectViews";
import { runKey, useMetricsStore } from "./store";
import type { BoardRuntime } from "./useBoardRuntime";

export interface CardHandlers {
  onMoveStart: (e: ReactPointerEvent, id: string) => void;
  onResizeStart: (e: ReactPointerEvent, id: string) => void;
  onWireStart: (e: ReactPointerEvent, id: string) => void;
  onSelect: (id: string) => void;
  onEdit: (id: string) => void;
  onDuplicate: (id: string) => void;
  onDelete: (id: string) => void;
}

function lastRun(at: number | null): string | undefined {
  return at ? `Refreshed ${new Date(at).toLocaleTimeString()}` : undefined;
}

/**
 * One object on the board: its view (chart, scoreboard, table, input, dropdown, button), the
 * error its query returned, and in edit mode the move / resize handles and, for inputs,
 * dropdowns and buttons, the green dot to wire it.
 */
export function ObjectCard({
  boardId,
  object,
  style,
  editing,
  selected,
  dragging,
  wireTarget,
  runtime,
  handlers,
}: {
  boardId: string;
  object: BoardObject;
  style: CSSProperties;
  editing: boolean;
  selected: boolean;
  dragging: boolean;
  /** A wire being drawn is over this object (and may end here). */
  wireTarget: boolean;
  runtime: BoardRuntime;
  handlers: CardHandlers;
}) {
  const run = useMetricsStore((s) => s.runs[runKey(boardId, object.id)]);
  const box = useRef<HTMLDivElement>(null);
  const scene = useRef<Scene | null>(null);
  const info = KIND_INFO[object.kind];
  const running = run?.status === "running";
  const result = run?.result ?? null;
  const chart = isChart(object.kind);
  const needsSql = info.data && object.sql.trim() === "";
  const error = run?.status === "error" && object.kind !== "button" ? run.error : null;

  let body;
  if (needsSql)
    body = (
      <div className="flex h-full flex-col">
        <p className="truncate px-3 pt-2.5 text-[12px] font-semibold">{object.title}</p>
        <p className="text-muted-foreground flex flex-1 items-center justify-center p-3 text-center text-xs">
          {editing
            ? "Add its SQL query in the settings."
            : "No SQL query yet. Edit the board to add one."}
        </p>
      </div>
    );
  else if (error && object.kind !== "input")
    body = (
      <div className="flex h-full min-h-0 flex-col">
        <p className="truncate px-3 pt-2.5 text-[12px] font-semibold">{object.title}</p>
        <div
          role="alert"
          className="text-error flex min-h-0 flex-1 items-start gap-1.5 overflow-auto p-3 text-xs"
        >
          <AlertCircle className="mt-px size-3.5 shrink-0" />
          <span className="break-words whitespace-pre-wrap">{error}</span>
        </div>
      </div>
    );
  else if (chart)
    body = <BoardChart object={object} result={result} boxRef={box} sceneRef={scene} />;
  else if (object.kind === "scoreboard")
    body = <ScoreboardView object={object} result={result} previous={run?.previous ?? null} />;
  else if (object.kind === "table")
    body = (
      <div className="flex h-full min-h-0 flex-col">
        <p className="truncate px-3 pt-2.5 pb-1.5 text-[12px] font-semibold">{object.title}</p>
        <div className="min-h-0 flex-1">
          <DataTableView result={result} />
        </div>
      </div>
    );
  else if (object.kind === "input")
    body = (
      <InputView
        boardId={boardId}
        object={object}
        disabled={editing}
        onChange={() => runtime.fire(object.id)}
      />
    );
  else if (object.kind === "dropdown")
    body = (
      <DropdownView
        boardId={boardId}
        object={object}
        result={result}
        running={running}
        disabled={editing}
        onChange={() => runtime.fire(object.id)}
      />
    );
  else
    body = (
      <ButtonView
        object={object}
        running={running}
        disabled={editing}
        onClick={() => void runtime.click(object.id)}
      />
    );

  const waiting = info.data && !needsSql && !error && result === null;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          data-board-object={object.id}
          role="group"
          aria-label={object.title}
          tabIndex={editing ? 0 : undefined}
          title={lastRun(run?.at ?? null)}
          style={style}
          onPointerDown={(e) => {
            if (!editing || e.button !== 0) return;
            handlers.onMoveStart(e, object.id);
          }}
          onFocus={() => editing && handlers.onSelect(object.id)}
          className={cn(
            "bg-surface border-border absolute flex flex-col rounded-lg border shadow-xs outline-none",
            editing && "cursor-grab touch-none select-none",
            dragging && "z-20 cursor-grabbing shadow-lg",
            selected && "ring-primary ring-2",
            wireTarget && "ring-success ring-2",
          )}
        >
          <div
            className={cn(
              "min-h-0 flex-1 overflow-hidden rounded-lg",
              editing && "pointer-events-none",
            )}
          >
            {body}
          </div>
          {(running || waiting) && info.data && (
            <span
              className="text-muted-foreground pointer-events-none absolute top-2 right-2"
              aria-label="Running"
            >
              <Loader2 className="size-3.5 animate-spin" />
            </span>
          )}
          {editing && info.source && (
            <span
              role="button"
              aria-label={`Wire ${object.title} to an object`}
              title="Drag onto an object to refresh it when this fires"
              onPointerDown={(e) => {
                e.stopPropagation();
                handlers.onWireStart(e, object.id);
              }}
              className="bg-success ring-surface absolute top-1/2 -right-1.5 size-3 -translate-y-1/2 cursor-crosshair rounded-full ring-2"
            />
          )}
          {editing && (
            <span
              aria-hidden
              onPointerDown={(e) => {
                e.stopPropagation();
                handlers.onResizeStart(e, object.id);
              }}
              className="absolute right-0 bottom-0 size-3.5 cursor-nwse-resize rounded-br-lg"
            >
              <span className="border-muted-foreground/60 absolute right-1 bottom-1 size-1.5 border-r-2 border-b-2" />
            </span>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-48">
        {runsQuery(object) && (
          <ContextMenuItem onSelect={() => runtime.refresh(object.id)}>Refresh</ContextMenuItem>
        )}
        {object.kind === "button" && !editing && (
          <ContextMenuItem onSelect={() => void runtime.click(object.id)}>Click</ContextMenuItem>
        )}
        <ContextMenuItem onSelect={() => handlers.onEdit(object.id)}>Edit…</ContextMenuItem>
        {chart && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem
              disabled={!result}
              onSelect={() => scene.current && void copyChartSvg(scene.current)}
            >
              Copy as SVG
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!result}
              onSelect={() => scene.current && void exportChartPng(scene.current)}
            >
              Export PNG…
            </ContextMenuItem>
          </>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => handlers.onDuplicate(object.id)}>
          Duplicate
        </ContextMenuItem>
        <ContextMenuItem variant="destructive" onSelect={() => handlers.onDelete(object.id)}>
          Delete
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
