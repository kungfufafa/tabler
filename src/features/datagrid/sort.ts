import type { SortSpec } from "@/lib/ipc";

/**
 * Header click (TablePlus): descending → ascending → no sort. Shift-click adds the column as the
 * next sort key (or turns its existing key) and keeps the others.
 */
export function nextSort(sort: SortSpec[], column: string, additive: boolean): SortSpec[] {
  const existing = sort.find((s) => s.column === column);
  const rotated: SortSpec | null = !existing
    ? { column, dir: "desc", nulls: null }
    : existing.dir === "desc"
      ? { column, dir: "asc", nulls: null }
      : null;
  if (!additive) return rotated ? [rotated] : [];
  // The key keeps its place among the others while it turns.
  if (existing)
    return rotated
      ? sort.map((s) => (s.column === column ? rotated : s))
      : sort.filter((s) => s.column !== column);
  return [...sort, rotated!];
}

/**
 * Header menu ▸ Add to Sort ▸ ASC / DESC: `column` becomes the last sort key in `dir` (moved there
 * when it was a key already), after the current ones.
 */
export function addSort(sort: SortSpec[], column: string, dir: SortSpec["dir"]): SortSpec[] {
  return [...sort.filter((s) => s.column !== column), { column, dir, nulls: null }];
}
