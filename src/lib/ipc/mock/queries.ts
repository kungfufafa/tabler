// Mock handlers for the saved queries of the sidebar's Queries pane: list_saved_queries,
// save_saved_query, delete_saved_query, save_query_folder, delete_query_folder. Mirrors the app DB
// rules in `src-tauri/src/appdb.rs` (validation, name order, folder cascade).
import type { QueryFolder } from "../bindings/QueryFolder";
import type { QueryFolderInput } from "../bindings/QueryFolderInput";
import type { SavedQuery } from "../bindings/SavedQuery";
import type { SavedQueryInput } from "../bindings/SavedQueryInput";
import type { SavedQueryLibrary } from "../bindings/SavedQueryLibrary";
import { mockFileText, mockWriteFile } from "./query";
import { mockError, registerMock } from "./transport";

let folders: QueryFolder[] = [];
let queries: SavedQuery[] = [];

const now = () => new Date().toISOString();
const byName = (a: { name: string; createdAt: string }, b: { name: string; createdAt: string }) =>
  a.name.toLowerCase().localeCompare(b.name.toLowerCase()) ||
  a.createdAt.localeCompare(b.createdAt);

function seed(): void {
  const at = "2026-01-05T09:00:00.000Z";
  folders = [
    { id: "f-reports", name: "Reports", parentId: null, createdAt: at },
    { id: "f-maintenance", name: "Maintenance", parentId: null, createdAt: at },
  ];
  const q = (
    id: string,
    name: string,
    sql: string,
    extra: Partial<SavedQuery> = {},
  ): SavedQuery => ({
    id,
    name,
    keyword: null,
    sql,
    folderId: null,
    profileId: null,
    createdAt: at,
    updatedAt: at,
    ...extra,
  });
  queries = [
    q(
      "q-recent-users",
      "Recent users",
      "select id, email, created_at\nfrom users\norder by created_at desc\nlimit 20;",
      {
        keyword: "recent",
      },
    ),
    q(
      "q-posts-per-user",
      "Posts per user",
      "select u.email, count(p.id) as posts\nfrom users u\nleft join posts p on p.user_id = u.id\ngroup by u.email\norder by posts desc;",
      { keyword: "ppu", folderId: "f-reports", profileId: "p-local-pg" },
    ),
    q("q-user-by-id", "User by id", "select * from users where id = :id;", {
      keyword: "uid",
      folderId: "f-reports",
    }),
    q(
      "q-pg-activity",
      "Active sessions",
      "select pid, usename, state, query\nfrom pg_stat_activity\nwhere state <> 'idle';",
      { folderId: "f-maintenance", profileId: "p-local-pg" },
    ),
    q(
      "q-table-sizes",
      "Table sizes (MySQL)",
      "select table_name, data_length + index_length as bytes\nfrom information_schema.tables\nwhere table_schema = database()\norder by bytes desc;",
      {
        folderId: "f-maintenance",
        profileId: "p-shop-mysql",
      },
    ),
  ];
}
seed();

/** Test helper: back to the seeded library, or an empty one. */
export function resetMockSavedQueries(opts: { empty?: boolean } = {}): void {
  seed();
  if (opts.empty) {
    folders = [];
    queries = [];
  }
}

function name(raw: unknown, what: string): string {
  const n = typeof raw === "string" ? raw.trim() : "";
  if (!n) mockError("INVALID_INPUT", `${what} name is required`);
  if ([...n].length > 200) mockError("INVALID_INPUT", `${what} name is longer than 200 characters`);
  return n;
}

function keyword(raw: unknown): string | null {
  const k = typeof raw === "string" ? raw.trim() : "";
  if (!k) return null;
  if (/\s/.test(k)) mockError("INVALID_INPUT", "the keyword must be a single word");
  if ([...k].length > 64) mockError("INVALID_INPUT", "the keyword is longer than 64 characters");
  return k;
}

