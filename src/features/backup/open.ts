import { allowFeature } from "@/lib/caps";
import { useConnectionsStore } from "@/stores/connections";
import { openBackup, type BackupMode } from "./store";

/**
 * Opens the window on the active connection and its database (MySQL: the database chosen in the
 * sidebar); without one the first saved PostgreSQL / MySQL connection is preselected.
 */
export function openBackupForActive(mode: BackupMode): void {
  const { activeId, byId } = useConnectionsStore.getState();
  const active = activeId ? byId[activeId] : undefined;
  if (!active) {
    openBackup({ mode, profileId: null, database: null });
    return;
  }
  const { info, currentSchema } = active;
  if (!allowFeature(info.connectionId, mode === "restore" ? "restore" : "backup")) return;
  const database =
    info.driver === "mysql" ? (currentSchema ?? info.currentDatabase) : info.currentDatabase;
  openBackup({ mode, profileId: info.profileId, database: database ?? null });
}
