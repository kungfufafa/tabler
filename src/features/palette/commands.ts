import { useQueryClient, type QueryClient, type QueryKey } from "@tanstack/react-query";
import {
  DatabaseZap,
  Keyboard,
  Monitor,
  Moon,
  Plus,
  Sun,
  Unplug,
  type LucideIcon,
} from "lucide-react";
import { useMemo } from "react";
import { toast } from "sonner";
import { closeConnectionTabs } from "@/features/connections/closeGuard";
import { openConnectionDialog } from "@/features/connections/store";
import { useConnect, useDisconnect } from "@/features/connections/useConnections";
import { openSettings } from "@/features/settings/store";
import { switchDatabase } from "@/features/sidebar/switchDatabase";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc, type ConnectionProfile, type TableRef } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type OpenTableOptions, type Tab } from "@/stores/tabs";
import { useUiStore, type ThemePref } from "@/stores/ui";
import { actions, type ActionId } from "./actions";
import { actionIcon, type PaletteItem } from "./items";
import { usePaletteStore } from "./store";

/** Side effects the palette items trigger. Built once by the always-mounted palette shell. */
export interface PaletteCommands {
  /** Shows the table's rows: in the connection's preview tab with `preview`, else a tab of its own. */
  openTable: (connectionId: string, table: TableRef, options?: OpenTableOptions) => void;
  activateTab: (tab: Tab) => void;
  /** Resolves once the connect settled (the hook reports failures itself). */
  connect: (profile: ConnectionProfile) => Promise<void>;
  switchConnection: (connectionId: string) => void;
  /** Shows another schema (PostgreSQL) / attached database (SQLite) in the sidebar. */
  selectSchema: (connectionId: string, schema: string) => void;
  /**
   * Points the connection at another database (asks first when its tabs hold unsaved work).
   * Resolves once the switch settled; failures are reported as a toast.
   */
  switchDatabase: (connectionId: string, database: string) => Promise<void>;
  /** Resolves true when the connection was disconnected, false when the user kept it. */
  disconnect: (connectionId: string) => Promise<boolean>;
  reloadSchema: (connectionId: string) => Promise<void>;
  setTheme: (pref: ThemePref) => void;
  /** Shows the connection manager with the New connection form open. */
  newConnection: () => void;
  openShortcuts: () => void;
  runAction: (id: ActionId) => Promise<unknown>;
}

/**
 * Drops every cached piece of schema metadata for a connection (backend cache + react-query).
 * Rejects when the backend cache could not be dropped or an observed query failed to refetch, so
 * callers never report a reload that did not happen.
 */
export async function reloadSchema(qc: QueryClient, connectionId: string): Promise<void> {
  let backendError: unknown = null;
  try {
    await ipc.schema.invalidate(connectionId);
  } catch (e) {
    // Refresh the renderer caches regardless, then report the failure.
    backendError = e;
  }
  const keys: QueryKey[] = [
    ["tables", connectionId],
    schemaKeys.schemas(connectionId),
    schemaKeys.databases(connectionId),
    ["table", connectionId],
    schemaKeys.completion(connectionId),
  ];
  // invalidateQueries swallows refetch errors; inspect what the visible queries ended up with.
  await Promise.all(keys.map((queryKey) => qc.invalidateQueries({ queryKey })));
  const failed = keys
    .flatMap((queryKey) => qc.getQueryCache().findAll({ queryKey, type: "active" }))
    .find((q) => q.state.status === "error");
  if (failed) throw failed.state.error ?? new Error("The schema could not be refreshed");
  if (backendError !== null) throw backendError;
}

/**
 * Disconnects only after the connection's tabs went through the regular close guard, so staged
 * edits and unsaved queries get the "Discard changes?" prompt first. Cancelling (or reviewing)
 * keeps the connection open. Resolves true when `disconnect` was called.
 */
export function disconnectGuarded(
  connectionId: string,
  disconnect: (connectionId: string) => void,
): Promise<boolean> {
  return closeConnectionTabs(connectionId, () => disconnect(connectionId));
}

const errorMessage = (e: unknown) => (e instanceof Error ? e.message : String(e));

function showWorkspace() {
  useUiStore.getState().setView("workspace");
}

export function usePaletteCommands(): PaletteCommands {
  const qc = useQueryClient();
  // mutateAsync, not mutate + per-call callbacks: a second mutate detaches the observer from the
  // first mutation, whose per-call callbacks then never run.
  const { mutateAsync: connect } = useConnect();
  const { mutate: disconnect } = useDisconnect();
  return useMemo<PaletteCommands>(
    () => ({
      openTable: (connectionId, table, options) => {
        useTabsStore
          .getState()
          .openTable(connectionId, { schema: table.schema, name: table.name }, "table", options);
        showWorkspace();
      },
      activateTab: (tab) => {
        // Keep the sidebar in sync with the tab's connection.
        const conns = useConnectionsStore.getState();
        if (conns.byId[tab.connectionId] && conns.activeId !== tab.connectionId)
          conns.setActive(tab.connectionId);
        useTabsStore.getState().activate(tab.id);
        showWorkspace();
      },
      connect: async (profile) => {
        const palette = usePaletteStore.getState();
        // A connect for this profile is already on its way; another would open a second session.
        if (!palette.beginConnect(profile.id)) return;
        const id = toast.loading(`Connecting to ${profile.name}…`);
        try {
          await connect(profile.id);
        } catch {
          // useConnect's onError already shows what went wrong.
        } finally {
          toast.dismiss(id);
          palette.endConnect(profile.id);
        }
      },
      switchConnection: (connectionId) => {
        useConnectionsStore.getState().setActive(connectionId);
        showWorkspace();
      },
      selectSchema: (connectionId, schema) => {
        useConnectionsStore.getState().setCurrentSchema(connectionId, schema);
        showWorkspace();
      },
      switchDatabase: async (connectionId, database) => {
        if (await switchDatabase(qc, connectionId, database)) showWorkspace();
      },
      disconnect: (connectionId) => disconnectGuarded(connectionId, disconnect),
      reloadSchema: async (connectionId) => {
        try {
          await reloadSchema(qc, connectionId);
        } catch (e) {
          toast.error("Could not reload schema", { description: errorMessage(e) });
          return;
        }
        toast.success("Schema reloaded");
      },
      setTheme: (pref) => useUiStore.getState().setThemePref(pref),
      newConnection: () => {
        useUiStore.getState().setView("launcher");
        openConnectionDialog();
      },
      openShortcuts: () => openSettings("shortcuts"),
      runAction: (id) => actions.run(id),
    }),
    [qc, connect, disconnect],
  );
}

