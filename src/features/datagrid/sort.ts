import type { SortSpec } from "@/lib/ipc";

/** Click: asc → desc → none; shift-click adds/rotates a secondary sort. */
export function nextSort(sort: SortSpec[], column: string, additive: boolean): SortSpec[] {
  const existing = sort.find((s) => s.column === column);
  const rotated: SortSpec | null = !existing
    ? { column, dir: "asc", nulls: null }
    : existing.dir === "asc"
      ? { column, dir: "desc", nulls: null }
      : null;
  if (!additive) return rotated ? [rotated] : [];
  const others = sort.filter((s) => s.column !== column);
  return rotated ? [...others, rotated] : others;
}
