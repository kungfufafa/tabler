import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import {
  columnWidth,
  GAP,
  GRID_COLUMNS,
  moveBy,
  resizeBy,
  resolveCollisions,
  rowsHeight,
  toRect,
  usedRows,
  type Layout,
  type Rect,
} from "./layout";
import { canWireTo, KIND_INFO, wire, type BoardObject } from "./model";
import { ObjectCard, type CardHandlers } from "./ObjectCard";
import { updateObjects } from "./store";
import type { BoardRuntime } from "./useBoardRuntime";

/** Pointer travel (px) below which a press on an object is a click, not a drag. */
const CLICK_SLOP = 4;
/** Rows added under the objects in edit mode, to drop things below them. */
const EDIT_SPARE_ROWS = 4;

type Gesture =
  | {
      kind: "move" | "resize";
      id: string;
      start: Layout;
      x0: number;
      y0: number;
      dx: number;
      dy: number;
      layout: Layout;
      moved: boolean;
    }
  | {
      kind: "wire";
      id: string;
      from: { x: number; y: number };
      to: { x: number; y: number };
      over: string | null;
    };

/** A wire's path: out of the source's right edge, into the target's left (or right) edge. */
function wirePath(a: { x: number; y: number }, b: { x: number; y: number }): string {
  const bend = Math.max(40, Math.abs(b.x - a.x) / 2);
  const back = b.x < a.x ? -1 : 1;
  return `M${a.x} ${a.y} C${a.x + bend} ${a.y} ${b.x - bend * back} ${b.y} ${b.x} ${b.y}`;
}

const rightMid = (r: Rect) => ({ x: r.left + r.width, y: r.top + r.height / 2 });
const leftMid = (r: Rect) => ({ x: r.left, y: r.top + r.height / 2 });

/**
 * The board canvas: objects on a 12-column grid. In edit mode objects are dragged to move them
 * and resized from their corner (both snap to the grid; an object dropped onto others pushes them
 * down), selected to edit them in the inspector, and wired by dragging the green dot of an input,
 * dropdown or button onto another object. Arrow keys move the selected object, ⇧ + arrows resize
 * it, ⌫ deletes it.
 */
