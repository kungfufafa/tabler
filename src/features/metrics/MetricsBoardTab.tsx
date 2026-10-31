import {
  AlertCircle,
  ChartColumnBig,
  ChartLine,
  ChartPie,
  Check,
  Hash,
  ListFilter,
  Loader2,
  MousePointerClick,
  Pencil,
  Plus,
  RefreshCw,
  Table2,
  TextCursorInput,
  type LucideIcon,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { sessionKind } from "@/features/drivers/catalog";
import { NotAvailable } from "@/features/drivers/NotAvailable";
import { useAction } from "@/features/palette/actions";
import type { ToolTabProps } from "@/features/tabs/toolTabs";
import { connectionDialect, has } from "@/lib/caps";
import { ipc, toIpcError, type Dialect, type MetricsBoard } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { useConnectionsStore } from "@/stores/connections";
import { DeleteBoardDialog, ExportBoardDialog, ImportPasswordDialog } from "./BoardDialogs";
import { BoardList } from "./BoardList";
import { Canvas } from "./Canvas";
import { Inspector } from "./Inspector";
import {
  duplicateObject,
  KIND_INFO,
  newObject,
  OBJECT_KINDS,
  removeObject,
  type ObjectKind,
} from "./model";
import { openMetricsBoard, pickBoardOpenPath, type MetricsParams } from "./open";
import {
  boardsFor,
  createBoard,
  deleteBoard,
  flushSave,
  loadBoards,
  renameBoard,
  setBoardProfile,
  updateObjects,
  useMetricsStore,
  type BoardDraft,
} from "./store";
import { useBoardRuntime } from "./useBoardRuntime";

const KIND_ICONS: Record<ObjectKind, LucideIcon> = {
  bar: ChartColumnBig,
  line: ChartLine,
  pie: ChartPie,
  scoreboard: Hash,
  table: Table2,
  input: TextCursorInput,
  dropdown: ListFilter,
  button: MousePointerClick,
};

/** `Untitled Board`, `Untitled Board 2`… */
function freeName(base: string, boards: readonly BoardDraft[]): string {
  const taken = new Set(boards.map((b) => b.name.toLowerCase()));
  if (!taken.has(base.toLowerCase())) return base;
  for (let i = 2; ; i++) if (!taken.has(`${base} ${i}`.toLowerCase())) return `${base} ${i}`;
}

/** The board on the right of the tab: its bar (Edit / Done, +, Refresh), canvas and inspector. */
function BoardView({
  connectionId,
  dialect,
  board,
  editing,
  setEditing,
}: {
  connectionId: string;
  dialect: Dialect;
  board: BoardDraft;
  editing: boolean;
  setEditing: (v: boolean) => void;
}) {
  const runtime = useBoardRuntime(connectionId, board.id, dialect);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const objects = board.objects;
  const selected = editing ? (objects.find((o) => o.id === selectedId) ?? null) : null;
  const anyRunning = useMetricsStore((s) =>
    objects.some((o) => s.runs[`${board.id}:${o.id}`]?.status === "running"),
  );

  // ⌘R / View ▸ Refresh: every object of the board.
  useAction("view.refresh", runtime.refreshAll);

  const add = (kind: ObjectKind) => {
    const o = newObject(kind, objects);
    updateObjects(board.id, (list) => [...list, o]);
    setSelectedId(o.id);
  };
  const duplicate = (id: string) => {
    const o = objects.find((x) => x.id === id);
    if (!o) return;
    const copy = duplicateObject(o, objects);
    updateObjects(board.id, (list) => [...list, copy]);
    setSelectedId(copy.id);
  };
  const remove = (id: string) => {
    updateObjects(board.id, (list) => removeObject(list, id));
    if (selectedId === id) setSelectedId(null);
  };

  return (
    <section aria-label={board.name} className="flex min-w-0 flex-1 flex-col">
      <div className="border-border flex h-9 shrink-0 items-center gap-1 border-b px-3">
        <h2 className="min-w-0 truncate text-[13px] font-semibold">{board.name}</h2>
        {board.saveError ? (
          <button
            type="button"
            className="text-error ml-1 flex items-center gap-1 text-[11px]"
            title={board.saveError}
            onClick={() => void flushSave(board.id)}
          >
            <AlertCircle className="size-3" />
            Not saved. Retry
          </button>
        ) : (
          board.dirty && <span className="text-muted-foreground ml-1 text-[11px]">Saving…</span>
        )}
        <span className="flex-1" />
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Refresh all"
          title={`Refresh all (${formatCombo(shortcutFor("workspace.reload") ?? "Mod+R")})`}
          onClick={runtime.refreshAll}
        >
          <RefreshCw className={anyRunning ? "animate-spin" : undefined} />
        </Button>
        {editing && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="xs" aria-label="Add object">
                <Plus />
                Add
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-44">
              {OBJECT_KINDS.map((kind, i) => {
                const Icon = KIND_ICONS[kind];
                return (
                  <div key={kind}>
                    {(i === 3 || i === 5) && <DropdownMenuSeparator />}
                    <DropdownMenuItem onSelect={() => add(kind)}>
                      <Icon />
                      {KIND_INFO[kind].label}
                    </DropdownMenuItem>
                  </div>
                );
              })}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        <Button
          variant={editing ? "default" : "outline"}
          size="xs"
          aria-pressed={editing}
          onClick={() => {
            setEditing(!editing);
            if (editing) {
              setSelectedId(null);
              void flushSave(board.id);
            }
          }}
        >
          {editing ? <Check /> : <Pencil />}
          {editing ? "Done" : "Edit"}
        </Button>
      </div>
      <div className="flex min-h-0 flex-1">
        <Canvas
          boardId={board.id}
          objects={objects}
          editing={editing}
          selectedId={selected?.id ?? null}
          runtime={runtime}
          onSelect={setSelectedId}
          onEdit={(id) => {
            setEditing(true);
            setSelectedId(id);
          }}
          onDuplicate={duplicate}
          onDelete={remove}
        />
        {selected && (
          <Inspector
            key={selected.id}
            boardId={board.id}
            object={selected}
            objects={objects}
            dialect={dialect}
            onRun={() =>
              selected.kind === "button"
                ? void runtime.click(selected.id)
                : runtime.refresh(selected.id)
            }
            onDuplicate={() => duplicate(selected.id)}
            onDelete={() => remove(selected.id)}
            onClose={() => setSelectedId(null)}
          />
        )}
      </div>
    </section>
  );
}

/**
 * The Metrics Board tab (TablePlus's dashboards, spec §8.4) of a connection: the boards on the
 * left (this connection's and those shown on every connection), the chosen board on the right.
 */
export function MetricsBoardTab({ tab, params }: ToolTabProps<MetricsParams>) {
  const connectionId = tab.connectionId;
  const info = useConnectionsStore((s) => s.byId[connectionId]?.info ?? null);
  const profileId = info?.profileId ?? "";
  const boards = useMetricsStore(useShallow((s) => boardsFor(s, profileId)));
  const status = useMetricsStore((s) => s.loaded[profileId]);
  const [editing, setEditing] = useState(false);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [exporting, setExporting] = useState<BoardDraft | null>(null);
  const [deleting, setDeleting] = useState<BoardDraft | null>(null);
  const [importPath, setImportPath] = useState<string | null>(null);

  useEffect(() => {
    if (profileId && status === undefined) void loadBoards(profileId);
  }, [profileId, status]);

  const shown = boards.find((b) => b.id === params.boardId) ?? boards[0] ?? null;

  const select = useCallback(
    (id: string | null) => {
      setEditing(false);
      openMetricsBoard(connectionId, id);
    },
    [connectionId],
  );

  const imported = (list: MetricsBoard[]) => {
    void loadBoards(profileId).then(() => {
      const first = list[0];
      if (first) select(first.id);
    });
    toast.success(
      list.length === 1 ? `Imported “${list[0]?.name}”` : `Imported ${list.length} boards`,
    );
  };

  const actions = {
    onSelect: (id: string) => select(id),
    onNew: () => {
      void createBoard(profileId, freeName("Untitled Board", boards)).then(
        (b) => {
          select(b.id);
          setEditing(true);
          setRenamingId(b.id);
        },
        (err: unknown) =>
          toast.error("Could not create the board", { description: toIpcError(err).message }),
      );
    },
    onRename: (id: string, name: string) => void renameBoard(id, name),
    onDuplicate: (id: string) => {
      const b = boards.find((x) => x.id === id);
      if (!b) return;
      void createBoard(b.profileId, freeName(`${b.name} copy`, boards), b.objects).then(
        (copy) => select(copy.id),
        (err: unknown) =>
          toast.error("Could not duplicate the board", { description: toIpcError(err).message }),
      );
    },
    onToggleAll: (id: string) => {
      const b = boards.find((x) => x.id === id);
      if (b) void setBoardProfile(id, b.profileId === null ? profileId : null);
    },
    onExport: (id: string) => {
      const b = boards.find((x) => x.id === id);
      if (b) void flushSave(id).then(() => setExporting(b));
    },
    onImport: async () => {
      const path = (await pickBoardOpenPath())?.trim();
      if (!path) return;
      try {
        const outcome = await ipc.metrics.importBoards(path, null, profileId);
        if (outcome.needsPassword) setImportPath(path);
        else imported(outcome.boards);
      } catch (err) {
        toast.error("Could not import the board", { description: toIpcError(err).message });
      }
    },
    onDelete: (id: string) => setDeleting(boards.find((x) => x.id === id) ?? null),
  };

  if (!info)
    return (
      <div className="text-muted-foreground flex h-full items-center justify-center text-xs">
        Not connected.
      </div>
    );
  // Its queries are split and classified by the session's core dialect, on a session that offers
  // boards (a restored tab skips the opener's check): none for a Redshift session, although its
  // driver is PostgreSQL's.
  const dialect = has(info, "metrics") ? connectionDialect(info) : null;
  if (!dialect) return <NotAvailable kind={sessionKind(info)} what="Metrics boards" />;

  return (
    <div data-slot="metrics-board" className="bg-background flex h-full min-h-0">
      <BoardList
        boards={boards}
        selectedId={shown?.id ?? null}
        renamingId={renamingId}
        setRenamingId={setRenamingId}
        loading={status === "loading" || status === undefined}
        error={status && status !== "ok" && status !== "loading" ? status : null}
        actions={actions}
      />
      {shown ? (
        <BoardView
          key={`${connectionId}:${shown.id}`}
          connectionId={connectionId}
          dialect={dialect}
          board={shown}
          editing={editing}
          setEditing={setEditing}
        />
      ) : (
        <div className="text-muted-foreground flex min-w-0 flex-1 flex-col items-center justify-center gap-3 p-6 text-center text-xs">
          {status === "loading" || status === undefined ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <>
              <p>
                A board shows charts, scoreboards and tables from your queries, with inputs to
                filter them.
              </p>
              <Button size="sm" onClick={actions.onNew}>
                <Plus />
                New Board…
              </Button>
            </>
          )}
        </div>
      )}
      <ExportBoardDialog board={exporting} onClose={() => setExporting(null)} />
      <DeleteBoardDialog
        board={deleting}
        onClose={() => setDeleting(null)}
        onConfirm={(id) =>
          void deleteBoard(id).catch((err: unknown) =>
            toast.error("Could not delete the board", { description: toIpcError(err).message }),
          )
        }
      />
      <ImportPasswordDialog
        path={importPath}
        profileId={profileId}
        onImported={imported}
        onClose={() => setImportPath(null)}
      />
    </div>
  );
}
