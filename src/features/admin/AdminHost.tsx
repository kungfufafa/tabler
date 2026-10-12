import { useEffect, useMemo } from "react";
import { useDisconnect } from "@/features/connections/useConnections";
import { actions, useAction } from "@/features/palette/actions";
import { disconnectGuarded } from "@/features/palette/commands";
import { isShortcutDispatch } from "@/lib/keys/useShortcuts";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import { getQueryTab, isBusy } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { CreateDatabaseDialog, DropDatabaseDialog, RenameDatabaseDialog } from "./DatabaseDialogs";
import { ProcessListDialog } from "./ProcessList";
import { CreateSchemaDialog, DropSchemaDialog } from "./SchemaDialogs";
import {
  closeAdmin,
  openCreateDatabase,
  openCreateSchema,
  openDropDatabase,
  openDropSchema,
  openProcessList,
  openUserManagement,
  useAdminStore,
} from "./store";
import { UserManagementDialog } from "./UserManagement";

/**
 * The Connection menu's actions (Process List… ⌘., User Management…, Disconnect) and the
 * database / schema commands of the active connection, plus the one admin window or dialog open
 * at a time. Mount once.
 */
export function AdminHost() {
  useAdminActions();
  const request = useAdminStore((s) => s.request);
  const connection = useConnectionsStore((s) =>
    request ? (s.byId[request.connectionId] ?? null) : null,
  );
  const gone = request !== null && connection === null;

  // The connection went away (disconnect, lost): nothing left to administer.
  useEffect(() => {
    if (gone) closeAdmin();
  }, [gone]);

  if (!request || !connection) return null;
  const { connectionId, panel, target } = request;
  const info = connection.info;
  const key = `${panel}:${connectionId}:${target ?? ""}`;
  switch (panel) {
    case "processList":
      return (
        <ProcessListDialog
          key={key}
          connectionId={connectionId}
          driver={info.driver}
          connectionName={info.name}
          onClose={closeAdmin}
        />
      );
    case "users":
      return (
        <UserManagementDialog
          key={key}
          connectionId={connectionId}
          driver={info.driver}
          connectionName={info.name}
          currentDatabase={info.currentDatabase}
          currentUser={info.currentUser}
          onClose={closeAdmin}
        />
      );
    case "createDatabase":
      return (
        <CreateDatabaseDialog
          key={key}
          connectionId={connectionId}
          driver={info.driver}
          onClose={closeAdmin}
        />
      );
    case "renameDatabase":
      return (
        <RenameDatabaseDialog
          key={key}
          connectionId={connectionId}
          database={target ?? ""}
          currentDatabase={info.currentDatabase}
          onClose={closeAdmin}
        />
      );
    case "dropDatabase":
      return (
        <DropDatabaseDialog
          key={key}
          connectionId={connectionId}
          database={target ?? null}
          currentDatabase={info.currentDatabase}
          onClose={closeAdmin}
        />
      );
    case "createSchema":
      return (
        <CreateSchemaDialog
          key={key}
          connectionId={connectionId}
          currentUser={info.currentUser}
          onClose={closeAdmin}
        />
      );
    case "dropSchema":
      return (
        <DropSchemaDialog
          key={key}
          connectionId={connectionId}
          schema={target ?? null}
          onClose={closeAdmin}
        />
      );
  }
}

/**
 * ⌘. while the SQL tab on screen runs a query stops it instead of opening the Process List,
 * wherever the focus is in that tab (the editor has its own ⌘. Cancel; the results grid and the
 * rest reach this). Connection ▸ Process List… and the palette always open the list.
 */
function cancelsRunningQuery(): boolean {
  if (!isShortcutDispatch()) return false;
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  if (tab?.kind !== "query" || !isBusy(getQueryTab(tab.id))) return false;
  if (!actions.has("query.cancel")) return false;
  void actions.run("query.cancel");
  return true;
}

/** Registers the admin actions while a connection is active (only those its driver supports). */
function useAdminActions() {
  const active = useActiveConnection();
  const connectionId = active?.info.connectionId ?? null;
  const driver = active?.info.driver ?? null;
  const { mutate: disconnect } = useDisconnect();
  const handlers = useMemo(() => {
    if (!connectionId) return null;
    const server = driver === "postgres" || driver === "mysql";
    const pg = driver === "postgres";
    return {
      processList: () => {
        if (!cancelsRunningQuery()) openProcessList(connectionId);
      },
      users: () => openUserManagement(connectionId),
      newDatabase: server ? () => openCreateDatabase(connectionId) : null,
      dropDatabase: server ? () => openDropDatabase(connectionId) : null,
      newSchema: pg ? () => openCreateSchema(connectionId) : null,
      dropSchema: pg ? () => openDropSchema(connectionId) : null,
      disconnect: () => void disconnectGuarded(connectionId, disconnect),
    };
  }, [connectionId, driver, disconnect]);

  useAction("admin.process-list", handlers?.processList);
  useAction("admin.users", handlers?.users);
  useAction("admin.new-database", handlers?.newDatabase);
  useAction("admin.drop-database", handlers?.dropDatabase);
  useAction("admin.new-schema", handlers?.newSchema);
  useAction("admin.drop-schema", handlers?.dropSchema);
  useAction("connection.disconnect", handlers?.disconnect);
}
