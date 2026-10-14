import { CloudDownload, Copy, Database, Loader2, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { announceFileDone } from "@/features/notify/done";
import { notifyInBackground } from "@/features/notify/notify";
import { ipc, toIpcError, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { basename, formatBytes } from "./options";
import { pickSqliteCopyPath } from "./pickers";
import type { BackupMode } from "./store";

/**
 * SQLite has no dump binary: its database is backed up by copying the file. A SQLite file over SSH
 * is copied from the local cache an open connection downloaded (the panel says so, and offers
 * "Download fresh copy first"), or downloaded from the SSH server for the backup.
 */
export function SqlitePanel({
  profile,
  mode,
  onChooseOther,
}: {
  profile: ConnectionProfile;
  mode: BackupMode;
  /** Shows the PostgreSQL / MySQL connections instead; null when there are none. */
  onChooseOther: (() => void) | null;
}) {
  const [copying, setCopying] = useState(false);
  const file = profile.sqlitePath ?? "";
  const overSsh = profile.ssh !== null;
  // The connection's downloaded copy is what a backup copies unless a fresh one is asked for.
  const cached = useConnectionsStore((s) =>
    s.order.some((id) => s.byId[id]?.info.profileId === profile.id),
  );
  const copy = async (fresh = false) => {
    const stem = basename(file).replace(/\.(sqlite3?|db)$/i, "") || "database";
    const date = new Date().toISOString().slice(0, 10);
    let dest: string | null;
    try {
      dest = await pickSqliteCopyPath(`${stem}-${date}.sqlite`);
    } catch (e) {
      toast.error("Could not open the save dialog", { description: toIpcError(e).message });
      return;
    }
    if (!dest) return;
    setCopying(true);
    try {
      const size = await (overSsh
        ? ipc.backup.copySqlite(profile.id, dest, fresh)
        : ipc.backup.copySqlite(profile.id, dest));
      const cachedCopy = overSsh && cached && !fresh;
      announceFileDone({
        title: `Copied ${profile.name}`,
        popupTitle: "Backup completed",
        description: `${basename(dest)} · ${formatBytes(size)}${cachedCopy ? " · cached copy" : ""}`,
        path: dest,
      });
    } catch (e) {
      const message = toIpcError(e).message;
      void notifyInBackground({ title: `Backup of ${profile.name} failed`, body: message });
      toast.error("Could not copy the database file", { description: message });
    } finally {
      setCopying(false);
    }
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-8 py-10 text-center">
      <Database className="text-muted-foreground size-8" />
      <div className="flex max-w-md flex-col gap-1">
        <p className="text-sm font-medium">
          {mode === "backup"
            ? "SQLite databases are backed up by copying the database file."
            : "SQLite databases are restored by replacing the database file."}
        </p>
        <p className="text-muted-foreground text-xs">
          {mode === "backup"
            ? "There is no dump tool to run: the whole database lives in one file."
            : "Close the connection, then copy the backup over the database file (or open the copy as a new connection)."}
        </p>
        {file && (
          <p className="text-muted-foreground font-mono text-[11px] break-all" title={file}>
            {overSsh && profile.ssh ? `${profile.ssh.host}:${file}` : file}
          </p>
        )}
        {overSsh && mode === "backup" && (
          <p
            role="note"
            className="text-muted-foreground mt-1 flex items-start gap-1.5 text-left text-[11px]"
          >
            <TriangleAlert className="text-warning mt-0.5 size-3 shrink-0" />
            {cached
              ? "This database is on an SSH server. Copying it backs up the cached copy Tabler downloaded when the connection was opened: changes made on the server since then are not in it. Download a fresh copy first to include them."
              : "This database is on an SSH server. Its file is downloaded for the backup, over SFTP."}
          </p>
        )}
      </div>
      <div className="flex flex-wrap items-center justify-center gap-2">
        {mode === "backup" && (
          <Button type="button" size="sm" onClick={() => void copy()} disabled={copying || !file}>
            {copying ? <Loader2 className="animate-spin" /> : <Copy />}
            {overSsh && cached
              ? "Copy Cached File…"
              : overSsh
                ? "Download and Copy…"
                : "Copy Database File…"}
          </Button>
        )}
        {mode === "backup" && overSsh && cached && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void copy(true)}
            disabled={copying || !file}
          >
            <CloudDownload />
            Download Fresh Copy First…
          </Button>
        )}
        {onChooseOther && (
          <Button type="button" variant="outline" size="sm" onClick={onChooseOther}>
            Choose another connection
          </Button>
        )}
      </div>
    </div>
  );
}