export function Canvas({
  boardId,
  objects,
  editing,
  selectedId,
  runtime,
  onSelect,
  onEdit,
  onDuplicate,
  onDelete,
}: {
  boardId: string;
  objects: readonly BoardObject[];
  editing: boolean;
  selectedId: string | null;
  runtime: BoardRuntime;
  onSelect: (id: string | null) => void;
  onEdit: (id: string) => void;
  onDuplicate: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  const grid = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [gesture, setGesture] = useState<Gesture | null>(null);

  useEffect(() => {
    const el = grid.current;
    if (!el) return;
    const measure = () => setWidth(el.clientWidth);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const colW = columnWidth(width);

  /** The objects as the gesture would leave them (others pushed out of the way). */
  const shown = useMemo(() => {
    if (!gesture || gesture.kind === "wire" || !gesture.moved) return objects;
    const placed = objects.map((o) => (o.id === gesture.id ? { ...o, layout: gesture.layout } : o));
    return resolveCollisions(placed, gesture.id);
  }, [objects, gesture]);

  const local = useCallback((clientX: number, clientY: number) => {
    const r = grid.current?.getBoundingClientRect();
    return { x: clientX - (r?.left ?? 0), y: clientY - (r?.top ?? 0) };
  }, []);

  /** Follows the pointer until it is released (window listeners: the pointer may leave). */
  const track = useCallback(
    (onMove: (e: PointerEvent) => void, onUp: (e: PointerEvent) => void) => {
      const move = (e: PointerEvent) => onMove(e);
      const up = (e: PointerEvent) => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        onUp(e);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      window.addEventListener("pointercancel", up);
    },
    [],
  );

  const startLayoutGesture = useCallback(
    (kind: "move" | "resize", e: React.PointerEvent, id: string) => {
      const o = objects.find((x) => x.id === id);
      if (!o) return;
      // No text selection or native drag; the press still focuses the object (arrow keys).
      e.preventDefault();
      e.currentTarget.closest<HTMLElement>("[data-board-object]")?.focus({ preventScroll: true });
      onSelect(id);
      const g: Gesture = {
        kind,
        id,
        start: o.layout,
        x0: e.clientX,
        y0: e.clientY,
        dx: 0,
        dy: 0,
        layout: o.layout,
        moved: false,
      };
      setGesture(g);
      track(
        (ev) => {
          const dx = ev.clientX - g.x0;
          const dy = ev.clientY - g.y0;
          const moved = g.moved || Math.hypot(dx, dy) >= CLICK_SLOP;
          const layout =
            kind === "move"
              ? moveBy(g.start, dx, dy, colW)
              : resizeBy(g.start, dx, dy, colW, KIND_INFO[o.kind].min);
          Object.assign(g, { dx, dy, moved, layout });
          setGesture({ ...g });
        },
        () => {
          setGesture(null);
          const { start, layout } = g;
          if (
            !g.moved ||
            (layout.x === start.x &&
              layout.y === start.y &&
              layout.w === start.w &&
              layout.h === start.h)
          )
            return;
          updateObjects(boardId, (list) =>
            resolveCollisions(
              list.map((x) => (x.id === id ? { ...x, layout } : x)),
              id,
            ),
          );
        },
      );
    },
    [boardId, colW, objects, onSelect, track],
  );

  const handlers: CardHandlers = {
    onMoveStart: (e, id) => startLayoutGesture("move", e, id),
    onResizeStart: (e, id) => startLayoutGesture("resize", e, id),
    onWireStart: (e, id) => {
      const o = objects.find((x) => x.id === id);
      if (!o) return;
      e.preventDefault();
      onSelect(id);
      const from = rightMid(toRect(o.layout, colW));
      const target = (ev: PointerEvent): string | null => {
        const el = document.elementFromPoint(ev.clientX, ev.clientY);
        const hit = el?.closest<HTMLElement>("[data-board-object]")?.dataset.boardObject ?? null;
        const t = hit && hit !== id ? objects.find((x) => x.id === hit) : undefined;
        return t && canWireTo(t) ? t.id : null;
      };
      setGesture({ kind: "wire", id, from, to: from, over: null });
      track(
        (ev) =>
          setGesture({
            kind: "wire",
            id,
            from,
            to: local(ev.clientX, ev.clientY),
            over: target(ev),
          }),
        (ev) => {
          setGesture(null);
          const to = target(ev);
          if (!to) return;
          const t = objects.find((x) => x.id === to);
          if (!t || o.targets.includes(to)) return;
          updateObjects(boardId, (list) => wire(list, id, to));
          if (t.refresh.mode !== "event")
            toast.info(`“${t.title}” now refreshes on event`, {
              description: `It runs again when “${o.title}” fires.`,
            });
        },
      );
    },
    onSelect: (id) => onSelect(id),
    onEdit,
    onDuplicate,
    onDelete,
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!editing || !selectedId) return;
    // Only keys pressed on an object or the canvas itself (not in a field of the inspector).
    const el = e.target as HTMLElement;
    if (!el.dataset.boardObject && el !== e.currentTarget) return;
    const o = objects.find((x) => x.id === selectedId);
    if (!o) return;
    if (e.key === "Escape") {
      e.preventDefault();
      onSelect(null);
      return;
    }
    if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      onDelete(o.id);
      return;
    }
    const step: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    const d = step[e.key];
    if (!d) return;
    e.preventDefault();
    const { min } = KIND_INFO[o.kind];
    const l = o.layout;
    const layout = e.shiftKey
      ? {
          ...l,
          w: Math.min(GRID_COLUMNS - l.x, Math.max(min.w, l.w + d[0])),
          h: Math.max(min.h, l.h + d[1]),
        }
      : {
          ...l,
          x: Math.min(GRID_COLUMNS - l.w, Math.max(0, l.x + d[0])),
          y: Math.max(0, l.y + d[1]),
        };
    updateObjects(boardId, (list) =>
      resolveCollisions(
        list.map((x) => (x.id === o.id ? { ...x, layout } : x)),
        o.id,
      ),
    );
  };

  const rows = usedRows(shown.map((o) => o.layout)) + (editing ? EDIT_SPARE_ROWS : 0);
  const rects = new Map(shown.map((o) => [o.id, toRect(o.layout, colW)]));
  const wires = editing
    ? shown.flatMap((o) =>
        o.targets.flatMap((t) => {
          const a = rects.get(o.id);
          const b = rects.get(t);
          if (!a || !b) return [];
          const from = rightMid(a);
          const to = b.left >= from.x ? leftMid(b) : rightMid(b);
          return [{ key: `${o.id}>${t}`, d: wirePath(from, to), to }];
        }),
      )
    : [];

  return (
    <div
      className="min-h-0 flex-1 overflow-auto p-3 outline-none"
      tabIndex={-1}
      onKeyDown={onKeyDown}
      onPointerDown={(e) => {
        if (editing && e.target === e.currentTarget) onSelect(null);
      }}
    >
      <div
        ref={grid}
        className="relative"
        style={{ height: Math.max(rowsHeight(rows), 0), minHeight: "100%" }}
        onPointerDown={(e) => {
          if (editing && e.target === e.currentTarget) onSelect(null);
        }}
      >
        {editing && width > 0 && (
          <div
            aria-hidden
            className="pointer-events-none absolute inset-0 flex"
            style={{ gap: GAP }}
          >
            {Array.from({ length: GRID_COLUMNS }, (_, i) => (
              <div key={i} className="bg-foreground/[0.025] flex-1 rounded-sm" />
            ))}
          </div>
        )}
        {gesture && gesture.kind !== "wire" && gesture.moved && (
          <div
            aria-hidden
            className="border-primary/60 bg-primary/5 pointer-events-none absolute rounded-lg border-2 border-dashed"
            style={rectStyle(toRect(gesture.layout, colW))}
          />
        )}
        {width > 0 &&
          shown.map((o) => {
            const r = rects.get(o.id)!;
            const moving = gesture?.kind === "move" && gesture.id === o.id && gesture.moved;
            const base = moving ? toRect(objects.find((x) => x.id === o.id)!.layout, colW) : r;
            return (
              <ObjectCard
                key={o.id}
                boardId={boardId}
                object={o}
                editing={editing}
                selected={editing && selectedId === o.id}
                dragging={moving}
                wireTarget={gesture?.kind === "wire" && gesture.over === o.id}
                runtime={runtime}
                handlers={handlers}
                style={{
                  ...rectStyle(base),
                  ...(moving
                    ? { transform: `translate(${gesture.dx}px, ${gesture.dy}px)`, opacity: 0.9 }
                    : {}),
                  transition: gesture
                    ? undefined
                    : "left 120ms, top 120ms, width 120ms, height 120ms",
                }}
              />
            );
          })}
        {editing && (wires.length > 0 || gesture?.kind === "wire") && (
          <svg
            aria-hidden
            className="pointer-events-none absolute inset-0 z-30 overflow-visible"
            width="100%"
            height="100%"
          >
            {wires.map((w) => (
              <g key={w.key}>
                <path
                  d={w.d}
                  fill="none"
                  stroke="var(--success)"
                  strokeWidth={1.5}
                  opacity={0.75}
                />
                <circle cx={w.to.x} cy={w.to.y} r={3} fill="var(--success)" />
              </g>
            ))}
            {gesture?.kind === "wire" && (
              <path
                d={wirePath(gesture.from, gesture.to)}
                fill="none"
                stroke="var(--success)"
                strokeWidth={2}
                strokeDasharray="4 3"
              />
            )}
          </svg>
        )}
        {objects.length === 0 && (
          <div
            className={cn(
              "text-muted-foreground absolute inset-0 flex items-center justify-center p-6 text-center text-xs",
            )}
          >
            {editing
              ? "Add charts, scoreboards, tables and inputs with + above."
              : "This board is empty. Click Edit to add objects."}
          </div>
        )}
      </div>
    </div>
  );
}

function rectStyle(r: Rect) {
  return { left: r.left, top: r.top, width: r.width, height: r.height };
}
