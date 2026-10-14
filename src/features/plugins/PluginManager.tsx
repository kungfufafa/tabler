import { FolderOpen, Loader2, Puzzle, RotateCw, Trash2, Upload } from "lucide-react";
import { useMemo, useState } from "react";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { usePrefs } from "@/stores/prefs";
import { BUILTIN_FEATURES, BUILTIN_PLUGINS } from "./builtin";
import { isPluginEnabled } from "./pluginPrefs";
import { pluginEntries } from "./registry";
import {
  closePluginManager,
  installPluginFromFolder,
  refreshPlugins,
  removePlugin,
  revealPluginsFolder,
  setPluginEnabled,
  usePluginsStore,
} from "./store";
import type { PluginEntry } from "./types";

const CONTEXT_LABELS: Record<string, string> = {
  grid: "Grid",
  sidebar: "Sidebar",
  editor: "SQL editor",
  global: "Menu",
};

/** The key a command shows: the keymap's binding for a built-in action (SQL Format), else its own. */
function commandKey(plugin: PluginEntry, index: number): string | null {
  const builtin = plugin.builtin
    ? BUILTIN_PLUGINS.find((p) => p.identifier === plugin.identifier)?.commands[index]
    : undefined;
  if (builtin?.action) return shortcutFor(builtin.action) ?? null;
  return plugin.commands[index]?.key ?? null;
}

function PluginRow({
  plugin,
  enabled,
  selected,
  onSelect,
}: {
  plugin: PluginEntry;
  enabled: boolean;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <div
      role="option"
      aria-selected={selected}
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") onSelect();
      }}
      className={cn(
        "flex cursor-default items-start gap-2.5 rounded-md px-2.5 py-2 outline-none",
        "focus-visible:ring-ring/50 focus-visible:ring-2",
        selected ? "bg-accent text-accent-foreground" : "hover:bg-muted/60",
      )}
    >
      <Puzzle
        aria-hidden
        className={cn("mt-0.5 size-4 shrink-0", enabled ? "text-brand" : "text-muted-foreground")}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 text-xs">
          <span className="truncate font-medium">{plugin.name}</span>
          <span className="text-muted-foreground shrink-0 tabular-nums">{plugin.version}</span>
          {plugin.author && (
            <span className="text-muted-foreground truncate">· {plugin.author}</span>
          )}
          {plugin.builtin && (
            <Badge variant="secondary" className="h-4 shrink-0 px-1.5 text-[10px]">
              Built-in
            </Badge>
          )}
        </div>
        {plugin.description && (
          <p className="text-muted-foreground mt-0.5 line-clamp-2 text-[11px]">
            {plugin.description}
          </p>
        )}
        <ul className="mt-1 flex flex-wrap gap-1">
          {plugin.commands.map((c, i) => {
            const key = commandKey(plugin, i);
            return (
              <li
                key={`${c.name}-${i}`}
                className="bg-muted text-muted-foreground inline-flex items-center gap-1 rounded px-1.5 py-px text-[10px]"
                title={`Offered in: ${c.contexts.map((x) => CONTEXT_LABELS[x] ?? x).join(", ")}`}
              >
                <span className="text-foreground/80">{c.name}</span>
                {key && <span className="font-mono">{formatCombo(key)}</span>}
              </li>
            );
          })}
        </ul>
      </div>
      <Switch
        size="sm"
        checked={enabled}
        aria-label={`${enabled ? "Disable" : "Enable"} ${plugin.name}`}
        onClick={(e) => e.stopPropagation()}
        onCheckedChange={(on) => void setPluginEnabled(plugin, on)}
        className="mt-0.5"
      />
    </div>
  );
}

