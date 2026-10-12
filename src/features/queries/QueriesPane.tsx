import { useQueryClient } from "@tanstack/react-query";
import {
  BookmarkPlus,
  ChevronRight,
  Copy,
  FileCode2,
  Folder,
  FolderInput,
  FolderOpen,
  FolderPlus,
  Pencil,
  Play,
  Plus,
  Settings,
  SquarePen,
  SquareStack,
  TextCursorInput,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SidebarSearch } from "@/features/sidebar/SidebarSearch";
import { toIpcError, type QueryFolder, type SavedQuery } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import type { OpenConnection } from "@/stores/connections";
import { setPref, usePrefs } from "@/stores/prefs";
import { openAddToQueries, openEditSavedQuery } from "./dialogStore";
import { copySavedQuery, insertSavedQuery, openSavedQuery, runSavedQuery } from "./open";
import {
  deleteQueryFolder,
  deleteSavedQuery,
  ensureSavedQueries,
  inputOf,
  moveSavedQuery,
  putQueryFolder,
  putSavedQuery,
  queriesFor,
  uniqueName,
  useSavedQueriesStore,
} from "./store";
import { folderOptions, folderSubtree, visibleRows, type QueryRow } from "./tree";

/** localStorage key of the collapsed folders (a list of folder ids). */
export const COLLAPSED_FOLDERS_KEY = "tabler.queries.collapsed";
/** Drag payloads: a saved query or a folder id. */
const DRAG_QUERY = "application/x-tabler-saved-query";
const DRAG_FOLDER = "application/x-tabler-query-folder";

