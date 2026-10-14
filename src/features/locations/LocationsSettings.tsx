import { FolderSearch, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { flushFileAutoSaves } from "@/features/editor/fileAutoSave";
import { pickFolder } from "@/features/queries/files";
import { refreshSavedQueries } from "@/features/queries/store";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import {
  ipc,
  toIpcError,
  type LocationChange,
  type LocationInspection,
  type LocationKind,
} from "@/lib/ipc";
import { flushPrefs } from "@/stores/prefs";
import { hasUnsaved } from "@/stores/tabs";
import { locationPrompt } from "./model";
import { loadLocations, setLocations, useLocationsStore } from "./store";

interface Pending {
  kind: LocationKind;
  /** null: back to the default folder. */
  path: string | null;
  seen: LocationInspection;
}

function PathText({ path }: { path: string }) {
  return (
    <span className="selectable block max-w-80 truncate font-mono text-[11px]" title={path}>
      {path}
    </span>
  );
}

/** Restarts so a new data folder is used (refused while something is unsaved). */
async function restartNow(): Promise<void> {
  if (hasUnsaved()) {
    toast.message("Save or close the unsaved work first", {
      description: "Then restart to use the new folder.",
    });
    return;
  }
  flushFileAutoSaves();
  await flushPrefs();
  try {
    await ipc.locations.restart();
  } catch (e) {
    toast.error("Could not restart", { description: toIpcError(e).message });
  }
}

/**
 * Settings ▸ Locations (TablePlus's Preferences ▸ Locations): the folder of the connections &
 * settings (the app database, with the query history) and the folder of the saved queries. Either
 * can be moved, e.g. into Dropbox or iCloud Drive: changing one asks whether to move the existing
 * data or start empty. Passwords stay in the Keychain.
 */
export function LocationsSettings() {
  const info = useLocationsStore((s) => s.info);
  const [pending, setPending] = useState<Pending | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void loadLocations().catch((e: unknown) =>
      toast.error("Could not read the locations", { description: toIpcError(e).message }),
    );
  }, []);

  const choose = async (kind: LocationKind, reset = false) => {
    const current = info;
    if (!current) return;
    const path = reset
      ? null
      : await pickFolder(
          kind === "data" ? "Connections & Settings Folder" : "Queries Folder",
          kind === "data" ? current.dataDir : current.queriesDir,
        );
    if (!reset && !path) return;
    const target = path ?? (kind === "data" ? current.defaultDataDir : current.defaultQueriesDir);
    let seen: LocationInspection;
    try {
      seen = await ipc.locations.inspect(kind, target);
    } catch (e) {
      toast.error("Could not check the folder", { description: toIpcError(e).message });
      return;
    }
    if (seen.problem && !(reset && kind === "queries")) {
      toast.error("That folder cannot be used", { description: seen.problem });
      return;
    }
    if (seen.isCurrent) {
      // Picking the folder in use again cancels a pending data folder change.
      if (kind === "data" && current.pendingDataDir) await apply({ kind, path, seen }, "move");
      else toast.message("That folder is already in use");
      return;
    }
    setPending({ kind, path, seen });
  };

  const apply = async (p: Pending, change: LocationChange) => {
    setBusy(true);
    try {
      if (p.kind === "data") {
        const next = await ipc.locations.setData(p.path, change);
        setLocations(next);
        if (next.pendingDataDir)
          toast.success("Restart Tabler to use the new folder", {
            action: { label: "Restart Now", onClick: () => void restartNow() },
          });
        else toast.message("No change is waiting for a restart");
      } else {
        const next = await ipc.locations.setQueries(p.path, change);
        setLocations(next);
        await refreshSavedQueries();
        toast.success("Saved queries now live in the new folder");
      }
      setPending(null);
    } catch (e) {
      toast.error("Could not change the folder", { description: toIpcError(e).message });
    } finally {
      setBusy(false);
    }
  };

  const reveal = (kind: LocationKind) =>
    void ipc.locations
      .reveal(kind)
      .catch((e: unknown) =>
        toast.error("Could not show it in Finder", { description: toIpcError(e).message }),
      );

  const data = info;
  if (!data) return null;
  const prompt = pending ? locationPrompt(pending.kind, pending.seen) : null;
  const dataCustom = (data.configuredDataDir ?? null) !== null;
  const queriesCustom = data.configuredQueriesDir !== null;

  return (
    <>
      <SettingGroup title="Locations">
        <SettingRow
          id="locations-data"
          label="Connections & settings"
          description={
            <>
              <PathText path={data.dataDir} />
              Connection profiles, preferences and the query history (tabler.sqlite). Passwords stay
              in the macOS Keychain, so another Mac asks for them again.
              {data.dataFallback && data.configuredDataDir && (
                <span className="text-warning mt-1 flex items-start gap-1">
                  <TriangleAlert className="mt-0.5 size-3 shrink-0" />
                  {`${data.configuredDataDir} was not available at startup: this session uses the default folder.`}
                </span>
              )}
              {data.pendingDataDir && (
                <span className="text-brand mt-1 block">
                  {`After a restart: ${data.pendingDataDir}`}
                </span>
              )}
            </>
          }
        >
          {data.pendingDataDir && (
            <Button size="xs" onClick={() => void restartNow()}>
              Restart Now
            </Button>
          )}
          <Button size="xs" variant="outline" onClick={() => void choose("data")}>
            Choose…
          </Button>
          <Button
            size="xs"
            variant="ghost"
            disabled={!dataCustom && !data.pendingDataDir}
            onClick={() => void choose("data", true)}
          >
            Default
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Show the connections & settings file in Finder"
            title="Show in Finder"
            onClick={() => reveal("data")}
          >
            <FolderSearch />
          </Button>
        </SettingRow>
        <SettingRow
          id="locations-queries"
          label="Queries"
          description={
            <>
              <PathText path={data.queriesDir} />
              Saved queries, one .sql file each; folders are sub-folders. Changes apply at once.
              {data.queriesFallback && data.configuredQueriesDir && (
                <span className="text-warning mt-1 flex items-start gap-1">
                  <TriangleAlert className="mt-0.5 size-3 shrink-0" />
                  {`${data.configuredQueriesDir} was not available at startup: this session uses the default folder.`}
                </span>
              )}
            </>
          }
        >
          <Button size="xs" variant="outline" onClick={() => void choose("queries")}>
            Choose…
          </Button>
          <Button
            size="xs"
            variant="ghost"
            disabled={!queriesCustom && !data.queriesFallback}
            onClick={() => void choose("queries", true)}
          >
            Default
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Show the queries folder in Finder"
            title="Show in Finder"
            onClick={() => reveal("queries")}
          >
            <FolderSearch />
          </Button>
        </SettingRow>
      </SettingGroup>
      <AlertDialog open={pending !== null} onOpenChange={(o) => !o && !busy && setPending(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="text-sm">{prompt?.title}</AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              {prompt?.description}
              {pending && (
                <span className="mt-1 block font-mono text-[11px] break-all">
                  {pending.seen.path}
                </span>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm" disabled={busy}>
              Cancel
            </AlertDialogCancel>
            {pending &&
              prompt?.choices.map((c) => (
                <Button
                  key={c.change}
                  size="sm"
                  disabled={busy}
                  variant={c.destructive ? "destructive" : c.primary ? "default" : "outline"}
                  onClick={() => void apply(pending, c.change)}
                >
                  {c.label}
                </Button>
              ))}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
