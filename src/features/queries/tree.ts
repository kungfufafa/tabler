import type { QueryFolder, SavedQuery } from "@/lib/ipc";

/** A row of the Queries pane: a folder (collapsible) or a saved query, indented by `depth`. */
export type QueryRow =
  | { kind: "folder"; folder: QueryFolder; depth: number; open: boolean; count: number }
  | { kind: "query"; query: SavedQuery; depth: number };

const byName = (a: { name: string }, b: { name: string }) =>
  a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });

/** Case-insensitive match of a query's name, keyword or SQL. */
export function queryMatches(q: SavedQuery, term: string): boolean {
  if (!term) return true;
  const t = term.toLowerCase();
  return (
    q.name.toLowerCase().includes(t) ||
    (q.keyword?.toLowerCase().includes(t) ?? false) ||
    q.sql.toLowerCase().includes(t)
  );
}

interface Index {
  childFolders: Map<string | null, QueryFolder[]>;
  childQueries: Map<string | null, SavedQuery[]>;
}

/** Children by parent; a missing parent (deleted meanwhile) counts as the top level. */
function index(folders: readonly QueryFolder[], queries: readonly SavedQuery[]): Index {
  const ids = new Set(folders.map((f) => f.id));
  const parentOf = (id: string | null) => (id !== null && ids.has(id) ? id : null);
  const childFolders = new Map<string | null, QueryFolder[]>();
  const childQueries = new Map<string | null, SavedQuery[]>();
  for (const f of folders) {
    const p = f.parentId === f.id ? null : parentOf(f.parentId);
    childFolders.set(p, [...(childFolders.get(p) ?? []), f]);
  }
  for (const q of queries) {
    const p = parentOf(q.folderId);
    childQueries.set(p, [...(childQueries.get(p) ?? []), q]);
  }
  for (const list of childFolders.values()) list.sort(byName);
  for (const list of childQueries.values()) list.sort(byName);
  return { childFolders, childQueries };
}

/**
 * The rows to show, depth-first: each level lists its folders, then its queries, by name. A
 * collapsed folder hides its contents. With a search term only matching queries are listed, in
 * their (expanded) folders; a folder whose name matches lists everything inside it.
 */
export function visibleRows(
  folders: readonly QueryFolder[],
  queries: readonly SavedQuery[],
  collapsed: ReadonlySet<string>,
  term = "",
): QueryRow[] {
  const { childFolders, childQueries } = index(folders, queries);
  const needle = term.trim().toLowerCase();
  const rows: QueryRow[] = [];
  const seen = new Set<string>();

  /** Rows of one folder's contents; `all` lists everything (no filtering). */
  const walk = (parent: string | null, depth: number, all: boolean): QueryRow[] => {
    const out: QueryRow[] = [];
    for (const f of childFolders.get(parent) ?? []) {
      if (seen.has(f.id)) continue; // a cycle in stored data
      seen.add(f.id);
      const nameHit = needle !== "" && f.name.toLowerCase().includes(needle);
      const inner = walk(f.id, depth + 1, all || nameHit);
      const count = countQueries(f.id, childFolders, childQueries);
      if (needle !== "" && !all && !nameHit && inner.length === 0) continue;
      const open = needle !== "" || !collapsed.has(f.id);
      out.push({ kind: "folder", folder: f, depth, open, count });
      if (open) out.push(...inner);
    }
    for (const q of childQueries.get(parent) ?? []) {
      if (all || queryMatches(q, needle)) out.push({ kind: "query", query: q, depth });
    }
    return out;
  };
  rows.push(...walk(null, 0, needle === ""));
  return rows;
}

function countQueries(
  id: string,
  childFolders: Map<string | null, QueryFolder[]>,
  childQueries: Map<string | null, SavedQuery[]>,
  seen = new Set<string>(),
): number {
  if (seen.has(id)) return 0;
  seen.add(id);
  let n = childQueries.get(id)?.length ?? 0;
  for (const f of childFolders.get(id) ?? [])
    n += countQueries(f.id, childFolders, childQueries, seen);
  return n;
}

/** Every folder, depth-first by name, for "Move to Folder" and the folder picker. */
export function folderOptions(
  folders: readonly QueryFolder[],
): { folder: QueryFolder; depth: number }[] {
  const { childFolders } = index(folders, []);
  const out: { folder: QueryFolder; depth: number }[] = [];
  const seen = new Set<string>();
  const walk = (parent: string | null, depth: number) => {
    for (const f of childFolders.get(parent) ?? []) {
      if (seen.has(f.id)) continue;
      seen.add(f.id);
      out.push({ folder: f, depth });
      walk(f.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

/** The folder and every folder inside it (a folder cannot move into one of these). */
export function folderSubtree(folders: readonly QueryFolder[], id: string): Set<string> {
  const out = new Set([id]);
  for (let grew = true; grew;) {
    grew = false;
    for (const f of folders) {
      if (f.parentId !== null && out.has(f.parentId) && !out.has(f.id)) {
        out.add(f.id);
        grew = true;
      }
    }
  }
  return out;
}