function readCollapsed(): Set<string> {
  try {
    const raw = globalThis.localStorage?.getItem(COLLAPSED_FOLDERS_KEY);
    const list: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(list) ? list.filter((x) => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function writeCollapsed(ids: ReadonlySet<string>): void {
  try {
    globalThis.localStorage?.setItem(COLLAPSED_FOLDERS_KEY, JSON.stringify([...ids]));
  } catch {
    // Not persisted.
  }
}

type Renaming = { kind: "query" | "folder"; id: string } | null;
type PendingDelete =
  | { kind: "query"; query: SavedQuery }
  | { kind: "folder"; folder: QueryFolder; count: number }
  | null;

const rowKey = (r: QueryRow) => (r.kind === "folder" ? `f:${r.folder.id}` : `q:${r.query.id}`);
const INDENT = 12;

/** The inline name editor of a row (Rename…, New Folder…): ↩ / blur keeps it, Esc cancels. */
function RenameInput({
  initial,
  onDone,
}: {
  initial: string;
  onDone: (name: string | null) => void;
}) {
  const [value, setValue] = useState(initial);
  const done = useRef(false);
  const finish = (name: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(name);
  };
  return (
    <input
      autoFocus
      aria-label="Name"
      value={value}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setValue(e.target.value)}
      onBlur={() => finish(value)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(value);
        else if (e.key === "Escape") finish(null);
      }}
      onClick={(e) => e.stopPropagation()}
      className="bg-background border-ring h-4.5 min-w-0 flex-1 rounded-sm border px-1 text-xs outline-none"
    />
  );
}

/**
 * The sidebar's Queries pane (TablePlus's "Queries", formerly "Favorites"): the saved queries of
 * the active connection and of every connection, in folders. Double-click (or ↩) runs one in a
 * new tab, or inserts it into the SQL editor with ⚙ ▸ Double Click to Insert; right-click for the
 * rest. Queries and folders can be dragged into folders.
 */
export function QueriesPane({ connection }: { connection: OpenConnection }) {
  const qc = useQueryClient();
  const allQueries = useSavedQueriesStore((s) => s.queries);
  const folders = useSavedQueriesStore((s) => s.folders);
  const loaded = useSavedQueriesStore((s) => s.loaded);
  const error = useSavedQueriesStore((s) => s.error);
  const doubleClickInserts = usePrefs((p) => p.queriesDoubleClickInsert);
  const [search, setSearch] = useState("");
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const [renaming, setRenaming] = useState<Renaming>(null);
  const [pendingDelete, setPendingDelete] = useState<PendingDelete>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  /** A menu item starts an inline rename: the closing menu must not take the focus back. */
  const keepFocus = useRef(false);
  const onMenuCloseAutoFocus = (e: Event) => {
    if (!keepFocus.current) return;
    keepFocus.current = false;
    e.preventDefault();
  };
  const startRename = (target: NonNullable<Renaming>) => {
    keepFocus.current = true;
    setRenaming(target);
  };
  const { profileId, name: connectionName } = connection.info;
  const draftConnection = useMemo(
    () => ({ profileId, name: connectionName }),
    [profileId, connectionName],
  );

  useEffect(() => void ensureSavedQueries(), []);

  const queries = useMemo(() => queriesFor(allQueries, profileId), [allQueries, profileId]);
  const rows = useMemo(
    () => visibleRows(folders, queries, collapsed, search),
    [folders, queries, collapsed, search],
  );
  const moveTargets = useMemo(() => folderOptions(folders), [folders]);

  const fail = (what: string) => (e: unknown) =>
    toast.error(what, { description: toIpcError(e).message });

  const setFolderOpen = (id: string, open: boolean) => {
    setCollapsed((prev) => {
      if (prev.has(id) !== open) return prev;
      const next = new Set(prev);
      if (open) next.delete(id);
      else next.add(id);
      writeCollapsed(next);
      return next;
    });
  };

  const activate = (q: SavedQuery) => {
    if (doubleClickInserts) insertSavedQuery(connection, q);
    else runSavedQuery(qc, connection, q);
  };

  const newQuery = (folderId: string | null = null) =>
    openAddToQueries({ sql: "", connection: draftConnection, folderId });

  const newFolder = (parentId: string | null = null) => {
    keepFocus.current = true;
    const siblings = folders.filter((f) => f.parentId === parentId).map((f) => f.name);
    void putQueryFolder({ id: null, name: uniqueName("New Folder", siblings), parentId }).then(
      (f) => {
        if (parentId) setFolderOpen(parentId, true);
        setRenaming({ kind: "folder", id: f.id });
      },
      fail("Could not create the folder"),
    );
  };

  const rename = (target: NonNullable<Renaming>, name: string | null) => {
    setRenaming(null);
    const trimmed = name?.trim();
    if (!trimmed) return;
    if (target.kind === "query") {
      const q = allQueries.find((x) => x.id === target.id);
      if (q && q.name !== trimmed)
        void putSavedQuery({ ...inputOf(q), name: trimmed }).catch(fail("Could not rename"));
    } else {
      const f = folders.find((x) => x.id === target.id);
      if (f && f.name !== trimmed)
        void putQueryFolder({ id: f.id, name: trimmed, parentId: f.parentId }).catch(
          fail("Could not rename"),
        );
    }
  };

  const moveQuery = (q: SavedQuery, folderId: string | null) => {
    if (q.folderId === folderId) return;
    if (folderId) setFolderOpen(folderId, true);
    void moveSavedQuery(q, folderId).catch(fail("Could not move the query"));
  };

  const moveFolder = (f: QueryFolder, parentId: string | null) => {
    if (f.parentId === parentId) return;
    if (parentId && folderSubtree(folders, f.id).has(parentId)) {
      toast.message("A folder cannot be moved into itself");
      return;
    }
    if (parentId) setFolderOpen(parentId, true);
    void putQueryFolder({ id: f.id, name: f.name, parentId }).catch(
      fail("Could not move the folder"),
    );
  };

  const confirmDelete = () => {
    const target = pendingDelete;
    setPendingDelete(null);
    if (!target) return;
    if (target.kind === "query")
      void deleteSavedQuery(target.query.id).catch(fail("Could not delete the query"));
    else void deleteQueryFolder(target.folder.id).catch(fail("Could not delete the folder"));
  };

  // --- keyboard ---------------------------------------------------------------------------

  const rowEls = () =>
    Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-query-row]") ?? []);
  const focusRow = (index: number) => {
    const els = rowEls();
    els[Math.max(0, Math.min(els.length - 1, index))]?.focus();
  };

  const onRowKeyDown = (ev: KeyboardEvent<HTMLDivElement>, row: QueryRow) => {
    if (renaming) return;
    const mod = ev.metaKey || ev.ctrlKey;
    const els = rowEls();
    const i = els.indexOf(ev.currentTarget);
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      if (ev.key === "ArrowUp" && i <= 0) searchRef.current?.focus();
      else focusRow(i + (ev.key === "ArrowDown" ? 1 : -1));
    } else if (ev.key === "ArrowRight" && row.kind === "folder") {
      ev.preventDefault();
      if (!row.open) setFolderOpen(row.folder.id, true);
      else focusRow(i + 1);
    } else if (ev.key === "ArrowLeft") {
      ev.preventDefault();
      if (row.kind === "folder" && row.open) setFolderOpen(row.folder.id, false);
      else {
        // To the enclosing folder.
        for (let j = rows.indexOf(row) - 1; j >= 0; j--) {
          const r = rows[j]!;
          if (r.kind === "folder" && r.depth < row.depth) return focusRow(j);
        }
      }
    } else if (ev.key === "Enter" && !mod) {
      ev.preventDefault();
      if (row.kind === "folder") setFolderOpen(row.folder.id, !row.open);
      else activate(row.query);
    } else if (ev.key === "Enter" && mod && row.kind === "query") {
      ev.preventDefault();
      runSavedQuery(qc, connection, row.query);
    } else if (mod && ev.key.toLowerCase() === "c" && row.kind === "query") {
      ev.preventDefault();
      copySavedQuery(row.query);
    } else if (ev.key === "F2" || (mod && ev.key.toLowerCase() === "r")) {
      ev.preventDefault();
      setRenaming(
        row.kind === "folder"
          ? { kind: "folder", id: row.folder.id }
          : { kind: "query", id: row.query.id },
      );
    } else if (ev.key === "Delete" || (mod && ev.key === "Backspace")) {
      ev.preventDefault();
      setPendingDelete(
        row.kind === "query"
          ? { kind: "query", query: row.query }
          : { kind: "folder", folder: row.folder, count: row.count },
      );
    }
  };

  // --- drag and drop --------------------------------------------------------------------------

  const dragKind = (e: DragEvent) =>
    e.dataTransfer.types.includes(DRAG_QUERY)
      ? "query"
      : e.dataTransfer.types.includes(DRAG_FOLDER)
        ? "folder"
        : null;

  const onDragOver = (e: DragEvent, target: string | null) => {
    if (!dragKind(e)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = "move";
    setDropTarget(target ?? "");
  };

  const onDrop = (e: DragEvent, target: string | null) => {
    const kind = dragKind(e);
    if (!kind) return;
    e.preventDefault();
    e.stopPropagation();
    setDropTarget(null);
    if (kind === "query") {
      const q = allQueries.find((x) => x.id === e.dataTransfer.getData(DRAG_QUERY));
      if (q) moveQuery(q, target);
    } else {
      const f = folders.find((x) => x.id === e.dataTransfer.getData(DRAG_FOLDER));
      if (f && f.id !== target) moveFolder(f, target);
    }
  };

  // --- rows -----------------------------------------------------------------------------------

  const folderRow = (row: Extract<QueryRow, { kind: "folder" }>) => {
    const f = row.folder;
    const editing = renaming?.kind === "folder" && renaming.id === f.id;
    return (
      <ContextMenu key={rowKey(row)}>
        <ContextMenuTrigger asChild>
          <div
            role="treeitem"
            aria-expanded={row.open}
            aria-level={row.depth + 1}
            aria-label={f.name}
            tabIndex={0}
            data-query-row
            draggable={!editing}
            onDragStart={(e) => {
              e.dataTransfer.setData(DRAG_FOLDER, f.id);
              e.dataTransfer.effectAllowed = "move";
            }}
            onDragEnd={() => setDropTarget(null)}
            onDragOver={(e) => onDragOver(e, f.id)}
            onDragLeave={() => setDropTarget((t) => (t === f.id ? null : t))}
            onDrop={(e) => onDrop(e, f.id)}
            onClick={() => !editing && setFolderOpen(f.id, !row.open)}
            onKeyDown={(e) => onRowKeyDown(e, row)}
            style={{ paddingLeft: 6 + row.depth * INDENT }}
            className={cn(
              "hover:bg-sidebar-accent/60 focus-visible:bg-sidebar-accent flex h-5.5 w-full cursor-default items-center gap-1 pr-2.5 text-left text-xs outline-none select-none",
              dropTarget === f.id && "bg-brand/15 ring-brand/50 ring-1 ring-inset",
            )}
          >
            <ChevronRight
              aria-hidden
              className={cn(
                "text-muted-foreground size-3 shrink-0 transition-transform",
                row.open && "rotate-90",
              )}
            />
            {row.open ? (
              <FolderOpen aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
            ) : (
              <Folder aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
            )}
            {editing ? (
              <RenameInput
                initial={f.name}
                onDone={(n) => rename({ kind: "folder", id: f.id }, n)}
              />
            ) : (
              <span className="min-w-0 flex-1 truncate">{f.name}</span>
            )}
            {!editing && row.count > 0 && (
              <span className="text-muted-foreground/80 shrink-0 text-[10px] tabular-nums">
                {row.count}
              </span>
            )}
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent className="w-52" onCloseAutoFocus={onMenuCloseAutoFocus}>
          <ContextMenuItem onSelect={() => newQuery(f.id)}>
            <BookmarkPlus /> New Query…
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => newFolder(f.id)}>
            <FolderPlus /> New Folder…
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => startRename({ kind: "folder", id: f.id })}>
            <Pencil /> Rename…
          </ContextMenuItem>
          <MoveMenu
            current={f.parentId}
            options={moveTargets}
            exclude={folderSubtree(folders, f.id)}
            onMove={(target) => moveFolder(f, target)}
          />
          <ContextMenuSeparator />
          <ContextMenuItem
            variant="destructive"
            onSelect={() => setPendingDelete({ kind: "folder", folder: f, count: row.count })}
          >
            <Trash2 /> Delete Folder
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    );
  };

  const queryRow = (row: Extract<QueryRow, { kind: "query" }>) => {
    const q = row.query;
    const editing = renaming?.kind === "query" && renaming.id === q.id;
    return (
      <ContextMenu key={rowKey(row)}>
        <ContextMenuTrigger asChild>
          <div
            role="treeitem"
            aria-level={row.depth + 1}
            aria-label={q.name}
            tabIndex={0}
            data-query-row
            title={q.sql.length > 600 ? `${q.sql.slice(0, 600)}…` : q.sql}
            draggable={!editing}
            onDragStart={(e) => {
              e.dataTransfer.setData(DRAG_QUERY, q.id);
              e.dataTransfer.setData("text/plain", q.sql);
              e.dataTransfer.effectAllowed = "copyMove";
            }}
            onDragEnd={() => setDropTarget(null)}
            // Dropping on a query puts it next to it, in the same folder.
            onDragOver={(e) => onDragOver(e, q.folderId)}
            onDrop={(e) => onDrop(e, q.folderId)}
            onDoubleClick={() => !editing && activate(q)}
            onKeyDown={(e) => onRowKeyDown(e, row)}
            style={{ paddingLeft: 6 + row.depth * INDENT + 16 }}
            className="hover:bg-sidebar-accent/60 focus-visible:bg-sidebar-accent flex h-5.5 w-full cursor-default items-center gap-1.5 pr-2.5 text-left text-xs outline-none select-none"
          >
            <FileCode2 aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
            {editing ? (
              <RenameInput
                initial={q.name}
                onDone={(n) => rename({ kind: "query", id: q.id }, n)}
              />
            ) : (
              <span className="min-w-0 flex-1 truncate">{q.name}</span>
            )}
            {!editing && q.keyword && (
              <span
                className="text-muted-foreground border-border shrink-0 rounded border px-1 font-mono text-[10px] leading-3.5"
                title={`Keyword: type “${q.keyword}” in the SQL editor`}
              >
                {q.keyword}
              </span>
            )}
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent className="w-56" onCloseAutoFocus={onMenuCloseAutoFocus}>
          <ContextMenuItem onSelect={() => runSavedQuery(qc, connection, q)}>
            <Play /> Run
            <ContextMenuShortcut>{formatCombo("Mod+Enter")}</ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => openSavedQuery(connection, q)}>
            <SquareStack /> Open in New Tab
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => insertSavedQuery(connection, q)}>
            <TextCursorInput /> Insert to SQL Editor
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => copySavedQuery(q)}>
            <Copy /> Copy
            <ContextMenuShortcut>{formatCombo("Mod+C")}</ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => openEditSavedQuery(q, draftConnection)}>
            <SquarePen /> Edit…
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => startRename({ kind: "query", id: q.id })}>
            <Pencil /> Rename…
          </ContextMenuItem>
          <MoveMenu
            current={q.folderId}
            options={moveTargets}
            onMove={(target) => moveQuery(q, target)}
          />
          <ContextMenuSeparator />
          <ContextMenuItem
            variant="destructive"
            onSelect={() => setPendingDelete({ kind: "query", query: q })}
          >
            <Trash2 /> Delete
            <ContextMenuShortcut>{formatCombo("Mod+Backspace")}</ContextMenuShortcut>
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    );
  };

  let body;
  if (error && allQueries.length === 0) {
    body = <EmptyState title="Could not load queries" description={error} />;
  } else if (!loaded) {
    body = null;
  } else if (rows.length === 0) {
    body = search.trim() ? (
      <EmptyState title="No matches" description={`No saved query contains “${search.trim()}”.`} />
    ) : (
      <EmptyState
        icon={BookmarkPlus}
        title="No saved queries"
        description={`Save SQL with Add to Queries… (${formatCombo("Mod+Alt+S")}) in the SQL editor, or New ▸ Query….`}
      />
    );
  } else {
    body = rows.map((r) => (r.kind === "folder" ? folderRow(r) : queryRow(r)));
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1 px-2 pb-1.5">
        <SidebarSearch
          ref={searchRef}
          value={search}
          onChange={setSearch}
          onArrowDown={() => focusRow(0)}
          placeholder="Search queries…"
          label="Search queries"
          kbdHint={false}
        />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="New"
              title="New query or folder"
              className="text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground focus-visible:ring-ring/50 data-[state=open]:bg-sidebar-accent flex size-6.5 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2"
            >
              <Plus className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            className="w-40 text-xs"
            onCloseAutoFocus={onMenuCloseAutoFocus}
          >
            <DropdownMenuItem onSelect={() => newQuery()}>
              <BookmarkPlus /> Query…
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => newFolder()}>
              <FolderPlus /> Folder…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Queries options"
              title="Options"
              className="text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground focus-visible:ring-ring/50 data-[state=open]:bg-sidebar-accent flex size-6.5 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2"
            >
              <Settings className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-56 text-xs">
            <DropdownMenuCheckboxItem
              checked={doubleClickInserts}
              onCheckedChange={(v) => setPref("queriesDoubleClickInsert", v === true)}
            >
              Double Click to Insert
            </DropdownMenuCheckboxItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <div
        ref={listRef}
        role="tree"
        aria-label="Saved queries"
        className={cn(
          "relative min-h-0 flex-1 overflow-auto pb-1",
          dropTarget === "" && "bg-brand/5",
        )}
        // Dropping on the empty part of the list moves to the top level.
        onDragOver={(e) => onDragOver(e, null)}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropTarget(null);
        }}
        onDrop={(e) => onDrop(e, null)}
      >
        {body}
      </div>
      <AlertDialog open={pendingDelete !== null} onOpenChange={(o) => !o && setPendingDelete(null)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-sm">
              {pendingDelete?.kind === "folder"
                ? `Delete the folder “${pendingDelete.folder.name}”?`
                : `Delete “${pendingDelete?.query.name ?? ""}”?`}
            </AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              {pendingDelete?.kind === "folder" && pendingDelete.count > 0
                ? `The ${pendingDelete.count === 1 ? "query" : `${pendingDelete.count} queries`} inside it will be deleted too. This cannot be undone.`
                : "This cannot be undone."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <AlertDialogAction size="sm" variant="destructive" onClick={confirmDelete}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/** "Move to Folder ▸": the top level and every folder (but `exclude`). */
function MoveMenu({
  current,
  options,
  exclude,
  onMove,
}: {
  current: string | null;
  options: { folder: QueryFolder; depth: number }[];
  exclude?: ReadonlySet<string>;
  onMove: (folderId: string | null) => void;
}) {
  return (
    <ContextMenuSub>
      <ContextMenuSubTrigger>
        <FolderInput /> Move to Folder
      </ContextMenuSubTrigger>
      <ContextMenuSubContent className="max-h-80 w-48 overflow-auto">
        <ContextMenuItem disabled={current === null} onSelect={() => onMove(null)}>
          Top Level
        </ContextMenuItem>
        {options.length > 0 && <ContextMenuSeparator />}
        {options.map(({ folder, depth }) => (
          <ContextMenuItem
            key={folder.id}
            disabled={folder.id === current || exclude?.has(folder.id)}
            onSelect={() => onMove(folder.id)}
          >
            <span style={{ paddingLeft: depth * 10 }} className="truncate">
              {folder.name}
            </span>
          </ContextMenuItem>
        ))}
      </ContextMenuSubContent>
    </ContextMenuSub>
  );
}
