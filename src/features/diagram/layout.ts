import type { DiagramRelation, DiagramTable } from "@/lib/ipc";

/** Card metrics (px, at 100 %). */
export const CARD = {
  header: 28,
  row: 20,
  padBottom: 6,
  padX: 10,
  /** The PK / FK icon slot left of a column name. */
  iconSlot: 16,
  /** Between a column's name and its type. */
  gap: 14,
  minWidth: 168,
  maxWidth: 340,
  /** Average glyph widths used to size cards without measuring text. */
  headerChar: 7.1,
  nameChar: 6.4,
  typeChar: 6.1,
} as const;

/** Spacing of the automatic layout. */
export const LAYOUT = {
  /** Between two layers (columns of cards). */
  layerGap: 96,
  /** Between two cards of a layer. */
  cardGap: 28,
  /** Between components (groups of related tables). */
  componentGap: 72,
  /** A layer taller than this wraps into several columns. */
  maxColumnHeight: 1800,
  /** Rows of components / unrelated tables wrap past this width. */
  minWrapWidth: 1600,
} as const;

export interface Point {
  x: number;
  y: number;
}

export interface Size {
  w: number;
  h: number;
}

export interface Rect extends Point, Size {}

/** Top-left corner of every card, by table name. */
export type Positions = Record<string, Point>;

export function cardHeight(columnCount: number): number {
  return CARD.header + Math.max(columnCount, 1) * CARD.row + CARD.padBottom;
}

/** Card size from its text, estimated from glyph counts (deterministic, no DOM). */
export function cardSize(table: DiagramTable): Size {
  let w = CARD.padX * 2 + CARD.iconSlot + table.ref.name.length * CARD.headerChar;
  for (const c of table.columns) {
    const row =
      CARD.padX * 2 +
      CARD.iconSlot +
      c.name.length * CARD.nameChar +
      CARD.gap +
      c.dataType.length * CARD.typeChar;
    if (row > w) w = row;
  }
  return {
    w: Math.round(Math.min(CARD.maxWidth, Math.max(CARD.minWidth, w))),
    h: cardHeight(table.columns.length),
  };
}

