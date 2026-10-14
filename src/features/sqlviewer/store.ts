import { create } from "zustand";
import type { SqlDialect } from "@/features/editor/scanner";

/**
 * File ▸ New SQL Viewer (TablePlus ⇧⌘O): a SQL text editor bound to no connection. It opens and
 * saves `.sql` files with syntax highlighting and never runs anything. A viewer is a tool tab of
 * the active workspace (keyed by its tab id) or, with no connection open, the window's standalone
 * viewer (`STANDALONE`).
 */
export const STANDALONE = "standalone";

export interface ViewerDoc {
  /** The file it shows; null for an untitled buffer. */
  path: string | null;
  /** Highlighting and the comment syntax (no completion against a server). */
  dialect: SqlDialect;
  /** The buffer differs from the file (or an untitled buffer has text). */
  dirty: boolean;
  /**
   * The doc holds only the head of this (big) file: it is a copy, `path` stays null so saving
   * can never overwrite the original. Null for everything else.
   */
  headOf: string | null;
  /** "Showing the first 5 MB of x.sql; saving here will not overwrite the file" (with `headOf`). */
  headNotice: string | null;
  /** The file is being read. */
  loading: boolean;
  /** Why the file could not be read. */
  error: string | null;
  /** Bumped when the text is replaced from outside the editor (a file opened into it). */
  revision: number;
  /** A file waiting to replace the buffer once the user discards its unsaved changes. */
  pendingPath: string | null;
}

interface SqlViewerState {
  docs: Readonly<Record<string, ViewerDoc>>;
  /** The standalone viewer is showing (no connection was open when it was asked for). */
  standaloneOpen: boolean;
}

export const useSqlViewerStore = create<SqlViewerState>()(() => ({
  docs: {},
  standaloneOpen: false,
}));

export function newDoc(path: string | null, dialect: SqlDialect = "postgres"): ViewerDoc {
  return {
    path,
    dialect,
    dirty: false,
    headOf: null,
    headNotice: null,
    loading: path !== null,
    error: null,
    revision: 0,
    pendingPath: null,
  };
}

export function getDoc(key: string): ViewerDoc | undefined {
  return useSqlViewerStore.getState().docs[key];
}

/** Creates the viewer's doc unless it exists; returns it. */
export function ensureDoc(key: string, init: () => ViewerDoc): ViewerDoc {
  const existing = getDoc(key);
  if (existing) return existing;
  const doc = init();
  useSqlViewerStore.setState((s) => ({ docs: { ...s.docs, [key]: doc } }));
  return doc;
}

export function patchDoc(key: string, patch: Partial<ViewerDoc>): void {
  useSqlViewerStore.setState((s) => {
    const cur = s.docs[key];
    if (!cur) return s;
    const next = { ...cur, ...patch };
    if ((Object.keys(patch) as (keyof ViewerDoc)[]).every((k) => cur[k] === next[k])) return s;
    return { docs: { ...s.docs, [key]: next } };
  });
}

export function dropDocs(keys: readonly string[]): void {
  if (keys.length === 0) return;
  useSqlViewerStore.setState((s) => {
    const docs = { ...s.docs };
    for (const k of keys) delete docs[k];
    return { docs };
  });
}

export function setStandaloneOpen(open: boolean): void {
  useSqlViewerStore.setState({ standaloneOpen: open });
}

/** Test helper. */
export function resetSqlViewerStore(): void {
  useSqlViewerStore.setState({ docs: {}, standaloneOpen: false });
}
