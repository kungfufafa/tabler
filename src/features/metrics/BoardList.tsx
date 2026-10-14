import { Ellipsis, Gauge, Globe, Plus } from "lucide-react";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import type { BoardDraft } from "./store";

export interface BoardListActions {
  onSelect: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, name: string) => void;
  onDuplicate: (id: string) => void;
  onToggleAll: (id: string) => void;
  onExport: (id: string) => void;
  onImport: () => void;
  onDelete: (id: string) => void;
}

/**
 * The Metrics Board's left sidebar: the connection's boards and those shown on every connection
 * (globe). + is New Board…; a board's context menu renames, duplicates, shows it on every
 * connection, exports or deletes it; ⋯ imports a board file.
 */
export function BoardList({
  boards,
  selectedId,
  renamingId,
  setRenamingId,
  loading,
  error,
  actions,
}: {
  boards: readonly BoardDraft[];
  selectedId: string | null;
  renamingId: string | null;
  setRenamingId: (id: string | null) => void;
  loading: boolean;
  error: string | null;
  actions: BoardListActions;
}) {
  const [draft, setDraft] = useState("");
  // A rename that starts (here or from New Board…) begins with the board's name.
  const [editingId, setEditingId] = useState<string | null>(null);
  if (renamingId !== editingId) {
    setEditingId(renamingId);
    if (renamingId) setDraft(boards.find((b) => b.id === renamingId)?.name ?? "");
  }
  /** Set once a rename ended (Enter, Escape or blur), so the blur that follows does nothing. */
  const ended = useRef(false);
  const endRename = (b: BoardDraft, save: boolean) => {
    if (ended.current) return;
    ended.current = true;
    setRenamingId(null);
    const name = draft.trim();
    if (save && name !== "" && name !== b.name) actions.onRename(b.id, name);
  };
  return (
    <aside
      aria-label="Boards"
      data-slot="metrics-board-list"
      className="border-border bg-sidebar flex w-52 shrink-0 flex-col border-r"
    >
      <div className="border-border flex h-9 shrink-0 items-center gap-0.5 border-b pr-1 pl-3">
        <span className="text-muted-foreground min-w-0 flex-1 truncate text-[11px] font-semibold">
          Boards
        </span>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-xs" aria-label="More board actions">
              <Ellipsis />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-44">
            <DropdownMenuItem onSelect={actions.onNew}>New Board…</DropdownMenuItem>
            <DropdownMenuItem onSelect={actions.onImport}>Import Board…</DropdownMenuItem>
            <DropdownMenuItem
              disabled={!selectedId}
              onSelect={() => selectedId && actions.onExport(selectedId)}
            >
              Export Board…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="New Board…"
          title="New Board…"
          onClick={actions.onNew}
        >
          <Plus />
        </Button>
      </div>
      <ul role="listbox" aria-label="Boards" className="min-h-0 flex-1 overflow-y-auto p-1.5">
        {boards.map((b) => (
          <li key={b.id} role="option" aria-selected={b.id === selectedId}>
            {renamingId === b.id ? (
              <Input
                autoFocus
                aria-label="Board name"
                value={draft}
                className="h-6 px-1.5 text-xs md:text-xs"
                onChange={(e) => setDraft(e.target.value)}
                onFocus={(e) => {
                  ended.current = false;
                  e.currentTarget.select();
                }}
                onBlur={() => endRename(b, true)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") endRename(b, true);
                  if (e.key === "Escape") endRename(b, false);
                }}
              />
            ) : (
              <ContextMenu>
                <ContextMenuTrigger asChild>
                  <button
                    type="button"
                    onClick={() => actions.onSelect(b.id)}
                    onDoubleClick={() => setRenamingId(b.id)}
                    className={cn(
                      "flex h-6 w-full items-center gap-1.5 rounded-md px-1.5 text-left text-xs",
                      b.id === selectedId
                        ? "bg-sidebar-accent text-sidebar-accent-foreground font-medium"
                        : "hover:bg-sidebar-accent/60",
                    )}
                  >
                    <Gauge className="text-muted-foreground size-3.5 shrink-0" />
                    <span className="min-w-0 flex-1 truncate">{b.name}</span>
                    {b.profileId === null && (
                      <Globe
                        className="text-muted-foreground size-3 shrink-0"
                        aria-label="On every connection"
                      />
                    )}
                  </button>
                </ContextMenuTrigger>
                <ContextMenuContent className="w-52">
                  <ContextMenuItem onSelect={() => setRenamingId(b.id)}>Rename</ContextMenuItem>
                  <ContextMenuItem onSelect={() => actions.onDuplicate(b.id)}>
                    Duplicate
                  </ContextMenuItem>
                  <ContextMenuCheckboxItem
                    checked={b.profileId === null}
                    onSelect={() => actions.onToggleAll(b.id)}
                  >
                    Show on All Connections
                  </ContextMenuCheckboxItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem onSelect={() => actions.onExport(b.id)}>
                    Export Board…
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem variant="destructive" onSelect={() => actions.onDelete(b.id)}>
                    Delete
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            )}
          </li>
        ))}
        {boards.length === 0 && (
          <li className="text-muted-foreground px-1.5 py-2 text-[11px] italic">
            {loading ? "Loading…" : (error ?? "No boards yet")}
          </li>
        )}
      </ul>
      <div className="border-border shrink-0 border-t p-1.5">
        <Button
          variant="ghost"
          size="xs"
          className="text-muted-foreground w-full justify-start"
          onClick={actions.onNew}
        >
          <Plus />
          New Board…
        </Button>
      </div>
    </aside>
  );
}
