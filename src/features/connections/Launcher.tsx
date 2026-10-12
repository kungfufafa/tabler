import {
  DndContext,
  DragOverlay,
  PointerSensor,
  pointerWithin,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import { useQuery } from "@tanstack/react-query";
import {
  ArchiveRestore,
  ArrowUpDown,
  ChevronRight,
  Copy,
  Database,
  DatabaseBackup,
  FolderInput,
  FolderPlus,
  Import,
  Link2,
  MoreHorizontal,
  Pencil,
  Plug,
  Plus,
  Search,
  Share,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
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
import { Button } from "@/components/ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { openBackup } from "@/features/backup/store";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { comboFromEvent } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { DriverBadge } from "./badges";
import { ConnectionRow } from "./ConnectionRow";
import { idsInGroup, knownGroups, useGroupActions, useStoredGroups } from "./groups";
import { openExportConnections, openGroupDialog, startImportConnections } from "./ioStore";
import {
  CONNECTION_SORT_LABEL,
  CONNECTION_SORTS,
  connectionRowId,
  groupDropId,
  groupOfDropTarget,
  groupProfiles,
  isGroupCollapsed,
  visibleProfiles,
  type ConnectionSort,
} from "./list";
import { ContextMenuEntries, DropdownMenuEntries, type MenuEntry } from "./menuModel";
import { useOpenProfile } from "./openProfile";
import { openConnectionDialog, useConnectionDialog, useLauncherPrefs } from "./store";
import { profileToUrl } from "./url";
import { useDeleteProfile, useDuplicateProfile, useProfiles } from "./useConnections";

function WelcomeAction({
  icon: Icon,
  combo,
  disabled,
  onClick,
  children,
}: {
  icon: LucideIcon;
  combo?: string;
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] whitespace-nowrap transition-colors outline-none",
        "hover:bg-accent focus-visible:ring-ring/50 focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50",
      )}
    >
      <Icon className="text-muted-foreground size-4 shrink-0" />
      {/* No truncation: the welcome column is as wide as its longest action. */}
      <span className="grow">{children}</span>
      {combo && <Kbd combo={combo} className="ml-2 shrink-0" />}
    </button>
  );
}

function GroupHeader({
  name,
  count,
  collapsed,
  disabled,
  menu,
  onToggle,
}: {
  name: string;
  count: number;
  collapsed: boolean;
  /** While searching every group is shown expanded. */
  disabled: boolean;
  menu: readonly MenuEntry[];
  onToggle: () => void;
}) {
  // Connections dragged onto the header move into the group.
  const drop = useDroppable({ id: groupDropId(name) });
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <button
          ref={drop.setNodeRef}
          type="button"
          tabIndex={-1}
          aria-expanded={!collapsed}
          data-drop-over={drop.isOver || undefined}
          disabled={disabled}
          // Keep the focus on the list (its keyboard handling) when clicked.
          onMouseDown={(e) => e.preventDefault()}
          onClick={onToggle}
          className={cn(
            "text-muted-foreground hover:text-foreground mx-1.5 flex h-7 w-[calc(100%-0.75rem)] items-center gap-1 rounded-md px-1.5 pt-1 text-left text-[11px] font-semibold disabled:pointer-events-none",
            drop.isOver && "bg-brand-soft text-brand ring-brand/50 ring-1",
          )}
        >
          <ChevronRight
            className={cn("size-3 shrink-0 transition-transform", !collapsed && "rotate-90")}
          />
          <span className="truncate tracking-wide uppercase">{name}</span>
          <span className="font-normal tabular-nums">{count}</span>
        </button>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-48">
        <ContextMenuEntries entries={menu} />
      </ContextMenuContent>
    </ContextMenu>
  );
}

