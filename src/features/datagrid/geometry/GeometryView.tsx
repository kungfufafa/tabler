import { Maximize, Minus, Plus } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type WheelEvent as ReactWheelEvent,
} from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  axisLabel,
  boundsOf,
  isGeographic,
  niceStep,
  pointCount,
  svgPath,
  type Bounds,
  type ParsedGeometry,
  type Pt,
} from "./wkt";

const FILL = "color-mix(in oklab, var(--brand) 22%, transparent)";

/** `b` padded by `pad` of its size, never empty (a lone point gets a unit box around it). */
function padded(b: Bounds, pad: number, minSize: number): Bounds {
  let w = b.maxX - b.minX;
  let h = b.maxY - b.minY;
  const cx = (b.minX + b.maxX) / 2;
  const cy = (b.minY + b.maxY) / 2;
  if (w === 0 && h === 0) w = h = minSize;
  else if (w === 0) w = h;
  else if (h === 0) h = w;
  w *= 1 + pad * 2;
  h *= 1 + pad * 2;
  return { minX: cx - w / 2, maxX: cx + w / 2, minY: cy - h / 2, maxY: cy + h / 2 };
}

/** The shapes drawn into a `width` × `height` box through `project` (world → pixels). */
function Shapes({ g, project }: { g: ParsedGeometry; project: (p: Pt) => Pt }) {
  const s = g.shapes;
  return (
    <>
      {s.polygons.length > 0 && (
        <path
          data-geometry="polygons"
          d={svgPath(s, project, "polygons")}
          fill={FILL}
          fillRule="evenodd"
          stroke="var(--brand)"
          strokeWidth={1.25}
          strokeLinejoin="round"
        />
      )}
      {s.lines.length > 0 && (
        <path
          data-geometry="lines"
          d={svgPath(s, project, "lines")}
          fill="none"
          stroke="var(--brand)"
          strokeWidth={1.75}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      )}
      {s.points.slice(0, 5000).map((p, i) => {
        const [x, y] = project(p);
        return (
          <circle
            key={i}
            data-geometry="point"
            cx={x}
            cy={y}
            r={3.5}
            fill="var(--brand)"
            stroke="var(--surface)"
            strokeWidth={1}
          />
        );
      })}
    </>
  );
}

/** A small fitted drawing of a geometry (the grid's hover preview). */
export function GeometryPreview({
  g,
  width = 180,
  height = 120,
}: {
  g: ParsedGeometry;
  width?: number;
  height?: number;
}) {
  const b = boundsOf(g.shapes);
  if (!b || g.empty)
    return (
      <div
        className="text-muted-foreground flex items-center justify-center text-[11px] italic"
        style={{ width, height }}
      >
        {g.type} EMPTY
      </div>
    );
  const v = padded(b, 0.08, 1);
  const scale = Math.min(width / (v.maxX - v.minX), height / (v.maxY - v.minY));
  const ox = (width - (v.maxX - v.minX) * scale) / 2;
  const oy = (height - (v.maxY - v.minY) * scale) / 2;
  const project = ([x, y]: Pt): Pt => [ox + (x - v.minX) * scale, oy + (v.maxY - y) * scale];
  return (
    <svg
      role="img"
      aria-label={`${g.type} preview`}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
    >
      <Shapes g={g} project={project} />
    </svg>
  );
}

interface View {
  cx: number;
  cy: number;
  /** Pixels per world unit. */
  scale: number;
}

function fit(b: Bounds | null, w: number, h: number, geo: boolean): View {
  const v = b ? padded(b, 0.1, geo ? 0.01 : 1) : { minX: -180, maxX: 180, minY: -90, maxY: 90 };
  return {
    cx: (v.minX + v.maxX) / 2,
    cy: (v.minY + v.maxY) / 2,
    scale: Math.max(1e-12, Math.min(w / (v.maxX - v.minX), h / (v.maxY - v.minY))),
  };
}

/**
 * Quick Look ▸ Map: the geometry drawn over a plain coordinate grid (no map tiles — it works
 * offline). Scroll or the ± buttons zoom (around the pointer), a drag pans, Fit / double-click
 * resets. SRID 4326 (and geography) axes read lon / lat.
 */
