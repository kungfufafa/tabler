import { create } from "zustand";
import type { SavedQuery } from "@/lib/ipc";

/** What the saved query popup edits (TablePlus's "Add to Queries" / "Edit" popup). */
export interface SavedQueryDraft {
  /** null: a new query. */
  id: string | null;
  name: string;
  keyword: string;
  sql: string;
  folderId: string | null;
  /** null: every connection. */
  profileId: string | null;
}

/** The connection the popup was opened from: the query can be kept to it. */
export interface DraftConnection {
  profileId: string;
  name: string;
}

export interface SavedQueryDialogRequest {
  /** Distinguishes successive openings. */
  seq: number;
  draft: SavedQueryDraft;
  connection: DraftConnection | null;
  /** Called with the stored query once it is saved. */
  onSaved?: (query: SavedQuery) => void;
}

let seq = 0;

export const useSavedQueryDialogStore = create<{ request: SavedQueryDialogRequest | null }>()(
  () => ({ request: null }),
);

/** Why a keyword cannot be saved (null when it can). */
export function keywordProblem(keyword: string): string | null {
  const k = keyword.trim();
  if (/\s/.test(k)) return "The keyword must be a single word.";
  if (k.length > 64) return "The keyword is longer than 64 characters.";
  return null;
}

/** A default name from the SQL: its first meaningful line, shortened. */
export function nameFromSql(sql: string): string {
  const line =
    sql
      .split(/\r?\n/)
      .map((l) => l.trim())
      .find((l) => l && !l.startsWith("--")) ?? "";
  const clean = line.replace(/\s+/g, " ").replace(/;$/, "");
  if (!clean) return "Untitled Query";
  return clean.length > 48 ? `${clean.slice(0, 47).trimEnd()}…` : clean;
}

/** "Add to Queries…": a new saved query of `sql` (the name defaults from it). */
export function openAddToQueries(opts: {
  sql: string;
  connection?: DraftConnection | null;
  folderId?: string | null;
  onSaved?: (query: SavedQuery) => void;
}): void {
  useSavedQueryDialogStore.setState({
    request: {
      seq: ++seq,
      draft: {
        id: null,
        name: opts.sql.trim() ? nameFromSql(opts.sql) : "",
        keyword: "",
        sql: opts.sql,
        folderId: opts.folderId ?? null,
        profileId: null,
      },
      connection: opts.connection ?? null,
      onSaved: opts.onSaved,
    },
  });
}

/** "Edit…": the popup on an existing query. */
export function openEditSavedQuery(query: SavedQuery, connection: DraftConnection | null): void {
  useSavedQueryDialogStore.setState({
    request: {
      seq: ++seq,
      draft: {
        id: query.id,
        name: query.name,
        keyword: query.keyword ?? "",
        sql: query.sql,
        folderId: query.folderId,
        profileId: query.profileId,
      },
      connection,
    },
  });
}

export function closeSavedQueryDialog(): void {
  useSavedQueryDialogStore.setState({ request: null });
}
