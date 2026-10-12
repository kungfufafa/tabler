import { useQuery } from "@tanstack/react-query";
import {
  ChevronRight,
  Database,
  Import,
  Link2,
  Plug,
  Plus,
  Search,
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
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { comboFromEvent } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { ConnectionRow } from "./ConnectionRow";
import { useOpenProfile } from "./openProfile";
import { connectionRowId, groupProfiles, isGroupCollapsed, visibleProfiles } from "./list";
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
  onToggle,
}: {
  name: string;
  count: number;
  collapsed: boolean;
  /** While searching every group is shown expanded. */
  disabled: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      tabIndex={-1}
      aria-expanded={!collapsed}
      disabled={disabled}
      // Keep the focus on the list (its keyboard handling) when clicked.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onToggle}
      className="text-muted-foreground hover:text-foreground flex h-7 w-full items-center gap-1 px-3 pt-1 text-left text-[11px] font-semibold disabled:pointer-events-none"
    >
      <ChevronRight
        className={cn("size-3 shrink-0 transition-transform", !collapsed && "rotate-90")}
      />
      <span className="truncate tracking-wide uppercase">{name}</span>
      <span className="font-normal tabular-nums">{count}</span>
    </button>
  );
}

/**
 * The welcome screen and connection manager: the first screen, and reachable later with ⌘⇧O.
 * Drawn as a compact centred window (like TablePlus's welcome window) on the window background.
 * Left: app name, version and the "new connection" actions. Right: the saved connections, the
 * ungrouped ones first without a header, then the groups; click selects, double-click / ↩
 * connects, ↑/↓ move, ⌫ deletes, ⌘F searches, ⌘N creates. Nothing is selected on load: the list
 * only rings its keyboard target (the first row) while it has the focus.
 */
export function Launcher() {
  const profiles = useProfiles();
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
  const appInfo = useQuery({ queryKey: ["app-info"], queryFn: ipc.appInfo, staleTime: Infinity });
  const setUrlOpen = useConnectionDialog((s) => s.setUrlOpen);
  const savedId = useConnectionDialog((s) => s.savedId);

  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // A profile the form just saved becomes the list's selection.
  useEffect(() => {
    if (savedId) setSelectedId(savedId);
  }, [savedId]);
  const [pendingDelete, setPendingDelete] = useState<ConnectionProfile | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const searching = query.trim() !== "";
  const groups = useMemo(() => groupProfiles(profiles.data ?? [], query), [profiles.data, query]);
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

  const version = appInfo.data?.version;

  return (
    // The window background, with the welcome "window" centred on it (16px clear of the edges).
    <div
      ref={rootRef}
      className="bg-surface-2 dark:bg-background flex h-full min-h-0 min-w-0 items-center justify-center p-4"
    >
      <div className="bg-background border-border dark:bg-surface-2 flex h-[560px] max-h-full min-h-0 w-full max-w-[860px] overflow-hidden rounded-xl border shadow-sm dark:shadow-black/40">
        {/* As wide as its longest action (at least 240px), so no action is ever cut off. */}
        <aside className="border-border flex min-w-60 shrink-0 flex-col border-r px-3 pb-4">
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
            <Tooltip>
              <TooltipTrigger asChild>
                {/* A disabled button gets no pointer events: the wrapper carries the tooltip. */}
                <span className="block" tabIndex={0} aria-label="Import connections (coming soon)">
                  <WelcomeAction icon={Import} disabled>
                    Import connections…
                  </WelcomeAction>
                </span>
              </TooltipTrigger>
              <TooltipContent side="right">Coming soon</TooltipContent>
            </Tooltip>
          </nav>
        </aside>

        <section className="group/pane bg-surface flex min-w-0 flex-1 flex-col">
          <div className="border-border flex h-11 shrink-0 items-center border-b px-3">
            <div className="relative w-full">
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
          </div>

          <div className="min-h-0 flex-1 overflow-y-auto py-1.5">
            {profiles.isPending ? (
              <div className="flex flex-col gap-1 px-1.5">
                {[0, 1, 2].map((i) => (
                  <Skeleton key={i} className="h-11 rounded-md" />
                ))}
              </div>
            ) : profiles.isError ? (
              <EmptyState title="Could not load connections" description={profiles.error.message} />
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
                          onSelect={() => setSelectedId(p.id)}
                          onConnect={() => openProfile(p.id)}
                          onEdit={() => openConnectionDialog(p)}
                          onDuplicate={() =>
                            duplicate.mutate(p.id, { onSuccess: (copy) => setSelectedId(copy.id) })
                          }
                          onCopyUrl={() => copyUrl(p)}
                          onDelete={() => setPendingDelete(p)}
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
            )}
          </div>
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