export function MapView({
  g,
  typeName,
  className,
}: {
  g: ParsedGeometry;
  typeName?: string;
  className?: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 600, h: 360 });
  useEffect(() => {
    const el = host.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const w = Math.max(80, el.clientWidth);
      const h = Math.max(80, el.clientHeight);
      setSize((s) => (s.w === w && s.h === h ? s : { w, h }));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const geo = isGeographic(g.srid, typeName);
  const bounds = useMemo(() => boundsOf(g.shapes), [g]);
  const [view, setView] = useState<View | null>(null);
  const v = view ?? fit(bounds, size.w, size.h, geo);
  const project = ([x, y]: Pt): Pt => [
    size.w / 2 + (x - v.cx) * v.scale,
    size.h / 2 - (y - v.cy) * v.scale,
  ];
  const toWorld = (px: number, py: number): Pt => [
    v.cx + (px - size.w / 2) / v.scale,
    v.cy - (py - size.h / 2) / v.scale,
  ];

  const zoomAt = (factor: number, px = size.w / 2, py = size.h / 2) => {
    const [wx, wy] = toWorld(px, py);
    const scale = Math.max(1e-12, Math.min(1e12, v.scale * factor));
    // Keep the world point under (px, py) where it is.
    setView({
      scale,
      cx: wx - (px - size.w / 2) / scale,
      cy: wy + (py - size.h / 2) / scale,
    });
  };
  const onWheel = (e: ReactWheelEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    zoomAt(Math.exp(-e.deltaY * 0.0015), e.clientX - r.left, e.clientY - r.top);
  };
  const drag = useRef<{ x: number; y: number; view: View } | null>(null);
  const onPointerDown = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (e.button !== 0) return;
    drag.current = { x: e.clientX, y: e.clientY, view: v };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: ReactPointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d) return;
    setView({
      ...d.view,
      cx: d.view.cx - (e.clientX - d.x) / d.view.scale,
      cy: d.view.cy + (e.clientY - d.y) / d.view.scale,
    });
  };
  const endDrag = () => {
    drag.current = null;
  };

  // Grid lines over the visible extent.
  const [x0, y1] = toWorld(0, 0);
  const [x1, y0] = toWorld(size.w, size.h);
  const step = niceStep(Math.max(x1 - x0, (y1 - y0) * (size.w / size.h)));
  const lines = (lo: number, hi: number) => {
    const out: number[] = [];
    const first = Math.ceil(lo / step);
    for (let i = first; i * step <= hi && out.length < 200; i++) out.push(i * step);
    return out;
  };
  const xs = lines(x0, x1);
  const ys = lines(y0, y1);

  return (
    <div
      ref={host}
      data-testid="quick-look-map"
      className={cn(
        "border-border bg-surface relative min-h-0 flex-1 overflow-hidden rounded-md border",
        className,
      )}
    >
      <svg
        role="img"
        aria-label={`${g.type} on a ${geo ? "longitude / latitude" : "coordinate"} grid`}
        width={size.w}
        height={size.h}
        className="block cursor-grab touch-none select-none active:cursor-grabbing"
        onWheel={onWheel}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={() => setView(null)}
      >
        <g stroke="var(--border)" strokeWidth={1}>
          {xs.map((x) => {
            const [px] = project([x, 0]);
            return <line key={`x${x}`} x1={px} x2={px} y1={0} y2={size.h} />;
          })}
          {ys.map((y) => {
            const [, py] = project([0, y]);
            return <line key={`y${y}`} x1={0} x2={size.w} y1={py} y2={py} />;
          })}
        </g>
        <Shapes g={g} project={project} />
        <g className="fill-muted-foreground text-[10px] tabular-nums" style={{ fontSize: 10 }}>
          {xs.map((x) => {
            const [px] = project([x, 0]);
            return (
              <text key={`lx${x}`} x={px + 3} y={size.h - 4}>
                {axisLabel(x, step, "x", geo)}
              </text>
            );
          })}
          {ys.map((y) => {
            const [, py] = project([0, y]);
            return (
              <text key={`ly${y}`} x={4} y={py - 3}>
                {axisLabel(y, step, "y", geo)}
              </text>
            );
          })}
        </g>
      </svg>
      <div className="text-muted-foreground bg-surface/85 pointer-events-none absolute top-1.5 left-2 rounded px-1 text-[10px]">
        {geo ? "lon ↔ · lat ↕" : "x ↔ · y ↕"}
        {g.srid !== null && ` · SRID ${g.srid}`}
        {g.dims && ` · ${g.dims}`} · {pointCount(g.shapes).toLocaleString()} pts
      </div>
      <div className="absolute top-1.5 right-1.5 flex gap-0.5">
        <Button variant="outline" size="icon-xs" aria-label="Zoom in" onClick={() => zoomAt(1.5)}>
          <Plus />
        </Button>
        <Button
          variant="outline"
          size="icon-xs"
          aria-label="Zoom out"
          onClick={() => zoomAt(1 / 1.5)}
        >
          <Minus />
        </Button>
        <Button variant="outline" size="icon-xs" aria-label="Fit" onClick={() => setView(null)}>
          <Maximize />
        </Button>
      </div>
    </div>
  );
}
