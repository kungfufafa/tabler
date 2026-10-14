import { create } from "zustand";
import {
  ipc,
  toIpcError,
  type QueryFolder,
  type QueryFolderInput,
  type SavedQuery,
  type SavedQueryInput,
} from "@/lib/ipc";

/**
 * The saved queries (TablePlus's "Queries", formerly "Favorites") and their folders, mirrored from
 * the queries folder (`.sql` files, Settings ▸ Locations). Kept in a store rather than a query
 * cache because the SQL editor's completion source reads it outside React.
 */
interface SavedQueriesState {
  folders: QueryFolder[];
  queries: SavedQuery[];
  /** The first load finished (or failed). */
  loaded: boolean;
  error: string | null;
}

const EMPTY: SavedQueriesState = { folders: [], queries: [], loaded: false, error: null };

export const useSavedQueriesStore = create<SavedQueriesState>()(() => ({ ...EMPTY }));

let inflight: Promise<void> | null = null;

/** Reads the library from the app DB (again). Concurrent calls share one request. */
export function refreshSavedQueries(): Promise<void> {
  inflight ??= (async () => {
    try {
      const lib = await ipc.savedQueries.list();
      useSavedQueriesStore.setState({
        folders: lib.folders,
        queries: lib.queries,
        loaded: true,
        error: null,
      });
    } catch (e) {
      useSavedQueriesStore.setState({ loaded: true, error: toIpcError(e).message });
    } finally {
      inflight = null;
    }
  })();
  return inflight;
}

/** Loads the library once (the Queries pane and the editor's completion ask for it). */
export function ensureSavedQueries(): Promise<void> {
  const s = useSavedQueriesStore.getState();
  if (s.loaded && s.error === null) return Promise.resolve();
  return refreshSavedQueries();
}

function upsert<T extends { id: string }>(list: readonly T[], item: T): T[] {
  return list.some((x) => x.id === item.id)
    ? list.map((x) => (x.id === item.id ? item : x))
    : [...list, item];
}

/** Creates or replaces a saved query; the store follows at once. */
export async function putSavedQuery(input: SavedQueryInput): Promise<SavedQuery> {
  const saved = await ipc.savedQueries.save(input);
  useSavedQueriesStore.setState((s) => ({ queries: upsert(s.queries, saved) }));
  void refreshSavedQueries();
  return saved;
}

export async function deleteSavedQuery(id: string): Promise<void> {
  await ipc.savedQueries.delete(id);
  useSavedQueriesStore.setState((s) => ({ queries: s.queries.filter((q) => q.id !== id) }));
  void refreshSavedQueries();
}

/**
 * Renames a query. Only its name changes: the backend keeps the file's own SQL (`keepSql`), not
 * the copy this store holds, which can be stale (the file was edited elsewhere) or lossy (a file
 * that is not UTF-8).
 */
export function renameSavedQuery(query: SavedQuery, name: string): Promise<SavedQuery> {
  return putSavedQuery({ ...inputOf(query), name, keepSql: true });
}

/** Moves a query into `folderId` (null: the top level); its SQL is left as the file has it. */
export function moveSavedQuery(query: SavedQuery, folderId: string | null): Promise<SavedQuery> {
  return putSavedQuery({ ...inputOf(query), folderId, keepSql: true });
}

export async function putQueryFolder(input: QueryFolderInput): Promise<QueryFolder> {
  const saved = await ipc.savedQueries.saveFolder(input);
  // A folder's id follows its path: a renamed / moved one comes back with a new id.
  useSavedQueriesStore.setState((s) => ({
    folders: upsert(
      s.folders.filter((f) => f.id !== input.id),
      saved,
    ),
  }));
  void refreshSavedQueries();
  return saved;
}

/** "Import…": `.sql` / `.sql.gz` files become queries in `folderId` (null: the top level). */
export async function importSavedQueryFiles(
  paths: string[],
  folderId: string | null = null,
): Promise<SavedQuery[]> {
  const added = await ipc.savedQueries.importFiles(paths, folderId);
  await refreshSavedQueries();
  return added;
}

/** "Add Folder to Favorites…": a folder of `.sql` files is copied into the queries folder. */
export async function addQueryFolderFromDisk(
  path: string,
  parentId: string | null = null,
): Promise<QueryFolder> {
  const folder = await ipc.savedQueries.addFolderFromDisk(path, parentId);
  await refreshSavedQueries();
  return folder;
}

/** Show in Finder: a query's file, a folder, or (`null`) the queries folder. */
export function revealSavedQuery(
  target: { kind: "query" | "folder"; id: string } | null,
): Promise<void> {
  return ipc.savedQueries.reveal(target?.id ?? null, target?.kind === "folder");
}

/** Deletes a folder with everything inside it. */
export async function deleteQueryFolder(id: string): Promise<void> {
  await ipc.savedQueries.deleteFolder(id);
  await refreshSavedQueries();
}

/** The input that saves `query` unchanged. */
export function inputOf(query: SavedQuery): SavedQueryInput {
  return {
    id: query.id,
    name: query.name,
    keyword: query.keyword,
    sql: query.sql,
    folderId: query.folderId,
    profileId: query.profileId,
  };
}

/** The queries a connection sees: its own and those for every connection. */
export function queriesFor(
  queries: readonly SavedQuery[],
  profileId: string | null | undefined,
): SavedQuery[] {
  return queries.filter((q) => q.profileId === null || q.profileId === profileId);
}

/** `base`, or `base 2`, `base 3`… when a sibling already has that name. */
export function uniqueName(base: string, taken: Iterable<string>): string {
  const names = new Set([...taken].map((n) => n.toLowerCase()));
  if (!names.has(base.toLowerCase())) return base;
  for (let i = 2; ; i++) if (!names.has(`${base} ${i}`.toLowerCase())) return `${base} ${i}`;
}

/** Test helper: an empty, unloaded store. */
export function resetSavedQueriesStore(): void {
  inflight = null;
  useSavedQueriesStore.setState({ ...EMPTY });
}
