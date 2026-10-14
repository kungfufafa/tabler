/**
 * Where plugin commands show up besides the native Plugins menu: the grid's cell menu and the
 * sidebar's item menu (Plugins ▸), the palette (">Plugin: <name>: <command>") and their keys.
 */
import { Puzzle } from "lucide-react";
import type { ReactNode } from "react";
import {
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
import type { GridMenuContext } from "@/features/datagrid/menuExtensions";
import type { EditorMenuContext } from "@/features/editor/menuExtensions";
import { useLockStore } from "@/features/lock/store";
import type { MenuEntry } from "@/features/objects/menu";
import type { PaletteItem } from "@/features/palette/items";
import { comboFromEvent, formatCombo, keyFromEvent } from "@/lib/keys/combo";
import { shortcutFor, type Scope } from "@/lib/keys/keymap";
import { isCapturingShortcut, resolveScope } from "@/lib/keys/useShortcuts";
import { getPrefs } from "@/stores/prefs";
import {
  captureSidebarTarget,
  editorInvocation,
  focusInvocation,
  gridInvocation,
  lastSidebarTarget,
  sidebarInvocation,
  type SidebarTarget,
} from "./invocation";
import type { KeyOf } from "./menuModel";
import {
  commandsFor,
  groupByPlugin,
  paletteLabel,
  pluginCommands,
  type CommandRef,
} from "./registry";
import { runPluginCommand } from "./run";
import { usePluginsStore } from "./store";
import type { PluginContextKind, PluginInvocation } from "./types";

/** The enabled commands right now. */
export function enabledCommands(): CommandRef[] {
  return pluginCommands(usePluginsStore.getState().list, getPrefs().pluginStates);
}

/** A command's key: its keymap action's binding (SQL Format ⌃I), else its manifest key. */
export const effectiveKey: KeyOf = (c) =>
  c.builtin?.action ? (shortcutFor(c.builtin.action) ?? null) : c.key;

const locked = () => useLockStore.getState().status === "locked";

/**
 * Runs a command once the menu that chose it has closed (its focus restored), so a dialog the
 * command opens keeps the focus.
 */
export function runSoon(runId: string, inv: PluginInvocation): void {
  if (locked()) return;
  requestAnimationFrame(() => setTimeout(() => void runPluginCommand(runId, inv), 0));
}

/** The Plugins menu, the palette and keys: the command works on what has the focus. */
export function runFromFocus(runId: string): void {
  if (locked()) return;
  const scope = resolveScope(document.activeElement);
  if (scope === "dialog") return;
  if (scope === "sidebar") {
    captureSidebarTarget(document.activeElement);
    return runSoon(runId, sidebarInvocation());
  }
  const source: PluginContextKind =
    scope === "editor" ? "editor" : scope === "grid" ? "grid" : "global";
  runSoon(runId, focusInvocation(source));
}

// --- the grid's cell menu and the SQL editor's context menu ---------------------------------------

function gridItem(command: CommandRef, onRun: (c: CommandRef) => void): ReactNode {
  const key = effectiveKey(command);
  return (
    <ContextMenuItem key={command.runId} onSelect={() => onRun(command)}>
      {command.name}
      {key && <ContextMenuShortcut>{formatCombo(key)}</ContextMenuShortcut>}
    </ContextMenuItem>
  );
}

/** A separator and "Plugins ▸" with the commands, a plugin with several getting its own submenu. */
function pluginSubmenu(commands: CommandRef[], onRun: (c: CommandRef) => void): ReactNode {
  return (
    <>
      <ContextMenuSeparator />
      <ContextMenuSub>
        <ContextMenuSubTrigger>
          <Puzzle /> Plugins
        </ContextMenuSubTrigger>
        <ContextMenuSubContent className="min-w-48">
          {groupByPlugin(commands).map((group) =>
            group.length === 1 ? (
              gridItem(group[0]!, onRun)
            ) : (
              <ContextMenuSub key={group[0]!.plugin.identifier}>
                <ContextMenuSubTrigger>{group[0]!.plugin.name}</ContextMenuSubTrigger>
                <ContextMenuSubContent className="min-w-44">
                  {group.map((c) => gridItem(c, onRun))}
                </ContextMenuSubContent>
              </ContextMenuSub>
            ),
          )}
        </ContextMenuSubContent>
      </ContextMenuSub>
    </>
  );
}

/** Plugins ▸ at the end of a grid's cell menu (the commands with the "grid" context). */
export function gridPluginMenu(context: () => GridMenuContext): ReactNode {
  const commands = commandsFor(enabledCommands(), "grid");
  if (commands.length === 0) return null;
  return pluginSubmenu(commands, (c) => runSoon(c.runId, gridInvocation(context())));
}

/**
 * Plugins ▸ at the end of a SQL editor pane's context menu (the commands with the "editor"
 * context): they read the editor's text, selection and current statement
 * (`context.currentSql()` / `selectedSql()` / `currentStatement()`).
 */
export function editorPluginMenu({ tabId }: EditorMenuContext): ReactNode {
  const commands = commandsFor(enabledCommands(), "editor");
  if (commands.length === 0) return null;
  // Read when the item is chosen (the selection the user made), then run once the menu closed.
  return pluginSubmenu(commands, (c) => runSoon(c.runId, editorInvocation(tabId)));
}

// --- the sidebar's item menu -----------------------------------------------------------------------

/** Plugins ▸ at the end of a table / view menu (the commands with the "sidebar" context). */
export function sidebarPluginEntries(
  target: SidebarTarget | null = lastSidebarTarget(),
): MenuEntry[] {
  const commands = commandsFor(enabledCommands(), "sidebar");
  if (commands.length === 0) return [];
  const item = (c: CommandRef): MenuEntry => ({
    kind: "item",
    id: `plugin:${c.runId}`,
    label: c.name,
    shortcut: effectiveKey(c) ?? undefined,
    onSelect: () => runSoon(c.runId, sidebarInvocation(target)),
  });
  return [
    { kind: "separator", id: "plugins-sep" },
    {
      kind: "sub",
      id: "plugins",
      label: "Plugins",
      icon: Puzzle,
      items: groupByPlugin(commands).map((group) =>
        group.length === 1
          ? item(group[0]!)
          : {
              kind: "sub",
              id: `plugin-group:${group[0]!.plugin.identifier}`,
              label: group[0]!.plugin.name,
              items: group.map(item),
            },
      ),
    },
  ];
}

// --- the palette -----------------------------------------------------------------------------------

/** ">Plugin: <name>: <command>" for every enabled command (keymap-owned ones are listed already). */
export function palettePluginItems(): PaletteItem[] {
  return enabledCommands()
    .filter((c) => !c.builtin?.action)
    .map((c) => ({
      key: `plugin:${c.runId}`,
      group: "actions",
      label: paletteLabel(c),
      keywords: ["plugin", c.plugin.name, c.name],
      combo: effectiveKey(c) ?? undefined,
      icon: { type: "action", icon: Puzzle },
      run: () => runFromFocus(c.runId),
      verb: "Run",
    }));
}

// --- keys --------------------------------------------------------------------------------------------

/** Whether a command with `contexts` answers its key while focus is in `scope`. */
export function keyApplies(contexts: readonly PluginContextKind[], scope: Scope): boolean {
  if (scope === "dialog" || scope === "grid-edit" || scope === "filter") return false;
  if (contexts.includes("global")) return true;
  return (
    (scope === "editor" && contexts.includes("editor")) ||
    (scope === "grid" && contexts.includes("grid")) ||
    (scope === "sidebar" && contexts.includes("sidebar"))
  );
}

/**
 * Keydown (capture, after the app's own shortcuts): runs the installed command bound to the key.
 * Keys the keymap owns (SQL Format ⌃I) go through the action registry instead.
 */
export function onPluginKey(e: KeyboardEvent): void {
  if (e.defaultPrevented || isCapturingShortcut() || locked()) return;
  if (!e.metaKey && !e.ctrlKey && !e.altKey && !/^F\d+$/.test(keyFromEvent(e))) return;
  const combo = comboFromEvent(e);
  const target = e.target instanceof Element ? e.target : document.activeElement;
  const scope = resolveScope(target);
  const command = enabledCommands().find(
    (c) => !c.builtin?.action && c.key === combo && keyApplies(c.contexts, scope),
  );
  if (!command) return;
  e.preventDefault();
  e.stopPropagation();
  runFromFocus(command.runId);
}
