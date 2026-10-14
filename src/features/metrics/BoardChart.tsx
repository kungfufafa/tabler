import { useEffect, useMemo, useState, type PointerEvent, type RefObject } from "react";
import { NodeEl, Tooltip, XYHover } from "@/features/charts/ChartView";
import { buildChartData } from "@/features/charts/data";
import { layoutChart, nearestIndex, type Paint, type Scene } from "@/features/charts/scene";
import { paintVar } from "@/features/charts/svg";
import "@/features/charts/charts.css";
import type { MetricsResult } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { recolorScene, remapPaint } from "./chartColors";
import type { BoardObject } from "./model";
import { chartMapping } from "./results";

const MIN_W = 160;
const MIN_H = 120;

/** The element's size, tracked as it is resized. */
function useSize(ref: RefObject<HTMLElement | null>): { w: number; h: number } | null {
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      const w = Math.floor(r.width);
      const h = Math.floor(r.height);
      setSize((s) => (s && s.w === w && s.h === h ? s : { w, h }));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

/**
 * A board chart (bar, line or pie) of the object's result, titled with the object's title, in
 * the app's chart style (`features/charts`): tooltip on hover, the object's series colours.
 * `sceneRef` gets the drawn scene (Copy as SVG / Export PNG…).
 */
export function BoardChart({
  object,
  result,
  boxRef,
  sceneRef,
}: {
  object: BoardObject;
  result: MetricsResult | null;
  boxRef: RefObject<HTMLDivElement | null>;
  sceneRef: { current: Scene | null };
}) {
  const kind = object.kind === "line" ? "line" : object.kind === "pie" ? "pie" : "bar";
  const built = useMemo(() => {
    if (!result) return null;
    const { x, y } = chartMapping(object);
    return buildChartData({ kind, x, y }, result.columns, result.rows);
  }, [kind, object, result]);
  const size = useSize(boxRef);
  const scene = useMemo(() => {
    if (!built?.ok || !size) return null;
    const s = layoutChart(
      built.data,
      Math.max(MIN_W, size.w),
      Math.max(MIN_H, size.h),
      object.title,
    );
    return recolorScene(s, object.colors);
  }, [built, size, object.title, object.colors]);
  useEffect(() => {
    sceneRef.current = scene;
  }, [scene, sceneRef]);
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);
  const pie = kind === "pie";
  const count = scene ? (pie ? scene.slices.length : scene.xs.length) : 0;
  const shown = hover && hover.i < count ? hover : null;

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

  const tooltip = scene && shown ? scene.tooltip(shown.i) : null;
  const notes = built?.ok ? built.data.notes : [];
  return (
    <div className="tabler-chart relative flex h-full min-h-0 flex-col">
      <div ref={boxRef} className="relative min-h-0 flex-1 overflow-hidden">
        {built && !built.ok ? (
          <div className="flex h-full flex-col">
            <p className="truncate px-3 pt-2.5 text-[12px] font-semibold">{object.title}</p>
            <p className="text-muted-foreground flex flex-1 items-center justify-center p-3 text-center text-xs">
              {built.error}
            </p>
          </div>
        ) : (
          scene && (
            <svg
              width={scene.width}
              height={scene.height}
              viewBox={`0 0 ${scene.width} ${scene.height}`}
              role="img"
              aria-label={object.title}
              className="block select-none"
              onPointerMove={onMove}
              onPointerLeave={() => setHover(null)}
            >
              {shown && kind === "bar" && <XYHover scene={scene} i={shown.i} bar />}
              {scene.nodes.map((n, i) => (
                <NodeEl key={i} n={n} />
              ))}
              {shown && kind === "line" && (
                <LineHover scene={scene} i={shown.i} colors={object.colors} />
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
      {notes.length > 0 && (
        <p
          className="text-muted-foreground shrink-0 truncate px-3 pb-1.5 text-[10px]"
          title={notes.join(" ")}
        >
          {notes.join(" ")}
        </p>
      )}
    </div>
  );
}

/** A line chart's hover: crosshair and the points at `i`, in the object's colours. */
function LineHover({ scene, i, colors }: { scene: Scene; i: number; colors: readonly number[] }) {
  const x = scene.xs[i];
  if (x === undefined) return null;
  const { plot } = scene;
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
            fill={paintVar(remapPaint(`series-${(j % 8) + 1}` as Paint, colors))}
            stroke={paintVar("surface")}
            strokeWidth={2}
          />
        );
      })}
    </g>
  );
}
