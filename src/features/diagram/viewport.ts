import type { Point, Positions, Rect, Size } from "./layout";

/** Pan (`x`, `y`: screen px of the diagram origin) and zoom (`k`). */
export interface View {
  x: number;
  y: number;
  k: number;
}

export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 2.5;
/** One ⌘+ / ⌘− step. */
export const ZOOM_STEP = 1.2;
/** One arrow-key pan step (px), and with ⇧. */
export const PAN_STEP = 48;
export const PAN_STEP_LARGE = 240;

export const IDENTITY: View = { x: 0, y: 0, k: 1 };

export function clampZoom(k: number): number {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, k));
}

/** Zooms by `factor` keeping the diagram point under `anchor` (screen px) in place. */
export function zoomAt(view: View, factor: number, anchor: Point): View {
  const k = clampZoom(view.k * factor);
  const ratio = k / view.k;
  return {
    k,
    x: anchor.x - (anchor.x - view.x) * ratio,
    y: anchor.y - (anchor.y - view.y) * ratio,
  };
}

/** The view that shows `bounds` centred in `viewport` with `padding`, at most at `maxZoom`. */
export function fitView(bounds: Rect, viewport: Size, padding = 32, maxZoom = 1): View {
  if (bounds.w <= 0 || bounds.h <= 0 || viewport.w <= 0 || viewport.h <= 0)
    return { x: padding - bounds.x, y: padding - bounds.y, k: 1 };
  const k = clampZoom(
    Math.min(maxZoom, (viewport.w - padding * 2) / bounds.w, (viewport.h - padding * 2) / bounds.h),
  );
  return {
    k,
    x: (viewport.w - bounds.w * k) / 2 - bounds.x * k,
    y: (viewport.h - bounds.h * k) / 2 - bounds.y * k,
  };
}

/** Screen px → diagram coordinates. */
export function toDiagram(view: View, p: Point): Point {
  return { x: (p.x - view.x) / view.k, y: (p.y - view.y) / view.k };
}

/** The view that brings `rect` into the middle of `viewport` at the current zoom. */
export function centerOn(view: View, rect: Rect, viewport: Size): View {
  return {
    k: view.k,
    x: viewport.w / 2 - (rect.x + rect.w / 2) * view.k,
    y: viewport.h / 2 - (rect.y + rect.h / 2) * view.k,
  };
}

// --- remembered positions ---------------------------------------------------------------------

const STORAGE_PREFIX = "tabler.diagram.positions:";

/** Card positions are remembered per connection profile, database and schema. */
export function positionsKey(profileId: string, database: string | null, schema: string): string {
  return `${STORAGE_PREFIX}${profileId}\u0000${database ?? ""}\u0000${schema}`;
}

export function loadPositions(key: string): Positions | null {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    const out: Positions = {};
    for (const [name, p] of Object.entries(parsed as Record<string, unknown>)) {
      const { x, y } = (p ?? {}) as { x?: unknown; y?: unknown };
      if (typeof x === "number" && typeof y === "number" && Number.isFinite(x + y))
        out[name] = { x, y };
    }
    return out;
  } catch {
    return null;
  }
}

export function savePositions(key: string, positions: Positions): void {
  try {
    localStorage.setItem(key, JSON.stringify(positions));
  } catch {
    // Storage full or unavailable: the layout is simply not remembered.
  }
}

export function forgetPositions(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

// --- line style preference ---------------------------------------------------------------------

const LINE_STYLE_KEY = "tabler.diagram.lineStyle";

export function loadLineStyle(): "curved" | "orthogonal" {
  try {
    return localStorage.getItem(LINE_STYLE_KEY) === "orthogonal" ? "orthogonal" : "curved";
  } catch {
    return "curved";
  }
}

export function saveLineStyle(style: "curved" | "orthogonal"): void {
  try {
    localStorage.setItem(LINE_STYLE_KEY, style);
  } catch {
    // ignore
  }
}