/** While a grouped connection is dragged: drop it here to take it out of its group. */
function NoGroupDropZone({ group }: { group: string }) {
  const drop = useDroppable({ id: groupDropId("") });
  return (
    <div
      ref={drop.setNodeRef}
      data-drop-over={drop.isOver || undefined}
      className={cn(
        "border-border text-muted-foreground mx-1.5 mb-1 flex h-8 items-center justify-center rounded-md border border-dashed text-[11px]",
        drop.isOver && "border-brand bg-brand-soft text-brand",
      )}
    >
      Drop here to take it out of “{group}”
    </div>
  );
}

const separator: MenuEntry = { kind: "separator" };

const fileNameOf = (s: string) => s.replace(/[/\\:*?"<>|]+/g, "-").trim() || "Connections";

/**
 * The welcome screen and connection manager: the first screen, and reachable later with ⌘⇧O.
 * Drawn as a compact centred window (like TablePlus's welcome window) on the window background.
 * Left: app name, version and the "new connection" actions, Backup… / Restore… at the bottom.
 * Right: the saved connections, the ungrouped ones first without a header, then the groups (empty
 * ones too); click selects, double-click / ↩ connects, ↑/↓ move, ⌫ deletes, ⌘F searches, ⌘N
 * creates. Rows can be dragged onto a group header. Right-click (or "…") offers New ▸, Sort by ▸,
 * Import / Export Connections, Move to Group ▸ and the group actions. Nothing is selected on load:
 * the list only rings its keyboard target (the first row) while it has the focus.
 */
export function Launcher() {
  const profiles = useProfiles();
  const storedGroups = useStoredGroups();
  const groupActions = useGroupActions();
  const openProfile = useOpenProfile();
  // Shared with the palette: one connect per profile at a time, whoever started it.
  const connecting = usePaletteStore((s) => s.connecting);
  const remove = useDeleteProfile();
  const duplicate = useDuplicateProfile();
  const openIds = useConnectionsStore(
    useShallow((s) => Object.values(s.byId).map((c) => c.info.profileId)),
  );
  const collapsed = useLauncherPrefs((s) => s.collapsedGroups);
  const toggleGroup = useLauncherPrefs((s) => s.toggleGroup);
  const sortBy = useLauncherPrefs((s) => s.sortBy);
  const setSortBy = useLauncherPrefs((s) => s.setSortBy);
  const appInfo = useQuery({ queryKey: ["app-info"], queryFn: ipc.appInfo, staleTime: Infinity });
  const setUrlOpen = useConnectionDialog((s) => s.setUrlOpen);
  const savedId = useConnectionDialog((s) => s.savedId);

  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // A profile the form just saved (or an import added) becomes the list's selection.
  useEffect(() => {
    if (savedId) setSelectedId(savedId);
  }, [savedId]);
  const [pendingDelete, setPendingDelete] = useState<ConnectionProfile | null>(null);
  const [dragging, setDragging] = useState<ConnectionProfile | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));

  const all = useMemo(() => profiles.data ?? [], [profiles.data]);
  const allGroups = useMemo(
    () => knownGroups(storedGroups.data ?? [], all),
    [storedGroups.data, all],
  );
  const searching = query.trim() !== "";
  const groups = useMemo(
    () => groupProfiles(all, query, { groups: allGroups, sort: sortBy }),
    [all, query, allGroups, sortBy],
  );
  /** Rows on screen, in order (collapsed groups left out; searching shows every match). */
  const visible = useMemo(
    () => visibleProfiles(groups, collapsed, searching),
    [groups, collapsed, searching],
  );
  /**
   * The row drawn as selected: the user's pick (click, ↑/↓…) while it is on screen; while
   * searching, the first match stands in so ↩ in the search field connects what is shown.
   */
  const selected =
    visible.find((p) => p.id === selectedId) ?? (searching ? (visible[0] ?? null) : null);
  /** Where ↑/↓/↩ start from: the selection, else the first row on screen (ringed, not filled). */
  const current = selected ?? visible[0] ?? null;
  const hasList = groups.length > 0;

  // ⌘N new connection, ⌘F search — while the launcher (or nothing) has the focus.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const combo = comboFromEvent(e);
      if (combo !== "Mod+N" && combo !== "Mod+F") return;
      const t = e.target instanceof Node ? e.target : null;
      const inLauncher =
        !t ||
        t === document.body ||
        t === document.documentElement ||
        (rootRef.current?.contains(t) ?? false);
      if (!inLauncher) return;
      e.preventDefault();
      if (combo === "Mod+N") {
        openConnectionDialog();
      } else {
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Give the list the focus once it is there, so ↑/↓ and ↩ work straight away.
  const focusedOnce = useRef(false);
  useEffect(() => {
    if (!hasList || focusedOnce.current) return;
    focusedOnce.current = true;
    const active = document.activeElement;
    if (!active || active === document.body) listRef.current?.focus({ preventScroll: true });
  }, [hasList]);

  const select = (p: ConnectionProfile) => {
    setSelectedId(p.id);
    document.getElementById(connectionRowId(p.id))?.scrollIntoView?.({ block: "nearest" });
  };

  /**
   * Arrow / ↩ handling shared by the list and the search field. Returns whether it was handled.
   * With nothing selected the first ↑/↓ selects the current row (like a macOS table); ↩ connects
   * the selection, or from the list (which rings it) the current row.
   */
  const navigate = (e: React.KeyboardEvent, fromList: boolean): boolean => {
    if (e.metaKey || e.ctrlKey || e.altKey) return false;
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp": {
        const i = current ? visible.indexOf(current) : 0;
        const step = selected ? (e.key === "ArrowDown" ? 1 : -1) : 0;
        const p = visible[Math.max(0, Math.min(visible.length - 1, i + step))];
        if (p) select(p);
        break;
      }
      case "Enter": {
        const target = fromList ? current : selected;
        if (target) openProfile(target.id);
        break;
      }
      default:
        return false;
    }
    e.preventDefault();
    return true;
  };

  const onListKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    // Only keys aimed at the list (or a row): React bubbles keydown from the "…" button and from
    // the portaled menu items up to here.
    const t = e.target as HTMLElement;
    if (t !== e.currentTarget && t.getAttribute("role") !== "option") return;
    if (navigate(e, true)) return;
    if (e.altKey || e.ctrlKey) return;
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      const p = e.key === "Home" ? visible[0] : visible.at(-1);
      if (p) select(p);
    } else if ((e.key === "Backspace" || e.key === "Delete") && selected) {
      e.preventDefault();
      setPendingDelete(selected);
    }
  };

  const onSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (navigate(e, false)) return;
    if (e.key === "Escape" && query !== "") {
      e.preventDefault();
      setQuery("");
    }
  };

  const copyUrl = (p: ConnectionProfile) => {
    const url = profileToUrl(p);
    writeClipboard(url).then(
      () => toast.success("Copied", { description: url }),
      () => toast.error("Could not copy the URL"),
    );
  };

  // ---- menus -------------------------------------------------------------------------------

  const exportAll = () =>
    openExportConnections({
      title: "Export All Connections",
      profileIds: all.map((p) => p.id),
      groups: allGroups,
      fileName: "Connections",
    });
  const exportGroup = (group: string) =>
    openExportConnections({
      title: `Export “${group}”`,
      profileIds: idsInGroup(all, group),
      groups: [group],
      fileName: fileNameOf(group),
    });
  const exportOne = (p: ConnectionProfile) =>
    openExportConnections({
      title: `Export “${p.name}”`,
      profileIds: [p.id],
      groups: p.group ? [p.group] : [],
      fileName: fileNameOf(p.name),
    });
  const groupOf = (p: ConnectionProfile | null) => p?.group?.trim() || null;

  const exportMenu = (p: ConnectionProfile | null, group: string | null): MenuEntry => ({
    kind: "sub",
    label: "Export Connections",
    icon: Share,
    items: [
      { kind: "item", label: "Export all…", run: exportAll, disabled: all.length === 0 },
      {
        kind: "item",
        label: "Export this group…",
        run: () => group && exportGroup(group),
        disabled: !group || idsInGroup(all, group).length === 0,
      },
      {
        kind: "item",
        label: "Export this connection…",
        run: () => p && exportOne(p),
        disabled: !p,
      },
    ],
  });
  const importItem: MenuEntry = {
    kind: "item",
    label: "Import Connections…",
    icon: Import,
    run: () => void startImportConnections(),
  };
  const backup = (mode: "backup" | "restore") =>
    openBackup({ mode, profileId: selected?.id ?? null, database: null });
  const backupItems: MenuEntry[] = [
    { kind: "item", label: "Backup…", icon: DatabaseBackup, run: () => backup("backup") },
    { kind: "item", label: "Restore…", icon: ArchiveRestore, run: () => backup("restore") },
  ];
  const newMenu: MenuEntry = {
    kind: "sub",
    label: "New",
    icon: Plus,
    items: [
      { kind: "item", label: "Connection…", run: () => openConnectionDialog() },
      {
        kind: "item",
        label: "Group…",
        run: () => openGroupDialog({ kind: "new", moveIds: [] }),
      },
    ],
  };
  const sortMenu: MenuEntry = {
    kind: "sub",
    label: "Sort by",
    icon: ArrowUpDown,
    items: [
      {
        kind: "radio",
        value: sortBy,
        options: CONNECTION_SORTS.map((s) => ({ value: s, label: CONNECTION_SORT_LABEL[s] })),
        onChange: (v) => setSortBy(v as ConnectionSort),
      },
    ],
  };
  /** Right-click on empty space, and the "…" button of the list. */
  const listMenu: MenuEntry[] = [
    newMenu,
    sortMenu,
    separator,
    importItem,
    exportMenu(selected, groupOf(selected)),
    separator,
    ...backupItems,
  ];

  const moveMenu = (p: ConnectionProfile): MenuEntry => {
    const own = groupOf(p);
    return {
      kind: "sub",
      label: "Move to Group",
      icon: FolderInput,
      items: [
        ...allGroups.map((g): MenuEntry => ({
          kind: "item",
          label: g,
          run: () => void groupActions.moveTo([p.id], g),
          disabled: g === own,
        })),
        separator,
        {
          kind: "item",
          label: "New Group…",
          icon: FolderPlus,
          run: () => openGroupDialog({ kind: "new", moveIds: [p.id] }),
        },
        {
          kind: "item",
          label: "No Group",
          run: () => void groupActions.moveTo([p.id], null),
          disabled: own === null,
        },
      ],
    };
  };

  const rowMenu = (p: ConnectionProfile): MenuEntry[] => [
    { kind: "item", label: "Connect", icon: Plug, run: () => openProfile(p.id) },
    { kind: "item", label: "Edit…", icon: Pencil, run: () => openConnectionDialog(p) },
    {
      kind: "item",
      label: "Duplicate",
      icon: Copy,
      run: () => duplicate.mutate(p.id, { onSuccess: (copy) => setSelectedId(copy.id) }),
    },
    { kind: "item", label: "Copy as URL", icon: Link2, run: () => copyUrl(p) },
    separator,
    moveMenu(p),
    separator,
    exportMenu(p, groupOf(p)),
    importItem,
    separator,
    {
      kind: "item",
      label: "Delete…",
      icon: Trash2,
      run: () => setPendingDelete(p),
      destructive: true,
    },
  ];

  const groupMenu = (group: string): MenuEntry[] => [
    {
      kind: "item",
      label: "Rename Group…",
      icon: Pencil,
      run: () => openGroupDialog({ kind: "rename", group }),
    },
    {
      kind: "item",
      label: "Delete Group",
      icon: Trash2,
      run: () =>
        void groupActions.remove(group).then((ok) => {
          if (ok) useLauncherPrefs.getState().renameCollapsed(group, null);
        }),
    },
    separator,
    {
      kind: "item",
      label: "Export this group…",
      icon: Share,
      run: () => exportGroup(group),
      disabled: idsInGroup(all, group).length === 0,
    },
    importItem,
    separator,
    newMenu,
  ];

  // ---- drag and drop -----------------------------------------------------------------------

  const onDragStart = (e: DragStartEvent) => {
    const p = all.find((x) => x.id === e.active.id);
    setDragging(p ?? null);
  };
  const onDragEnd = (e: DragEndEvent) => {
    setDragging(null);
    const target = groupOfDropTarget(e.over?.id);
    const p = all.find((x) => x.id === e.active.id);
    if (target === undefined || !p || groupOf(p) === target) return;
    void groupActions.moveTo([p.id], target);
  };

  const version = appInfo.data?.version;

  return (
    // The window background, with the welcome "window" centred on it (16px clear of the edges).
    <div
      ref={rootRef}
      className="bg-surface-2 dark:bg-background flex h-full min-h-0 min-w-0 items-center justify-center p-4"
    >
      <div className="bg-background border-border dark:bg-surface-2 flex h-[560px] max-h-full min-h-0 w-full max-w-[860px] overflow-hidden rounded-xl border shadow-sm dark:shadow-black/40">
        {/* As wide as its longest action (at least 240px), so no action is ever cut off. */}
        <aside className="border-border flex min-w-60 shrink-0 flex-col border-r px-3 pb-3">
          <div className="flex min-h-0 flex-1 flex-col items-center justify-center px-3 py-6 text-center">
            <div className="bg-brand text-primary-foreground mb-4 flex size-16 items-center justify-center rounded-[18px] shadow-sm">
              <Database className="size-8" strokeWidth={1.75} />
            </div>
            <h1 className="text-xl font-semibold tracking-tight">Tabler</h1>
            <p className="text-muted-foreground mt-1 h-4 text-xs">
              {version ? `Version ${version}` : null}
            </p>
          </div>
          <nav aria-label="Get started" className="flex flex-col gap-0.5">
            <WelcomeAction
              icon={Plus}
              combo={shortcutFor("connection.new")}
              onClick={() => openConnectionDialog()}
            >
              Create a new connection…
            </WelcomeAction>
            <WelcomeAction icon={Link2} onClick={() => setUrlOpen(true)}>
              New connection from URL…
            </WelcomeAction>
            <WelcomeAction icon={Import} onClick={() => void startImportConnections()}>
              Import connections…
            </WelcomeAction>
          </nav>
          {/* TablePlus keeps Backup and Restore at the bottom of the welcome window. */}
          <nav
            aria-label="Backup and restore"
            className="border-border mt-2 grid grid-cols-2 gap-1.5 border-t pt-3"
          >
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => backup("backup")}
              title="Back up a database with its native dump tool"
            >
              <DatabaseBackup /> Backup…
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => backup("restore")}
              title="Restore a database from a dump file"
            >
              <ArchiveRestore /> Restore…
            </Button>
          </nav>
        </aside>

        <section className="group/pane bg-surface flex min-w-0 flex-1 flex-col">
          <div className="border-border flex h-11 shrink-0 items-center gap-1.5 border-b px-3">
            <div className="relative min-w-0 flex-1">
              <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
              <Input
                ref={searchRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={onSearchKeyDown}
                aria-label="Search connections"
                placeholder="Search for connection…"
                className="h-7 pr-10 pl-7"
                spellCheck={false}
                autoComplete="off"
              />
              <Kbd
                combo="Mod+F"
                className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2"
              />
            </div>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label="More connection actions"
                  title="New, sort, import / export, backup"
                >
                  <MoreHorizontal />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-52">
                <DropdownMenuEntries entries={listMenu} />
              </DropdownMenuContent>
            </DropdownMenu>
          </div>

          <ContextMenu>
            <ContextMenuTrigger asChild>
              <div
                data-testid="connection-list-area"
                className="min-h-0 flex-1 overflow-y-auto py-1.5"
              >
                {profiles.isPending ? (
                  <div className="flex flex-col gap-1 px-1.5">
                    {[0, 1, 2].map((i) => (
                      <Skeleton key={i} className="h-11 rounded-md" />
                    ))}
                  </div>
                ) : profiles.isError ? (
                  <EmptyState
                    title="Could not load connections"
                    description={profiles.error.message}
                  />
                ) : !hasList ? (
                  <EmptyState
                    icon={Plug}
                    title={searching ? "No matching connections" : "No connections yet"}
                    description={
                      searching
                        ? "Try another search term."
                        : "Create a connection to PostgreSQL, MySQL or SQLite to get started."
                    }
                  >
                    {!searching && (
                      <Button size="sm" onClick={() => openConnectionDialog()}>
                        <Plus /> Create a new connection…
                      </Button>
                    )}
                  </EmptyState>
                ) : (
                  <DndContext
                    sensors={sensors}
                    collisionDetection={pointerWithin}
                    onDragStart={onDragStart}
                    onDragEnd={onDragEnd}
                    onDragCancel={() => setDragging(null)}
                  >
                    {dragging && groupOf(dragging) && (
                      <NoGroupDropZone group={groupOf(dragging) ?? ""} />
                    )}
                    <div
                      ref={listRef}
                      role="listbox"
                      aria-label="Connections"
                      tabIndex={0}
                      aria-activedescendant={current ? connectionRowId(current.id) : undefined}
                      onKeyDown={onListKeyDown}
                      className="group/list flex flex-col outline-none"
                    >
                      {groups.map(([group, items]) => {
                        const isCollapsed = isGroupCollapsed(group, collapsed, searching);
                        const rows = isCollapsed
                          ? null
                          : items.map((p) => (
                              <ConnectionRow
                                key={p.id}
                                profile={p}
                                selected={selected?.id === p.id}
                                current={current?.id === p.id}
                                connecting={connecting.includes(p.id)}
                                connected={openIds.includes(p.id)}
                                menu={rowMenu(p)}
                                onSelect={() => setSelectedId(p.id)}
                                onConnect={() => openProfile(p.id)}
                              />
                            ));
                        // Connections without a group come first, without a header.
                        if (group === "") return <Fragment key="">{rows}</Fragment>;
                        return (
                          <div key={group} role="group" aria-label={group}>
                            <GroupHeader
                              name={group}
                              count={items.length}
                              collapsed={isCollapsed}
                              disabled={searching}
                              menu={groupMenu(group)}
                              onToggle={() => {
                                toggleGroup(group);
                                listRef.current?.focus({ preventScroll: true });
                              }}
                            />
                            {rows}
                          </div>
                        );
                      })}
                    </div>
                    <DragOverlay dropAnimation={null}>
                      {dragging ? (
                        <div className="bg-popover border-border flex h-9 w-56 items-center gap-2 rounded-md border px-2 shadow-md">
                          <DriverBadge driver={dragging.driver} color={dragging.color} />
                          <span className="truncate text-[13px] font-semibold">
                            {dragging.name}
                          </span>
                        </div>
                      ) : null}
                    </DragOverlay>
                  </DndContext>
                )}
              </div>
            </ContextMenuTrigger>
            <ContextMenuContent className="w-52">
              <ContextMenuEntries entries={listMenu} />
            </ContextMenuContent>
          </ContextMenu>
        </section>
      </div>

      <AlertDialog
        open={pendingDelete !== null}
        onOpenChange={(open) => !open && setPendingDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete “{pendingDelete?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              The saved connection and its stored password are removed. The database itself is not
              touched.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (pendingDelete) remove.mutate(pendingDelete.id);
                setPendingDelete(null);
              }}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
