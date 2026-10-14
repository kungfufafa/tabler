import { ClipboardCopy, ImageDown } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { BarIconButton } from "@/features/table-tab/BarParts";
import type { ResultColumn } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { resultData, useResultRowCount } from "@/stores/resultData";
import "./charts.css";
import { buildChartData } from "./data";
import type { ChartDirective } from "./directive";
import { copyChartSvg, exportChartPng } from "./export";
import {
  layoutChart,
  nearestIndex,
  type Paint,
  type Scene,
  type SceneNode,
  type TooltipData,
} from "./scene";
import { paintVar } from "./svg";

const MIN_W = 200;
const MIN_H = 140;

export function NodeEl({ n }: { n: SceneNode }) {
  switch (n.t) {
    case "rect":
      return <rect x={n.x} y={n.y} width={n.w} height={n.h} rx={n.rx} fill={paintVar(n.fill)} />;
    case "path":
      return (
        <path
          d={n.d}
          fill={n.fill === "none" ? "none" : paintVar(n.fill)}
          stroke={n.stroke ? paintVar(n.stroke) : undefined}
          strokeWidth={n.strokeWidth}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      );
    case "line":
      return (
        <line
          x1={n.x1}
          y1={n.y1}
          x2={n.x2}
          y2={n.y2}
          stroke={paintVar(n.stroke)}
          strokeWidth={1}
          shapeRendering="crispEdges"
        />
      );
    case "circle":
      return (
        <circle
          cx={n.cx}
          cy={n.cy}
          r={n.r}
          fill={paintVar(n.fill)}
          stroke={n.stroke ? paintVar(n.stroke) : undefined}
          strokeWidth={n.strokeWidth}
        />
      );
    case "text":
      return (
        <text
          x={n.x}
          y={n.y}
          fill={paintVar(n.fill)}
          fontSize={n.size}
          fontWeight={n.weight}
          textAnchor={n.anchor === "start" ? undefined : n.anchor}
        >
          {n.text}
        </text>
      );
  }
}

/** A tooltip row's key: a short stroke for lines, a small square for bars and slices. */
function Key({ paint, mark }: { paint: Paint; mark: "rect" | "line" }) {
  return mark === "line" ? (
    <span className="h-0.5 w-2.5 shrink-0 rounded-full" style={{ background: paintVar(paint) }} />
  ) : (
    <span className="size-2 shrink-0 rounded-[2px]" style={{ background: paintVar(paint) }} />
  );
}

export function Tooltip({
  data,
  x,
  y,
  flip,
}: {
  data: TooltipData;
  x: number;
  y: number;
  flip: boolean;
}) {
  return (
    <div
      role="tooltip"
      className="bg-popover text-popover-foreground border-border pointer-events-none absolute z-10 max-w-64 rounded-md border px-2 py-1.5 text-[11px] shadow-md"
      style={{ left: x, top: y, transform: flip ? "translateX(-100%)" : undefined }}
    >
      {data.title && <div className="text-muted-foreground mb-1 truncate">{data.title}</div>}
      {data.rows.map((r, i) => (
        <div key={i} className="flex items-center gap-1.5 whitespace-nowrap">
          <Key paint={r.paint} mark={r.mark} />
          <span className="font-semibold tabular-nums">{r.value}</span>
          <span className="text-muted-foreground min-w-0 truncate">{r.label}</span>
        </div>
      ))}
    </div>
  );
}

/** The hover layer of a bar / line chart at point `i`: band, crosshair and points. */
export function XYHover({ scene, i, bar }: { scene: Scene; i: number; bar: boolean }): ReactNode {
  const x = scene.xs[i];
  if (x === undefined) return null;
  const { plot } = scene;
  if (bar)
    return (
      <rect
        x={x - scene.band / 2}
        y={plot.y}
        width={scene.band}
        height={plot.h}
        fill={paintVar("hover")}
        opacity={0.06}
      />
    );
  return (
    <g>
      <line
        x1={x}
        x2={x}
        y1={plot.y}
        y2={plot.y + plot.h}
        stroke={paintVar("muted")}
        strokeWidth={1}
        shapeRendering="crispEdges"
      />
      {scene.ys.map((ys, j) => {
        const y = ys[i];
        return y === null || y === undefined ? null : (
          <circle
            key={j}
            cx={x}
            cy={y}
            r={4}
            fill={paintVar(`series-${(j % 8) + 1}` as Paint)}
            stroke={paintVar("surface")}
            strokeWidth={2}
          />
        );
      })}
    </g>
  );
}