/** Plugins ▸ Manage Plugins… (⌘L): the built-in and installed plugins, install / remove / reload. */
export function PluginManager() {
  const open = usePluginsStore((s) => s.managerOpen);
  const list = usePluginsStore((s) => s.list);
  const loading = usePluginsStore((s) => s.loading);
  const error = usePluginsStore((s) => s.error);
  const states = usePrefs((p) => p.pluginStates);
  const plugins = useMemo(() => pluginEntries(list), [list]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [removing, setRemoving] = useState<PluginEntry | null>(null);
  const selected = plugins.find((p) => p.identifier === selectedId) ?? null;
  const builtin = plugins.filter((p) => p.builtin);
  const installed = plugins.filter((p) => !p.builtin);

  const row = (p: PluginEntry) => (
    <PluginRow
      key={p.identifier}
      plugin={p}
      enabled={isPluginEnabled(states, p.identifier, p.builtin)}
      selected={p.identifier === selectedId}
      onSelect={() => setSelectedId(p.identifier)}
    />
  );

  return (
    <>
      <Dialog open={open} onOpenChange={(o) => !o && closePluginManager()}>
        <DialogContent className="flex max-h-[85vh] flex-col gap-3 sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle className="text-sm">Plugins</DialogTitle>
            <DialogDescription className="text-xs">
              Plugins add commands to the Plugins menu, the grid and sidebar menus and the command
              palette. Third-party plugins are not reviewed by Tabler: enable only the ones you
              trust.
            </DialogDescription>
          </DialogHeader>
          <div
            className="-mx-1 min-h-0 flex-1 overflow-y-auto px-1"
            role="listbox"
            aria-label="Plugins"
          >
            <h3 className="text-muted-foreground mb-1 px-1 text-[11px] font-medium tracking-wide uppercase">
              Built-in
            </h3>
            {builtin.map(row)}
            <h3 className="text-muted-foreground mt-3 mb-1 flex items-center gap-1.5 px-1 text-[11px] font-medium tracking-wide uppercase">
              Installed
              {loading && <Loader2 className="size-3 animate-spin" aria-label="Loading" />}
            </h3>
            {installed.map(row)}
            {installed.length === 0 && !loading && (
              <p className="text-muted-foreground px-2.5 py-2 text-xs">
                No plugins installed. Use Install from Folder… with a folder holding a manifest.json
                and its scripts.
              </p>
            )}
            {error && <p className="text-error px-2.5 py-1 text-xs">{error}</p>}
            {(list?.invalid ?? []).map((bad) => (
              <div key={bad.folder} className="px-2.5 py-1.5 text-xs">
                <div className="text-muted-foreground truncate font-mono text-[11px]">
                  {bad.folder}
                </div>
                <div className="text-error">{bad.error}</div>
              </div>
            ))}
            <h3 className="text-muted-foreground mt-3 mb-1 px-1 text-[11px] font-medium tracking-wide uppercase">
              Built into Tabler
            </h3>
            {BUILTIN_FEATURES.map((f) => (
              <div key={f.name} className="px-2.5 py-1.5 text-xs">
                <span className="font-medium">{f.name}</span>
                <span className="text-muted-foreground"> — {f.description} </span>
                <span className="text-muted-foreground italic">({f.where})</span>
              </div>
            ))}
          </div>
          {list && (
            <p className="text-muted-foreground truncate font-mono text-[10px]" title={list.dir}>
              {list.dir}
            </p>
          )}
          <DialogFooter className="flex-row items-center gap-1.5 sm:justify-between">
            <div className="flex flex-wrap items-center gap-1.5">
              <Button size="sm" variant="outline" onClick={() => void installPluginFromFolder()}>
                <Upload /> Install from Folder…
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={!selected || selected.builtin}
                onClick={() => selected && setRemoving(selected)}
              >
                <Trash2 /> Remove
              </Button>
              <Button size="sm" variant="ghost" onClick={() => void refreshPlugins()}>
                <RotateCw /> Reload
              </Button>
              <Button size="sm" variant="ghost" onClick={() => void revealPluginsFolder()}>
                <FolderOpen /> Open Plugins Folder
              </Button>
            </div>
            <Button size="sm" onClick={closePluginManager}>
              Done
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <AlertDialog open={removing !== null} onOpenChange={(o) => !o && setRemoving(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove “{removing?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              Its folder is deleted from the Plugins folder and its commands leave the menus.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                const plugin = removing;
                setRemoving(null);
                setSelectedId(null);
                if (plugin) void removePlugin(plugin);
              }}
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
