import {
  ChevronDown,
  Copy,
  FileCode,
  FileImage,
  Focus,
  LayoutTemplate,
  Loader2,
  Maximize,
  RefreshCw,
  Search,
  Spline,
  Workflow,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { compileSearch, isSearching } from "@/features/objects/search";
import { useAction } from "@/features/palette/actions";
import { useSchemas } from "@/features/sidebar/useSchemaTree";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type TableView } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { DiagramScene } from "./DiagramScene";
import { copyDiagramSvg, exportDiagram, type ExportInput } from "./export";
import type { LineStyle } from "./geometry";
import { isLocal, relationsAmong, relationsOf, visibleTables, withNeighbours } from "./graph";
import { autoLayout, boundsOf, placeTables, rectsOf, type Positions, type Size } from "./layout";
import { useDiagramSchema } from "./queries";
import { LIVE_PALETTE, type SceneFocus, type SceneHandlers } from "./scene";
import type { DiagramRequest } from "./store";
import {
  centerOn,
  fitView,
  forgetPositions,
  IDENTITY,
  loadLineStyle,
  loadPositions,
  PAN_STEP,
  PAN_STEP_LARGE,
  positionsKey,
  saveLineStyle,
  savePositions,
  zoomAt,
  ZOOM_STEP,
  type View,
} from "./viewport";

/** A click on a table name waits this long for a second click (structure) before opening. */
export const NAME_DOUBLE_CLICK_MS = 250;
/** Pointer travel (px) below which a press is a click, not a drag. */
const DRAG_THRESHOLD = 3;

const EMPTY: ReadonlySet<string> = new Set();

type Gesture =
  | {
      kind: "card";
      name: string;
      names: string[];
      origin: Positions;
      additive: boolean;
      startX: number;
      startY: number;
      moved: boolean;
    }
  | {
      kind: "pan";
      origin: View;
      additive: boolean;
      startX: number;
      startY: number;
      moved: boolean;
    };

function isTextField(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.matches("input, textarea, select");
}

const message = (e: unknown) =>
  e instanceof Error
    ? e.message
    : typeof e === "object" && e && "message" in e
      ? String(e.message)
      : String(e);

/**
 * Generate Diagram… (spec §11.4 / §11.5): the ER diagram of a schema. Tables are cards (PK key,
 * FK arrow, type), foreign keys crow's-foot lines between the column rows. Cards can be dragged
 * (positions are remembered per profile, database and schema); the canvas pans and zooms.
 */