const THEMES: Array<{ pref: ThemePref; label: string; icon: LucideIcon }> = [
  { pref: "light", label: "Theme: Light", icon: Sun },
  { pref: "dark", label: "Theme: Dark", icon: Moon },
  { pref: "system", label: "Theme: System", icon: Monitor },
];

/**
 * Registered actions without a keyboard shortcut that the palette lists as commands (the
 * Connection menu and the ⌘K database list's commands). Shown only while they have a handler,
 * i.e. when the active connection's driver supports them.
 */
export const PALETTE_ONLY_ACTIONS = [
  "admin.users",
  "admin.new-database",
  "admin.drop-database",
  "admin.new-schema",
  "admin.drop-schema",
  "backup.open",
  "restore.open",
] as const satisfies readonly ActionId[];

const ADMIN_COMMANDS: Record<
  (typeof PALETTE_ONLY_ACTIONS)[number],
  { label: string; keywords: string; noRecent?: boolean }
> = {
  "admin.users": {
    label: "User Management…",
    keywords: "users roles accounts privileges grants permissions password",
  },
  "admin.new-database": { label: "New Database…", keywords: "create add database" },
  "admin.drop-database": {
    label: "Drop Database…",
    keywords: "delete remove database",
    noRecent: true,
  },
  "admin.new-schema": { label: "New Schema…", keywords: "create add schema namespace" },
  "admin.drop-schema": {
    label: "Drop Schema…",
    keywords: "delete remove schema namespace cascade",
    noRecent: true,
  },
  "backup.open": { label: "Backup…", keywords: "dump pg_dump mysqldump export database file" },
  "restore.open": { label: "Restore…", keywords: "load dump pg_restore psql mysql import file" },
};

/** Actions that only exist in the palette (no keyboard shortcut). */
export function paletteCommandItems(ctx: {
  themePref: ThemePref;
  /** The active connection, if any. */
  active: { connectionId: string; name: string; schema: string | null } | null;
  commands: Pick<
    PaletteCommands,
    "setTheme" | "newConnection" | "openShortcuts" | "disconnect" | "reloadSchema"
  > &
    Partial<Pick<PaletteCommands, "runAction">>;
  /** Whether an action has a handler (lists `PALETTE_ONLY_ACTIONS`); none listed without it. */
  has?: (id: ActionId) => boolean;
}): PaletteItem[] {
  const { commands, active } = ctx;
  const items: PaletteItem[] = THEMES.map(({ pref, label, icon }) => ({
    key: `cmd:theme.${pref}`,
    group: "actions",
    label,
    keywords: ["appearance color scheme toggle theme", pref === "dark" ? "dark mode" : ""],
    icon: { type: "action", icon },
    current: ctx.themePref === pref,
    run: () => commands.setTheme(pref),
    verb: "Apply",
  }));
  items.push({
    key: "cmd:connection.new",
    group: "actions",
    label: "New connection…",
    keywords: ["add create connection profile launcher manager"],
    icon: { type: "action", icon: Plus },
    run: () => commands.newConnection(),
    verb: "Run",
  });
  items.push({
    key: "cmd:settings.shortcuts",
    group: "actions",
    label: "Settings: Shortcuts",
    keywords: ["keyboard keymap key bindings hotkeys preferences"],
    icon: { type: "action", icon: Keyboard },
    run: () => commands.openShortcuts(),
    verb: "Open",
  });
  if (active) {
    items.push(
      {
        key: "cmd:schema.reload",
        group: "actions",
        label: "Reload schema",
        detail: active.schema ?? undefined,
        keywords: ["refresh tables metadata cache"],
        icon: { type: "action", icon: DatabaseZap },
        run: () => commands.reloadSchema(active.connectionId),
        verb: "Run",
      },
      {
        key: "cmd:connection.disconnect",
        group: "actions",
        label: "Disconnect",
        detail: active.name,
        keywords: ["close connection"],
        icon: { type: "action", icon: Unplug },
        run: () => commands.disconnect(active.connectionId),
        verb: "Run",
        noRecent: true,
      },
    );
  }
  const run = commands.runAction;
  if (ctx.has && run)
    for (const id of PALETTE_ONLY_ACTIONS) {
      if (!ctx.has(id)) continue;
      const c = ADMIN_COMMANDS[id];
      items.push({
        key: `action:${id}`,
        group: "actions",
        label: c.label,
        detail: active?.name,
        keywords: [c.keywords],
        icon: { type: "action", icon: actionIcon(id) },
        run: () => run(id),
        verb: "Run",
        ...(c.noRecent ? { noRecent: true } : {}),
      });
    }
  return items;
}
