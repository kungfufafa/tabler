import { useAction } from "@/features/palette/actions";
import { useSingleHost } from "@/features/export/singleHost";
import { BackupDialog } from "./BackupDialog";
import { openBackupForActive } from "./open";

/** Name of the single-host slot `BackupHost` registers. */
const BACKUP_HOST = "backup-window";

const openBackupWindow = () => openBackupForActive("backup");
const openRestoreWindow = () => openBackupForActive("restore");

/**
 * Mounts the Backup / Restore window and its actions (File ▸ Backup… / Restore…, the palette).
 * Mount once near the app root; extra instances stay inert.
 */
export function BackupHost() {
  const owner = useSingleHost(BACKUP_HOST);
  useAction("backup.open", owner ? openBackupWindow : null);
  useAction("restore.open", owner ? openRestoreWindow : null);
  return owner ? <BackupDialog /> : null;
}