/** The element's size, tracked as it is resized. */
function useSize(ref: RefObject<HTMLElement | null>): { w: number; h: number } | null {
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setSize((s) =>
        s && s.w === Math.floor(r.width) && s.h === Math.floor(r.height)
          ? s
          : { w: Math.floor(r.width), h: Math.floor(r.height) },
      );
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

export interface ChartViewProps {
  /** The result buffer (`resultData`) to chart. */
  resultId: string;
  columns: ResultColumn[];
  directive: ChartDirective;
  /** Rows may still be streaming in. */
  done: boolean;
}

/**
 * A result drawn as the chart its comment asks for (`-- TablePlus BarChart x: …, y: …`): bars,
 * lines or a pie over the loaded rows, with a tooltip on hover (or ←/→ with the chart focused),
 * Copy as SVG and Export PNG….
 */
export function ChartView({ resultId, columns, directive, done }: ChartViewProps) {
  const rowCount = useResultRowCount(resultId);
  const buffer = resultData.get(resultId);
  const rows = buffer?.rows;
  const cols = buffer && buffer.columns.length > 0 ? buffer.columns : columns;
  // `rows` grows in place while streaming: `rowCount` says when to rebuild.
  const built = useMemo(
    () => buildChartData(directive, cols, rows ?? []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [directive, cols, rows, rowCount],
  );
  const box = useRef<HTMLDivElement>(null);
  const size = useSize(box);
  const scene = useMemo(
    () =>
      built.ok && size
        ? layoutChart(built.data, Math.max(MIN_W, size.w), Math.max(MIN_H, size.h))
        : null,
    [built, size],
  );
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);
  const count = scene ? (scene.slices.length > 0 ? scene.slices.length : scene.xs.length) : 0;
  const shown = hover && hover.i < count ? hover : null;

  const pie = built.ok && built.data.kind === "pie";
  const anchorOf = (i: number): { x: number; y: number } | null => {
    if (!scene) return null;
    const slice = scene.slices[i];
    if (slice) return { x: slice.ax, y: slice.ay };
    const x = scene.xs[i];
    if (x === undefined) return null;
    const ys = scene.ys.map((s) => s[i]).filter((v): v is number => typeof v === "number");
    return { x, y: ys.length > 0 ? Math.min(...ys) : scene.plot.y + 12 };
  };

  const onMove = (e: PointerEvent<SVGSVGElement>) => {
    if (!scene || pie) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    const { plot } = scene;
    const slack = scene.band / 2 + 8;
    if (x < plot.x - slack || x > plot.x + plot.w + slack || y < plot.y || y > plot.y + plot.h) {
      setHover(null);
      return;
    }
    const i = nearestIndex(scene.xs, x);
    setHover(i === null ? null : { i, x, y });
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (count === 0) return;
    const at = shown?.i ?? -1;
    const next =
      e.key === "ArrowRight" || e.key === "ArrowDown"
        ? Math.min(count - 1, at + 1)
        : e.key === "ArrowLeft" || e.key === "ArrowUp"
          ? Math.max(0, at < 0 ? 0 : at - 1)
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? count - 1
              : e.key === "Escape"
                ? null
                : undefined;
    if (next === undefined) return;
    e.preventDefault();
    if (next === null) return setHover(null);
    const a = anchorOf(next);
    if (a) setHover({ i: next, ...a });
  };

  const notes = built.ok ? built.data.notes : [];
  const tooltip = scene && shown ? scene.tooltip(shown.i) : null;
  return (
    <div className="tabler-chart bg-surface flex h-full min-h-0 flex-col" data-slot="result-chart">
      <div className="border-border flex h-7 shrink-0 items-center gap-1 border-b px-2 text-[11px]">
        <span className="text-muted-foreground min-w-0 flex-1 truncate" title={notes.join(" ")}>
          {!done && "Loading rows… "}
          {notes.join(" ")}
        </span>
        <BarIconButton
          label="Copy as SVG"
          disabled={!scene}
          onClick={() => scene && void copyChartSvg(scene)}
        >
          <ClipboardCopy />
        </BarIconButton>
        <BarIconButton
          label="Export PNG…"
          disabled={!scene}
          onClick={() => scene && void exportChartPng(scene)}
        >
          <ImageDown />
        </BarIconButton>
      </div>
      <div
        ref={box}
        className="focus-visible:ring-ring/50 relative min-h-0 flex-1 overflow-hidden outline-none focus-visible:ring-2 focus-visible:ring-inset"
        tabIndex={0}
        aria-label={scene?.title ?? "Chart"}
        onKeyDown={onKeyDown}
        onBlur={() => setHover(null)}
      >
        {!built.ok ? (
          <div className="text-muted-foreground flex h-full items-center justify-center p-4 text-center text-xs">
            {built.error}
          </div>
        ) : (
          scene && (
            <svg
              width={scene.width}
              height={scene.height}
              viewBox={`0 0 ${scene.width} ${scene.height}`}
              role="img"
              aria-label={scene.title}
              className="block select-none"
              onPointerMove={onMove}
              onPointerLeave={() => setHover(null)}
            >
              {shown && !pie && built.data.kind === "bar" && (
                <XYHover scene={scene} i={shown.i} bar />
              )}
              {scene.nodes.map((n, i) => (
                <NodeEl key={i} n={n} />
              ))}
              {shown && !pie && built.data.kind === "line" && (
                <XYHover scene={scene} i={shown.i} bar={false} />
              )}
              {pie &&
                scene.slices.map((s, i) => (
                  <path
                    key={i}
                    d={shown?.i === i ? s.dLift : s.d}
                    fill={shown?.i === i ? paintVar(s.paint) : "transparent"}
                    stroke={shown?.i === i ? paintVar("surface") : undefined}
                    strokeWidth={2}
                    className={cn(shown?.i === i && "drop-shadow-sm")}
                    onPointerMove={(e) => {
                      const rect = e.currentTarget.ownerSVGElement?.getBoundingClientRect();
                      if (rect) setHover({ i, x: e.clientX - rect.left, y: e.clientY - rect.top });
                    }}
                    onPointerLeave={() => setHover(null)}
                  />
                ))}
            </svg>
          )
        )}
        {tooltip && shown && scene && (
          <Tooltip
            data={tooltip}
            x={shown.x > scene.width / 2 ? shown.x - 12 : shown.x + 12}
            y={Math.max(4, Math.min(shown.y - 10, scene.height - 24 - tooltip.rows.length * 16))}
            flip={shown.x > scene.width / 2}
          />
        )}
      </div>
    </div>
  );
}
