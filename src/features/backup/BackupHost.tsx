import { useEffect } from "react";
import { useAction } from "@/features/palette/actions";
import { createJobToast } from "@/features/export/jobToast";
import { useSingleHost } from "@/features/export/singleHost";
import { BackupDialog } from "./BackupDialog";
import { openBackupForActive } from "./open";
import { backupProgressToast, reopenBackup, useBackupStore } from "./store";

/** Name of the single-host slot `BackupHost` registers. */
const BACKUP_HOST = "backup-window";

/** Toast id of the progress of a job whose window was closed. */
const BACKUP_PROGRESS_TOAST = "backup-progress";

const openBackupWindow = () => openBackupForActive("backup");
const openRestoreWindow = () => openBackupForActive("restore");

/**
 * Mounts the Backup / Restore window and its actions (File ▸ Backup… / Restore…, the palette).
 * Mount once near the app root; extra instances stay inert. While a job runs with the window
 * closed, a toast shows its progress, with "Show" to bring the window back.
 */
export function BackupHost() {
  const owner = useSingleHost(BACKUP_HOST);
  useAction("backup.open", owner ? openBackupWindow : null);
  useAction("restore.open", owner ? openRestoreWindow : null);
  useEffect(() => {
    if (!owner) return;
    const job = createJobToast({ id: BACKUP_PROGRESS_TOAST, onShow: reopenBackup });
    job.sync(backupProgressToast(useBackupStore.getState()));
    const off = useBackupStore.subscribe((s) => job.sync(backupProgressToast(s)));
    return () => {
      off();
      job.sync(null);
    };
  }, [owner]);
  return owner ? <BackupDialog /> : null;
}
