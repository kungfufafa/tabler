import { Copy, Database, Loader2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ipc, toIpcError, type ConnectionProfile } from "@/lib/ipc";
import { basename, formatBytes } from "./options";
import { pickSqliteCopyPath, revealInFinder } from "./pickers";
import type { BackupMode } from "./store";

/** SQLite has no dump binary: its database is backed up by copying the file. */
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
  const copy = async () => {
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
      const size = await ipc.backup.copySqlite(profile.id, dest);
      toast.success(`Copied ${profile.name}`, {
        description: `${basename(dest)} · ${formatBytes(size)}`,
        action: {
          label: "Show in Finder",
          onClick: () => void revealInFinder(dest).catch(() => undefined),
        },
      });
    } catch (e) {
      toast.error("Could not copy the database file", { description: toIpcError(e).message });
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
            {file}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2">
        {mode === "backup" && (
          <Button type="button" size="sm" onClick={() => void copy()} disabled={copying || !file}>
            {copying ? <Loader2 className="animate-spin" /> : <Copy />}
            Copy Database File…
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