/** Parent → children edges between `names` (self references and duplicates left out). */
function edgesWithin(
  names: ReadonlySet<string>,
  relations: readonly DiagramRelation[],
  schema: string | null,
): Array<[string, string]> {
  const seen = new Set<string>();
  const out: Array<[string, string]> = [];
  for (const r of relations) {
    if (schema !== null && r.to.schema !== null && r.to.schema !== schema) continue;
    const parent = r.to.name;
    const child = r.from.name;
    if (parent === child || !names.has(parent) || !names.has(child)) continue;
    const key = `${parent}\u0000${child}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push([parent, child]);
  }
  return out.sort((a, b) => cmp(a[0], b[0]) || cmp(a[1], b[1]));
}

function cmp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Connected components, largest first (ties by their first name). */
function components(names: readonly string[], edges: Array<[string, string]>): string[][] {
  const parent = new Map(names.map((n) => [n, n]));
  const find = (n: string): string => {
    let root = n;
    while (parent.get(root) !== root) root = parent.get(root)!;
    parent.set(n, root);
    return root;
  };
  for (const [a, b] of edges) {
    const ra = find(a);
    const rb = find(b);
    if (ra !== rb) parent.set(ra < rb ? rb : ra, ra < rb ? ra : rb);
  }
  const groups = new Map<string, string[]>();
  for (const n of names) {
    const root = find(n);
    const g = groups.get(root);
    if (g) g.push(n);
    else groups.set(root, [n]);
  }
  return [...groups.values()].sort((a, b) => b.length - a.length || cmp(a[0]!, b[0]!));
}

/**
 * Longest-path layers (parents before their children) of an acyclic version of the graph: back
 * edges found by a name-ordered DFS are ignored, so cycles still get a deterministic layering.
 */
function layering(nodes: readonly string[], edges: Array<[string, string]>): Map<string, number> {
  const children = new Map<string, string[]>(nodes.map((n) => [n, []]));
  for (const [p, c] of edges) children.get(p)?.push(c);
  const state = new Map<string, 1 | 2>();
  const kept: Array<[string, string]> = [];
  const visit = (n: string) => {
    state.set(n, 1);
    for (const c of children.get(n) ?? []) {
      const s = state.get(c);
      if (s === 1) continue; // back edge: ignored
      kept.push([n, c]);
      if (s === undefined) visit(c);
    }
    state.set(n, 2);
  };
  for (const n of nodes) if (!state.has(n)) visit(n);

  const indeg = new Map<string, number>(nodes.map((n) => [n, 0]));
  const out = new Map<string, string[]>(nodes.map((n) => [n, []]));
  for (const [p, c] of kept) {
    out.get(p)!.push(c);
    indeg.set(c, indeg.get(c)! + 1);
  }
  const layer = new Map<string, number>(nodes.map((n) => [n, 0]));
  const queue = nodes.filter((n) => indeg.get(n) === 0);
  while (queue.length > 0) {
    const n = queue.shift()!;
    for (const c of out.get(n)!) {
      layer.set(c, Math.max(layer.get(c)!, layer.get(n)! + 1));
      const d = indeg.get(c)! - 1;
      indeg.set(c, d);
      if (d === 0) queue.push(c);
    }
  }
  return layer;
}

interface Placed {
  positions: Positions;
  w: number;
  h: number;
}

/** Layered layout of one connected component, at the origin. */
function layoutComponent(
  nodes: readonly string[],
  edges: Array<[string, string]>,
  sizes: ReadonlyMap<string, Size>,
): Placed {
  const layerOf = layering(nodes, edges);
  const depth = Math.max(...nodes.map((n) => layerOf.get(n)!)) + 1;
  const layers: string[][] = Array.from({ length: depth }, () => []);
  for (const n of nodes) layers[layerOf.get(n)!]!.push(n);

  const neighbours = new Map<string, string[]>(nodes.map((n) => [n, []]));
  for (const [p, c] of edges) {
    neighbours.get(p)!.push(c);
    neighbours.get(c)!.push(p);
  }

  // Barycenter ordering: a few down and up sweeps against the neighbouring layers.
  const rank = new Map<string, number>();
  const setRanks = (layer: string[]) =>
    layer.forEach((n, i) => rank.set(n, layer.length > 1 ? i / (layer.length - 1) : 0.5));
  layers.forEach(setRanks);
  const sweep = (i: number, ref: (n: string) => boolean) => {
    const layer = layers[i]!;
    const key = new Map<string, number>();
    for (const n of layer) {
      const near = neighbours.get(n)!.filter(ref);
      key.set(
        n,
        near.length > 0 ? near.reduce((s, m) => s + rank.get(m)!, 0) / near.length : rank.get(n)!,
      );
    }
    layer.sort((a, b) => key.get(a)! - key.get(b)! || cmp(a, b));
    setRanks(layer);
  };
  for (let iter = 0; iter < 4; iter++) {
    for (let i = 1; i < depth; i++) sweep(i, (m) => layerOf.get(m)! < i);
    for (let i = depth - 2; i >= 0; i--) sweep(i, (m) => layerOf.get(m)! > i);
  }

  // Coordinates: each layer is a column (tall layers wrap into several); a card sits as close as
  // the cards above it allow to the middle of its already placed neighbours.
  const positions: Positions = {};
  let x = 0;
  let height = 0;
  for (const layer of layers) {
    const columns: string[][] = [[]];
    let columnHeight = 0;
    for (const n of layer) {
      const h = sizes.get(n)!.h;
      const current = columns[columns.length - 1]!;
      if (current.length > 0 && columnHeight + h > LAYOUT.maxColumnHeight) {
        columns.push([n]);
        columnHeight = h + LAYOUT.cardGap;
      } else {
        current.push(n);
        columnHeight += h + LAYOUT.cardGap;
      }
    }
    for (const column of columns) {
      const width = Math.max(...column.map((n) => sizes.get(n)!.w));
      let bottom = 0;
      let first = true;
      for (const n of column) {
        const { h } = sizes.get(n)!;
        const placed = neighbours.get(n)!.filter((m) => positions[m] !== undefined);
        const desired =
          placed.length > 0
            ? placed.reduce((s, m) => s + positions[m]!.y + sizes.get(m)!.h / 2, 0) /
                placed.length -
              h / 2
            : bottom;
        const y = Math.round(Math.max(first ? 0 : bottom, desired, 0));
        positions[n] = { x, y };
        bottom = y + h + LAYOUT.cardGap;
        height = Math.max(height, y + h);
        first = false;
      }
      x += width + LAYOUT.layerGap;
    }
  }
  return { positions, w: x - LAYOUT.layerGap, h: height };
}

/** Rows of boxes left to right, wrapping past `wrap` (shelf packing). */
function shelve(boxes: Placed[], wrap: number, gap: number, top: number): Placed {
  const positions: Positions = {};
  let x = 0;
  let y = top;
  let rowHeight = 0;
  let w = 0;
  for (const box of boxes) {
    if (x > 0 && x + box.w > wrap) {
      x = 0;
      y += rowHeight + gap;
      rowHeight = 0;
    }
    for (const [name, p] of Object.entries(box.positions))
      positions[name] = { x: p.x + x, y: p.y + y };
    x += box.w + gap;
    w = Math.max(w, x - gap);
    rowHeight = Math.max(rowHeight, box.h);
  }
  return { positions, w, h: boxes.length > 0 ? y + rowHeight - top : 0 };
}

/**
 * Automatic layout: related tables in layers (referenced tables left of the tables pointing at
 * them), one block per group of related tables, unrelated tables in rows underneath. Deterministic:
 * the same tables and relations always give the same positions, whatever their input order.
 */
export function autoLayout(
  tables: readonly DiagramTable[],
  relations: readonly DiagramRelation[],
  schema: string | null = null,
): Positions {
  const sorted = [...tables].sort((a, b) => cmp(a.ref.name, b.ref.name));
  const names = sorted.map((t) => t.ref.name);
  const sizes = new Map(sorted.map((t) => [t.ref.name, cardSize(t)]));
  const edges = edgesWithin(new Set(names), relations, schema);
  const groups = components(names, edges);

  const blocks = groups
    .filter((g) => g.length > 1)
    .map((g) => {
      const members = new Set(g);
      return layoutComponent(
        g,
        edges.filter(([p]) => members.has(p)),
        sizes,
      );
    });
  const singles = groups
    .filter((g) => g.length === 1)
    .map((g): Placed => {
      const s = sizes.get(g[0]!)!;
      return { positions: { [g[0]!]: { x: 0, y: 0 } }, w: s.w, h: s.h };
    });

  const wrap = Math.max(LAYOUT.minWrapWidth, ...blocks.map((b) => b.w));
  const related = shelve(blocks, wrap, LAYOUT.componentGap, 0);
  const top = related.h > 0 ? related.h + LAYOUT.componentGap : 0;
  const unrelated = shelve(singles, wrap, LAYOUT.cardGap * 2, top);
  return { ...related.positions, ...unrelated.positions };
}

/**
 * Positions for `tables`: the remembered ones, plus an automatic layout of the tables without
 * one, placed underneath the remembered cards.
 */
export function placeTables(
  tables: readonly DiagramTable[],
  relations: readonly DiagramRelation[],
  schema: string | null,
  saved: Positions | null,
): Positions {
  const known = tables.filter((t) => saved?.[t.ref.name] !== undefined);
  if (!saved || known.length === 0) return autoLayout(tables, relations, schema);
  const positions: Positions = {};
  for (const t of known) positions[t.ref.name] = { ...saved[t.ref.name]! };
  const missing = tables.filter((t) => saved[t.ref.name] === undefined);
  if (missing.length === 0) return positions;
  const bounds = boundsOf(known, positions);
  const fresh = autoLayout(missing, relations, schema);
  for (const [name, p] of Object.entries(fresh))
    positions[name] = { x: p.x + bounds.x, y: p.y + bounds.y + bounds.h + LAYOUT.componentGap };
  return positions;
}

/** The rectangles of `tables` at `positions` (tables without a position are left out). */
export function rectsOf(tables: readonly DiagramTable[], positions: Positions): Map<string, Rect> {
  const out = new Map<string, Rect>();
  for (const t of tables) {
    const p = positions[t.ref.name];
    if (p) out.set(t.ref.name, { ...p, ...cardSize(t) });
  }
  return out;
}

/** Bounding box of the cards of `tables` (an empty box at the origin when there are none). */
export function boundsOf(tables: readonly DiagramTable[], positions: Positions): Rect {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const r of rectsOf(tables, positions).values()) {
    x0 = Math.min(x0, r.x);
    y0 = Math.min(y0, r.y);
    x1 = Math.max(x1, r.x + r.w);
    y1 = Math.max(y1, r.y + r.h);
  }
  if (x0 === Infinity) return { x: 0, y: 0, w: 0, h: 0 };
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

export function intersects(a: Rect, b: Rect, gap = 0): boolean {
  return (
    a.x < b.x + b.w + gap && b.x < a.x + a.w + gap && a.y < b.y + b.h + gap && b.y < a.y + a.h + gap
  );
}
