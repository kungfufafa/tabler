import { MAX_COL_W, MIN_COL_W, type ColumnState, type GridColumn } from "./types";

/** Every column id in display order (hidden ones included): `state.order` first, then the rest. */
export function effectiveOrder(allIds: readonly string[], state: ColumnState): string[] {
  const known = new Set(allIds);
  const out: string[] = [];
  const seen = new Set<string>();
  for (const id of state.order) {
    if (known.has(id) && !seen.has(id)) {
      out.push(id);
      seen.add(id);
    }
  }
  for (const id of allIds) {
    if (!seen.has(id)) {
      out.push(id);
      seen.add(id);
    }
  }
  return out;
}

/**
 * Moves `id` before `beforeId` (or to the end when null). The result's `order` lists ALL column ids,
 * hidden ones included, so a later "show" puts a column back where it was.
 */
export function moveColumn(
  allIds: readonly string[],
  state: ColumnState,
  id: string,
  beforeId: string | null,
): ColumnState {
  if (id === beforeId) return state;
  const order = effectiveOrder(allIds, state);
  if (!order.includes(id)) return state;
  const rest = order.filter((x) => x !== id);
  const at = beforeId === null ? -1 : rest.indexOf(beforeId);
  if (at < 0) rest.push(id);
  else rest.splice(at, 0, id);
  if (rest.every((x, i) => x === order[i])) return state;
  return { ...state, order: rest };
}

/** The id that follows `id` in display order (hidden columns included), or null at the end. */
export function idAfter(allIds: readonly string[], state: ColumnState, id: string): string | null {
  const order = effectiveOrder(allIds, state);
  const i = order.indexOf(id);
  return i >= 0 ? (order[i + 1] ?? null) : null;
}

export function hideColumn(state: ColumnState, id: string): ColumnState {
  if (state.hidden.includes(id)) return state;
  return { ...state, hidden: [...state.hidden, id] };
}

export function showAll(state: ColumnState): ColumnState {
  if (state.hidden.length === 0) return state;
  return { ...state, hidden: [] };
}

export function resetOrder(state: ColumnState): ColumnState {
  if (state.order.length === 0) return state;
  return { ...state, order: [] };
}

/** Hidden ids that still name a column (stale ids from an older schema are ignored). */
export function hiddenCount(columns: readonly GridColumn[], state: ColumnState): number {
  const hidden = new Set(state.hidden);
  return columns.filter((c) => hidden.has(c.id)).length;
}

const CHAR_W = 7.5;
const PADDING = 24;
/** Header extras: the type/key icon and its gap. */
const HEADER_EXTRA = 18;

/** Width that fits the header and the sampled cell texts (≈7.5px per char + padding), clamped. */
export function fitWidth(col: Pick<GridColumn, "name">, sampleTexts: readonly string[]): number {
  let chars = 0;
  for (const t of sampleTexts) if (t.length > chars) chars = t.length;
  const cells = chars * CHAR_W + PADDING;
  const header = col.name.length * CHAR_W + PADDING + HEADER_EXTRA;
  return Math.round(Math.max(MIN_COL_W, Math.min(MAX_COL_W, Math.max(cells, header))));
}
