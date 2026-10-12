import { create } from "zustand";
import type { ResultColumn, Value } from "@/lib/ipc";

/**
 * Row buffers for SQL editor results. Rows stream in chunks of up to 500 and a result can hold
 * 10k+ rows, so they live outside React/zustand state in plain arrays that are appended to in
 * place. Components subscribe to a per-result version number instead, which is bumped at most
 * once per animation frame, so a large result streams without re-rendering on every chunk.
 */

export interface ResultBuffer {
  columns: ResultColumn[];
  rows: Value[][];
}

export class ResultStore {
  private readonly buffers = new Map<string, ResultBuffer>();

  constructor(private readonly onChange: (id: string) => void = () => {}) {}

  get(id: string): ResultBuffer | undefined {
    return this.buffers.get(id);
  }

  has(id: string): boolean {
    return this.buffers.has(id);
  }

  ids(): string[] {
    return [...this.buffers.keys()];
  }

  setColumns(id: string, columns: ResultColumn[]): void {
    const buf = this.buffers.get(id);
    if (buf) buf.columns = columns;
    else this.buffers.set(id, { columns, rows: [] });
    this.onChange(id);
  }

  append(id: string, rows: Value[][]): void {
    let buf = this.buffers.get(id);
    if (!buf) {
      buf = { columns: [], rows: [] };
      this.buffers.set(id, buf);
    }
    for (const row of rows) buf.rows.push(row);
    this.onChange(id);
  }

  rowCount(id: string): number {
    return this.buffers.get(id)?.rows.length ?? 0;
  }

  /** Replaces a buffer's rows (committed result edits), keeping its columns. */
  replaceRows(id: string, rows: Value[][]): void {
    const buf = this.buffers.get(id);
    if (!buf) return;
    buf.rows = rows;
    this.onChange(id);
  }

  /**
   * Copies buffer `from` into a new buffer `to` (a pinned result keeps its rows when the run that
   * produced them is replaced). Rows are copied shallowly: cells are never mutated in place.
   */
  copy(from: string, to: string): boolean {
    const buf = this.buffers.get(from);
    if (!buf) return false;
    this.buffers.set(to, { columns: buf.columns.slice(), rows: buf.rows.slice() });
    this.onChange(to);
    return true;
  }

  /** Drops every buffer whose id matches `pred` (e.g. all results of a previous run). */
  dropWhere(pred: (id: string) => boolean): string[] {
    const dropped: string[] = [];
    for (const id of this.buffers.keys()) {
      if (pred(id)) {
        this.buffers.delete(id);
        dropped.push(id);
      }
    }
    return dropped;
  }

  clear(): void {
    this.buffers.clear();
  }
}

// --- versions (the reactive part) -------------------------------------------

interface VersionState {
  versions: Record<string, number>;
}

export const useResultVersions = create<VersionState>()(() => ({ versions: {} }));

const pending = new Set<string>();
let scheduled = false;

const schedule: (fn: () => void) => void =
  typeof requestAnimationFrame === "function"
    ? (fn) => void requestAnimationFrame(() => fn())
    : (fn) => void setTimeout(fn, 16);

/** Applies every pending version bump now (called once per frame, or directly in tests). */
export function flushResultVersions(): void {
  scheduled = false;
  if (pending.size === 0) return;
  const ids = [...pending];
  pending.clear();
  useResultVersions.setState((s) => {
    const versions = { ...s.versions };
    for (const id of ids) versions[id] = (versions[id] ?? 0) + 1;
    return { versions };
  });
}

function bump(id: string): void {
  pending.add(id);
  if (scheduled) return;
  scheduled = true;
  schedule(flushResultVersions);
}

/** The app-wide result buffers. */
export const resultData = new ResultStore(bump);

/** Drops buffers (and their version counters) matching `pred`. */
export function dropResults(pred: (id: string) => boolean): void {
  const dropped = resultData.dropWhere(pred);
  if (dropped.length === 0) return;
  for (const id of dropped) pending.delete(id);
  useResultVersions.setState((s) => {
    const versions = { ...s.versions };
    for (const id of dropped) delete versions[id];
    return { versions };
  });
}

/** Re-renders the caller whenever the buffer `id` changed (at most once per frame). */
export function useResultVersion(id: string | null): number {
  return useResultVersions((s) => (id ? (s.versions[id] ?? 0) : 0));
}

/** Live row count of a buffer; re-renders as rows stream in. */
export function useResultRowCount(id: string | null): number {
  useResultVersion(id);
  return id ? resultData.rowCount(id) : 0;
}