/** The folder and every folder inside it. */
function subtree(id: string): Set<string> {
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

const clone = <T>(v: T): T => structuredClone(v);

registerMock("list_saved_queries", (): SavedQueryLibrary => ({
  folders: clone([...folders].sort(byName)),
  queries: clone([...queries].sort(byName)),
}));

registerMock("save_saved_query", ({ input }) => {
  const i = input as SavedQueryInput;
  const n = name(i.name, "Query");
  const k = keyword(i.keyword);
  if (i.folderId && !folders.some((f) => f.id === i.folderId))
    mockError("NOT_FOUND", "the folder no longer exists");
  const at = now();
  const existing = i.id ? queries.find((q) => q.id === i.id) : undefined;
  const saved: SavedQuery = {
    id: i.id ?? crypto.randomUUID(),
    name: n,
    keyword: k,
    // `keepSql` (rename / move): the stored SQL is the file's own, whatever the caller holds.
    sql: i.keepSql === true && existing ? existing.sql : i.sql,
    folderId: i.folderId,
    profileId: i.profileId,
    createdAt: existing?.createdAt ?? at,
    updatedAt: at,
  };
  queries = existing ? queries.map((q) => (q === existing ? saved : q)) : [...queries, saved];
  return clone(saved);
});

registerMock("delete_saved_query", ({ id }) => {
  queries = queries.filter((q) => q.id !== id);
  return null;
});

registerMock("save_query_folder", ({ input }) => {
  const i = input as QueryFolderInput;
  const n = name(i.name, "Folder");
  const id = i.id ?? crypto.randomUUID();
  if (i.parentId) {
    if (!folders.some((f) => f.id === i.parentId))
      mockError("NOT_FOUND", "the parent folder no longer exists");
    if (subtree(id).has(i.parentId))
      mockError("INVALID_INPUT", "a folder cannot be moved into itself");
  }
  const existing = folders.find((f) => f.id === id);
  const saved: QueryFolder = {
    id,
    name: n,
    parentId: i.parentId,
    createdAt: existing?.createdAt ?? now(),
  };
  folders = existing ? folders.map((f) => (f === existing ? saved : f)) : [...folders, saved];
  return clone(saved);
});

registerMock("delete_query_folder", ({ id }) => {
  const gone = subtree(String(id));
  queries = queries.filter((q) => q.folderId === null || !gone.has(q.folderId));
  folders = folders.filter((f) => !gone.has(f.id));
  return null;
});

// --- files (Import…, Add Folder to Favorites…, Show in Finder) --------------------------------

/** Mock folders on disk for Add Folder to Favorites… (path → query name → SQL). */
const DISK_FOLDERS: Record<string, Record<string, string>> = {
  "/Users/me/queries": {
    sample: "SELECT p.id, p.title FROM posts p ORDER BY p.id DESC LIMIT 20;",
    "orders by day": "SELECT date(created_at) AS day, count(*) FROM orders GROUP BY 1 ORDER BY 1;",
  },
};

/** The query name an imported file gets: its name without `.sql` / `.sql.gz`. */
function importName(path: string): string {
  const base = path.split(/[\\/]/).pop() ?? path;
  return base.replace(/\.gz$/i, "").replace(/\.sql$/i, "") || "Imported";
}

registerMock("import_saved_queries", ({ paths, folderId }) => {
  const folder = (folderId as string | null) ?? null;
  if (folder && !folders.some((f) => f.id === folder))
    mockError("NOT_FOUND", "the folder no longer exists");
  const at = now();
  const added: SavedQuery[] = [];
  for (const path of (paths as string[]) ?? []) {
    const text = mockFileText(path);
    if (text === undefined) mockError("IO", `${path}: No such file or directory`);
    const taken = queries.filter((q) => q.folderId === folder).map((q) => q.name.toLowerCase());
    let n = importName(path);
    for (let i = 2; taken.includes(n.toLowerCase()); i++) n = `${importName(path)} ${i}`;
    const q: SavedQuery = {
      id: crypto.randomUUID(),
      name: n,
      keyword: null,
      sql: text,
      folderId: folder,
      profileId: null,
      createdAt: at,
      updatedAt: at,
    };
    queries = [...queries, q];
    added.push(q);
  }
  return clone(added);
});

registerMock("add_query_folder_from_disk", ({ path, parentId }) => {
  const src = DISK_FOLDERS[String(path).replace(/\/+$/, "")];
  if (!src) mockError("INVALID_INPUT", `${String(path)} is not a folder`);
  const at = now();
  const parent = (parentId as string | null) ?? null;
  const base = String(path).split("/").filter(Boolean).pop() ?? "Imported";
  const siblings = folders.filter((f) => f.parentId === parent).map((f) => f.name.toLowerCase());
  let n = base;
  for (let i = 2; siblings.includes(n.toLowerCase()); i++) n = `${base} ${i}`;
  const folder: QueryFolder = { id: crypto.randomUUID(), name: n, parentId: parent, createdAt: at };
  folders = [...folders, folder];
  for (const [name, sql] of Object.entries(src))
    queries = [
      ...queries,
      {
        id: crypto.randomUUID(),
        name,
        keyword: null,
        sql,
        folderId: folder.id,
        profileId: null,
        createdAt: at,
        updatedAt: at,
      },
    ];
  return clone(folder);
});

/** The `.sql` file of a saved query: written to the mock file system so a wizard can read it. */
registerMock("saved_query_path", ({ id }) => {
  const q = queries.find((x) => x.id === id);
  if (!q) mockError("NOT_FOUND", "the query no longer exists");
  const path = `/Users/me/Library/Application Support/Tabler/Queries/${q.name}.sql`;
  mockWriteFile(path, q.sql);
  return path;
});

registerMock("reveal_saved_query", ({ id, folder }) => {
  if (id && !(folder ? folders : queries).some((x) => x.id === id))
    mockError("NOT_FOUND", "the query no longer exists");
  return null;
});
