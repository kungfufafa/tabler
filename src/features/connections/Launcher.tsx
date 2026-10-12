import { Plug, Plus, Search } from "lucide-react";
import { useMemo, useState } from "react";
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
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { usePaletteStore } from "@/features/palette/store";
import type { ConnectionProfile } from "@/lib/ipc";
import { useShallow } from "zustand/react/shallow";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { ConnectionCard } from "./ConnectionCard";
import { ConnectionDialog } from "./ConnectionDialog";
import { connectionSummary } from "./schema";
import { openConnectionDialog, useConnectionDialog } from "./store";
import { useConnect, useDeleteProfile, useDuplicateProfile, useProfiles } from "./useConnections";

function groupProfiles(
  profiles: ConnectionProfile[],
  query: string,
): Array<[string, ConnectionProfile[]]> {
  const q = query.trim().toLowerCase();
  const filtered = q
    ? profiles.filter((p) =>
        `${p.name} ${connectionSummary(p)} ${p.group ?? ""}`.toLowerCase().includes(q),
      )
    : profiles;
  const groups = new Map<string, ConnectionProfile[]>();
  for (const p of filtered) {
    const key = p.group?.trim() || "";
    groups.set(key, [...(groups.get(key) ?? []), p]);
  }
  return [...groups.entries()].sort(([a], [b]) =>
    a === "" ? -1 : b === "" ? 1 : a.localeCompare(b),
  );
}

/** The newest open connection of a profile, if any. */
function openConnectionOf(profileId: string): string | null {
  const s = useConnectionsStore.getState();
  for (let i = s.order.length - 1; i >= 0; i--) {
    const id = s.order[i]!;
    if (s.byId[id]?.info.profileId === profileId) return id;
  }
  return null;
}

/** Connection manager: the first screen, and reachable later with ⌘⇧O. */
export function Launcher() {
  const profiles = useProfiles();
  const connect = useConnect();
  const setView = useUiStore((s) => s.setView);
  // Shared with the palette: one connect per profile at a time, whoever started it.
  const connecting = usePaletteStore((s) => s.connecting);
  const remove = useDeleteProfile();
  const duplicate = useDuplicateProfile();
  const openIds = useConnectionsStore(
    useShallow((s) => Object.values(s.byId).map((c) => c.info.profileId)),
  );
  const [query, setQuery] = useState("");
  const dialogOpen = useConnectionDialog((s) => s.open);
  const dialogProfile = useConnectionDialog((s) => s.profile);
  const setDialogOpen = useConnectionDialog((s) => s.setOpen);
  const [pendingDelete, setPendingDelete] = useState<ConnectionProfile | null>(null);
  const groups = useMemo(() => groupProfiles(profiles.data ?? [], query), [profiles.data, query]);

  /**
   * Opens a profile: switches to its open connection when there is one (no second session, pool
   * and tunnel), otherwise connects, ignoring clicks while that connect is still in flight.
   */
  const openProfile = (p: ConnectionProfile) => {
    const open = openConnectionOf(p.id);
    if (open) {
      useConnectionsStore.getState().setActive(open);
      setView("workspace");
      return;
    }
    const palette = usePaletteStore.getState();
    if (!palette.beginConnect(p.id)) return;
    connect
      .mutateAsync(p.id)
      .finally(() => usePaletteStore.getState().endConnect(p.id))
      // useConnect's onError already told the user.
      .catch(() => undefined);
  };

  return (
    <main className="bg-background flex h-full flex-col">
      <div className="border-border flex items-center gap-2 border-b px-5 py-3">
        <div>
          <h1 className="text-base font-semibold">Connections</h1>
          <p className="text-muted-foreground text-xs">Click a connection to open it.</p>
        </div>
        <div className="flex-1" />
        <div className="relative w-56">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter connections"
            className="h-8 pl-7"
          />
        </div>
        <Button size="sm" onClick={() => openConnectionDialog()}>
          <Plus /> New Connection
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
        {profiles.isPending ? (
          <div className="grid max-w-3xl gap-2">
            {[0, 1, 2].map((i) => (
              <Skeleton key={i} className="h-14 rounded-lg" />
            ))}
          </div>
        ) : profiles.isError ? (
          <EmptyState title="Could not load connections" description={profiles.error.message} />
        ) : groups.length === 0 ? (
          <EmptyState
            icon={Plug}
            title={query ? "No matching connections" : "No connections yet"}
            description={
              query
                ? "Try another search term."
                : "Create a connection to PostgreSQL, MySQL or SQLite to get started."
            }
          >
            {!query && (
              <Button onClick={() => openConnectionDialog()}>
                <Plus /> New Connection
              </Button>
            )}
          </EmptyState>
        ) : (
          <div className="flex max-w-3xl flex-col gap-5">
            {groups.map(([group, items]) => (
              <section key={group || "__ungrouped"} className="flex flex-col gap-2">
                {(group || groups.length > 1) && (
                  <h2 className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
                    {group || "Ungrouped"}
                  </h2>
                )}
                {items.map((p) => (
                  <ConnectionCard
                    key={p.id}
                    profile={p}
                    connecting={connecting.includes(p.id)}
                    connected={openIds.includes(p.id)}
                    onConnect={() => openProfile(p)}
                    onEdit={() => openConnectionDialog(p)}
                    onDuplicate={() => duplicate.mutate(p.id)}
                    onDelete={() => setPendingDelete(p)}
                  />
                ))}
              </section>
            ))}
          </div>
        )}
      </div>

      <ConnectionDialog open={dialogOpen} profile={dialogProfile} onOpenChange={setDialogOpen} />

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
    </main>
  );
}