export function DiagramDialog({
  request,
  onClose,
}: {
  request: DiagramRequest;
  onClose: () => void;
}) {
  const { connectionId } = request;
  const conn = useConnectionsStore((s) => s.byId[connectionId]);
  const schemas = useSchemas(connectionId);
  const schema =
    request.schema ??
    conn?.currentSchema ??
    schemas.data?.find((s) => s.isDefault)?.name ??
    schemas.data?.[0]?.name ??
    null;
  const query = useDiagramSchema(connectionId, schema);
  const data = query.data;
  const storageKey =
    schema !== null && conn
      ? positionsKey(conn.info.profileId, conn.info.currentDatabase, schema)
      : null;

  const [selection, setSelection] = useState<ReadonlySet<string>>(
    () => new Set((request.tables ?? []).map((t) => t.name)),
  );
  const [onlySelected, setOnlySelected] = useState((request.tables?.length ?? 0) > 0);
  const [positions, setPositions] = useState<Positions | null>(null);
  const [view, setView] = useState<View>(IDENTITY);
  const [search, setSearch] = useState("");
  const [lineStyle, setLineStyleState] = useState<LineStyle>(loadLineStyle);
  const [hovered, setHovered] = useState<string | null>(null);
  const [size, setSize] = useState<Size>({ w: 0, h: 0 });
  const [panning, setPanning] = useState(false);
  const [spaceHeld, setSpaceHeld] = useState(false);

  const canvasRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // Tables and selection as they are now (the selection may name tables that are gone).
  const names = useMemo(() => new Set(data?.tables.map((t) => t.ref.name)), [data]);
  const selected = useMemo(
    () => new Set([...selection].filter((n) => names.has(n))),
    [selection, names],
  );
  const visible = useMemo(
    () => (data ? visibleTables(data, selected, onlySelected) : []),
    [data, selected, onlySelected],
  );
  const visibleNames = useMemo(() => new Set(visible.map((t) => t.ref.name)), [visible]);
  const drawn = useMemo(
    () => (data ? relationsAmong(data, visibleNames) : []),
    [data, visibleNames],
  );
  const external = useMemo(
    () => data?.relations.filter((r) => !isLocal(r, data.schema)).length ?? 0,
    [data],
  );
  const matches = useMemo(() => {
    if (!isSearching(search)) return EMPTY;
    const match = compileSearch(search);
    return new Set(visible.filter((t) => match(t.ref.name)).map((t) => t.ref.name));
  }, [search, visible]);

  // Latest values for the window-level pointer handlers.
  const latest = useRef({
    view,
    positions,
    selected,
    visible,
    size,
    spaceHeld,
    storageKey,
    onlySelected,
  });
  useLayoutEffect(() => {
    latest.current = {
      view,
      positions,
      selected,
      visible,
      size,
      spaceHeld,
      storageKey,
      onlySelected,
    };
  });

  // Positions: the remembered ones (tables added since get an automatic place).
  useEffect(() => {
    if (!data) return;
    setPositions((prev) =>
      placeTables(
        data.tables,
        data.relations,
        data.schema,
        prev ?? (storageKey ? loadPositions(storageKey) : null),
      ),
    );
  }, [data, storageKey]);

  // The canvas size, for fitting.
  useLayoutEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setSize((s) => (s.w === r.width && s.h === r.height ? s : { w: r.width, h: r.height }));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const fitTo = useCallback((tables = latest.current.visible) => {
    const { positions: p, size: s } = latest.current;
    if (!p || s.w === 0) return;
    setView(fitView(boundsOf(tables, p), s));
  }, []);

  // Fit once the cards and the canvas size are known.
  const fitted = useRef(false);
  useEffect(() => {
    if (fitted.current || !positions || size.w === 0 || visible.length === 0) return;
    fitted.current = true;
    setView(fitView(boundsOf(visible, positions), size));
  }, [positions, size, visible]);

  const center = () => ({ x: latest.current.size.w / 2, y: latest.current.size.h / 2 });
  const zoomBy = (factor: number) => setView((v) => zoomAt(v, factor, center()));
  const actualSize = () => setView((v) => zoomAt(v, 1 / v.k, center()));

  const setLineStyle = (style: LineStyle) => {
    setLineStyleState(style);
    saveLineStyle(style);
  };

  const openTable = useCallback(
    (name: string, kind: TableView) => {
      if (schema === null) return;
      useTabsStore.getState().openTable(connectionId, { schema, name }, kind);
      useUiStore.getState().setView("workspace");
      onClose();
    },
    [connectionId, schema, onClose],
  );

  // ⌘W closes the diagram (its handler stacks on the tab's while open).
  useAction("tab.close", onClose);

  // --- pointer: drag cards, pan the background ------------------------------------------------

  const gesture = useRef<Gesture | null>(null);
  const suppressClick = useRef(false);

  const select = useCallback((name: string, additive: boolean) => {
    setSelection((prev) => {
      if (!additive) return new Set([name]);
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }, []);

  const startGesture = useCallback(
    (g: Gesture) => {
      gesture.current = g;
      suppressClick.current = false;
      const move = (e: PointerEvent) => {
        const cur = gesture.current;
        if (!cur) return;
        const dx = e.clientX - cur.startX;
        const dy = e.clientY - cur.startY;
        if (!cur.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
        cur.moved = true;
        if (cur.kind === "pan") {
          setPanning(true);
          setView({ ...cur.origin, x: cur.origin.x + dx, y: cur.origin.y + dy });
        } else {
          const k = latest.current.view.k;
          setPositions((prev) => {
            if (!prev) return prev;
            const next = { ...prev };
            for (const [n, p] of Object.entries(cur.origin))
              next[n] = { x: Math.round(p.x + dx / k), y: Math.round(p.y + dy / k) };
            return next;
          });
        }
      };
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        const cur = gesture.current;
        gesture.current = null;
        if (!cur) return;
        suppressClick.current = cur.moved;
        setPanning(false);
        if (cur.kind === "pan") {
          // A click on the background clears the selection, unless it decides what is shown.
          if (!cur.moved && !cur.additive && !latest.current.onlySelected) setSelection(EMPTY);
          return;
        }
        if (!cur.moved) {
          select(cur.name, cur.additive);
          return;
        }
        // Remember where the cards were dropped (after the last move's state update).
        requestAnimationFrame(() => {
          const { positions: p, storageKey: key } = latest.current;
          if (p && key) savePositions(key, p);
        });
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      window.addEventListener("pointercancel", up);
    },
    [select],
  );

  const focusCanvas = () => {
    const el = canvasRef.current;
    if (el && !el.contains(document.activeElement)) el.focus({ preventScroll: true });
  };

  const onCanvasPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 && e.button !== 1) return;
    focusCanvas();
    startGesture({
      kind: "pan",
      origin: latest.current.view,
      additive: e.metaKey || e.shiftKey || e.ctrlKey,
      startX: e.clientX,
      startY: e.clientY,
      moved: false,
    });
  };

  const clickTimer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (clickTimer.current !== null) window.clearTimeout(clickTimer.current);
    },
    [],
  );

  const handlers = useMemo<SceneHandlers>(
    () => ({
      onCardPointerDown: (name, e) => {
        // Space + drag pans, even over a card.
        if (e.button !== 0 || latest.current.spaceHeld) return;
        e.stopPropagation();
        focusCanvas();
        const { positions: p, selected: sel } = latest.current;
        if (!p) return;
        const moving = sel.has(name) ? [...sel].filter((n) => p[n]) : [name];
        startGesture({
          kind: "card",
          name,
          names: moving,
          origin: Object.fromEntries(moving.map((n) => [n, p[n]!])),
          additive: e.metaKey || e.shiftKey || e.ctrlKey,
          startX: e.clientX,
          startY: e.clientY,
          moved: false,
        });
      },
      onNameClick: (name: string, e: ReactMouseEvent<SVGTextElement>) => {
        if (suppressClick.current || e.metaKey || e.shiftKey || e.ctrlKey) return;
        if (clickTimer.current !== null) window.clearTimeout(clickTimer.current);
        clickTimer.current = window.setTimeout(() => {
          clickTimer.current = null;
          openTable(name, "table");
        }, NAME_DOUBLE_CLICK_MS);
      },
      onNameDoubleClick: (name: string) => {
        if (clickTimer.current !== null) window.clearTimeout(clickTimer.current);
        clickTimer.current = null;
        openTable(name, "structure");
      },
      onRelationHover: setHovered,
    }),
    [startGesture, openTable],
  );

  // Wheel: pan; with ⌘ (or a pinch, which arrives as ⌃-wheel) zoom at the pointer. WebKit
  // reports trackpad pinches as gesture events instead.
  useEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const anchor = (e: { clientX: number; clientY: number }) => {
      const r = el.getBoundingClientRect();
      return { x: e.clientX - r.left, y: e.clientY - r.top };
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 16 : 1;
      if (e.metaKey || e.ctrlKey) {
        const factor = Math.exp(-e.deltaY * unit * (e.ctrlKey && !e.metaKey ? 0.01 : 0.0025));
        setView((v) => zoomAt(v, factor, anchor(e)));
      } else {
        setView((v) => ({ ...v, x: v.x - e.deltaX * unit, y: v.y - e.deltaY * unit }));
      }
    };
    let gestureStartK = 1;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      gestureStartK = latest.current.view.k;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as Event & { scale?: number; clientX?: number; clientY?: number };
      const target = gestureStartK * (g.scale ?? 1);
      setView((v) =>
        zoomAt(v, target / v.k, anchor({ clientX: g.clientX ?? 0, clientY: g.clientY ?? 0 })),
      );
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
    };
  }, []);

  // Space held = pan mode.
  useEffect(() => {
    const release = () => setSpaceHeld(false);
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === "Space") release();
    };
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", release);
    };
  }, []);

  // --- commands --------------------------------------------------------------------------------

  const selectAll = () => setSelection(new Set(latest.current.visible.map((t) => t.ref.name)));

  const toggleOnlySelected = () => {
    const next = !onlySelected;
    setOnlySelected(next);
    if (data) fitTo(visibleTables(data, selected, next));
  };

  const autoArrange = () => {
    if (!data) return;
    if (storageKey) forgetPositions(storageKey);
    const fresh = autoLayout(data.tables, data.relations, data.schema);
    setPositions(fresh);
    const s = latest.current.size;
    if (s.w > 0) setView(fitView(boundsOf(visible, fresh), s));
  };

  const searchEnter = () => {
    if (matches.size === 0) return;
    setSelection(new Set(matches));
    const p = latest.current.positions;
    if (!p) return;
    const hits = visible.filter((t) => matches.has(t.ref.name));
    if (hits.length === 1) {
      const rect = rectsOf(hits, p).get(hits[0]!.ref.name);
      if (rect) setView((v) => centerOn(v, rect, latest.current.size));
    } else fitTo(hits);
  };

  const exportInput = (): ExportInput | null =>
    positions ? { tables: visible, relations: drawn, positions, lineStyle } : null;
  const baseName = `${schema ?? "diagram"}-diagram`;
  const runExport = async (format: "svg" | "png") => {
    const input = exportInput();
    if (!input) return;
    try {
      const path = await exportDiagram(input, format, baseName);
      if (path) toast.success("Diagram exported", { description: path });
    } catch (e) {
      toast.error("Could not export the diagram", { description: message(e) });
    }
  };
  const copySvg = async () => {
    const input = exportInput();
    if (!input) return;
    try {
      await copyDiagramSvg(input);
      toast.success("Diagram copied as SVG");
    } catch (e) {
      toast.error("Could not copy the diagram", { description: message(e) });
    }
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    const key = e.key.toLowerCase();
    if (mod && !e.altKey && key === "f") {
      e.preventDefault();
      searchRef.current?.focus();
      searchRef.current?.select();
      return;
    }
    if (mod && (e.key === "=" || e.key === "+")) {
      e.preventDefault();
      zoomBy(ZOOM_STEP);
      return;
    }
    if (mod && (e.key === "-" || e.key === "_")) {
      e.preventDefault();
      zoomBy(1 / ZOOM_STEP);
      return;
    }
    if (mod && e.key === "0") {
      e.preventDefault();
      actualSize();
      return;
    }
    // The rest belongs to the canvas: menus (portalled, but their keys bubble here through
    // React) and fields keep their arrows, space and ⌘A.
    if (
      isTextField(e.target) ||
      !(e.target instanceof Node && canvasRef.current?.contains(e.target))
    )
      return;
    if (mod && key === "a") {
      e.preventDefault();
      selectAll();
      return;
    }
    if (e.code === "Space" && !mod) {
      e.preventDefault();
      if (!spaceHeld) setSpaceHeld(true);
      return;
    }
    const step = e.shiftKey ? PAN_STEP_LARGE : PAN_STEP;
    const pan: Record<string, [number, number]> = {
      ArrowLeft: [step, 0],
      ArrowRight: [-step, 0],
      ArrowUp: [0, step],
      ArrowDown: [0, -step],
    };
    const d = pan[e.key];
    if (d && !mod) {
      e.preventDefault();
      setView((v) => ({ ...v, x: v.x + d[0], y: v.y + d[1] }));
    }
  };

  // --- render ------------------------------------------------------------------------------------

  const focus = useMemo<SceneFocus>(() => {
    const searching = isSearching(search);
    return {
      selected,
      lit: searching ? matches : selected.size > 0 && data ? withNeighbours(data, selected) : null,
      litRelations: selected.size > 0 ? relationsOf(drawn, selected) : null,
      matches,
      hoveredRelation: hovered,
    };
  }, [search, matches, selected, data, drawn, hovered]);

  const title = schema ? `Diagram — ${schema}` : "Diagram";
  const counts = data
    ? `${visible.length === data.tables.length ? data.tables.length : `${visible.length} of ${data.tables.length}`} ${data.tables.length === 1 ? "table" : "tables"} · ${drawn.length} ${drawn.length === 1 ? "relation" : "relations"}${external > 0 ? ` · ${external} to other schemas not shown` : ""}`
    : query.isError
      ? "Could not load the schema"
      : "Loading…";
  const grid = 20 * view.k;

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        aria-label={title}
        className="flex h-[94vh] max-h-[94vh] w-[96vw] max-w-[96vw] flex-col gap-2 p-3 sm:max-w-[96vw]"
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          canvasRef.current?.focus({ preventScroll: true });
        }}
        onEscapeKeyDown={(e) => {
          // ⎋ in a search field with text clears it first.
          if (document.activeElement === searchRef.current && search !== "") {
            e.preventDefault();
            setSearch("");
            canvasRef.current?.focus({ preventScroll: true });
          }
        }}
        onKeyDown={onKeyDown}
      >
        <DialogHeader className="flex-row items-center gap-2 space-y-0 pr-8">
          <div className="min-w-0 flex-1">
            <DialogTitle className="truncate text-sm">{title}</DialogTitle>
            <DialogDescription className="truncate text-xs">{counts}</DialogDescription>
          </div>
          <div className="relative">
            <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
            <Input
              ref={searchRef}
              aria-label="Search tables"
              placeholder="Search tables (⌘F)"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  searchEnter();
                }
              }}
              className="h-7 w-52 pl-7 text-xs md:text-xs"
            />
            {isSearching(search) && (
              <span className="text-muted-foreground pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-[10px] tabular-nums">
                {matches.size}
              </span>
            )}
          </div>
          <Button
            variant={onlySelected ? "secondary" : "ghost"}
            size="sm"
            aria-pressed={onlySelected}
            onClick={toggleOnlySelected}
            title="Show only the selected tables and the tables related to them"
            className="h-7 text-xs"
          >
            <Focus />
            Selected + Neighbours
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={lineStyle === "curved" ? "Orthogonal lines" : "Curved lines"}
            title={
              lineStyle === "curved"
                ? "Lines: curved (switch to orthogonal)"
                : "Lines: orthogonal (switch to curved)"
            }
            onClick={() => setLineStyle(lineStyle === "curved" ? "orthogonal" : "curved")}
          >
            {lineStyle === "curved" ? <Spline /> : <Workflow />}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={autoArrange}
            disabled={!data}
            title="Arrange every table again (forgets the positions you moved them to)"
            className="h-7 text-xs"
          >
            <LayoutTemplate />
            Auto Layout
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="sm" disabled={!positions} className="h-7 text-xs">
                Export
                <ChevronDown />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-40">
              <DropdownMenuItem onSelect={() => void runExport("svg")}>
                <FileCode />
                SVG…
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void runExport("png")}>
                <FileImage />
                PNG…
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => void copySvg()}>
                <Copy />
                Copy as SVG
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Refresh"
            title="Refresh"
            onClick={() => void query.refetch()}
            disabled={query.isFetching || schema === null}
          >
            <RefreshCw className={cn(query.isFetching && "animate-spin")} />
          </Button>
        </DialogHeader>

        <div
          ref={canvasRef}
          role="application"
          aria-label="Diagram canvas"
          aria-roledescription="diagram"
          tabIndex={0}
          onPointerDown={onCanvasPointerDown}
          className={cn(
            "bg-background border-border relative min-h-0 flex-1 touch-none overflow-hidden rounded-md border outline-none select-none",
            "focus-visible:ring-ring/40 focus-visible:ring-2",
            panning ? "cursor-grabbing" : spaceHeld && "cursor-grab",
          )}
          style={{
            backgroundImage: "radial-gradient(var(--border) 1px, transparent 1px)",
            backgroundSize: `${grid}px ${grid}px`,
            backgroundPosition: `${view.x}px ${view.y}px`,
          }}
        >
          {data && positions && (
            <svg className="absolute inset-0 size-full" aria-hidden="true">
              <g transform={`translate(${view.x} ${view.y}) scale(${view.k})`}>
                <DiagramScene
                  tables={visible}
                  relations={drawn}
                  positions={positions}
                  lineStyle={lineStyle}
                  palette={LIVE_PALETTE}
                  focus={focus}
                  handlers={handlers}
                />
              </g>
            </svg>
          )}
          {(query.isPending || (data && !positions)) && schema !== null && (
            <div className="absolute inset-0 grid place-items-center">
              <Loader2 className="text-muted-foreground size-4 animate-spin" />
            </div>
          )}
          {query.isError && (
            <div role="alert" className="absolute inset-0 grid place-items-center p-6">
              <div className="flex max-w-md flex-col items-center gap-2 text-center">
                <p className="text-error text-xs">{query.error.message}</p>
                <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
                  Try Again
                </Button>
              </div>
            </div>
          )}
          {data && data.tables.length === 0 && (
            <p className="text-muted-foreground absolute inset-0 grid place-items-center text-xs italic">
              This schema has no tables
            </p>
          )}
        </div>

        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Zoom out"
            title="Zoom out (⌘−)"
            onClick={() => zoomBy(1 / ZOOM_STEP)}
          >
            <ZoomOut />
          </Button>
          <button
            type="button"
            onClick={actualSize}
            title="Actual size (⌘0)"
            className="text-muted-foreground hover:text-foreground w-11 text-center text-[11px] tabular-nums"
          >
            {Math.round(view.k * 100)}%
          </button>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Zoom in"
            title="Zoom in (⌘+)"
            onClick={() => zoomBy(ZOOM_STEP)}
          >
            <ZoomIn />
          </Button>
          <Button
            variant="ghost"
            size="xs"
            onClick={() => fitTo()}
            disabled={!positions}
            title="Fit the diagram to the window"
            className="text-xs"
          >
            <Maximize />
            Fit
          </Button>
          <p className="text-muted-foreground ml-auto truncate text-[11px]">
            Click a name to open the table, double-click for its structure · drag cards to move them
            · drag or scroll to pan, ⌘-scroll or pinch to zoom
          </p>
        </div>
      </DialogContent>
    </Dialog>
  );
}
