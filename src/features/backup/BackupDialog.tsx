import { useQueryClient } from "@tanstack/react-query";
import { CircleAlert, DatabaseBackup, FileUp, Loader2, Undo2 } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useProfiles } from "@/features/connections/useConnections";
import { ipc, toIpcError, type ConnectionProfile, type RestoreFileInfo } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ConnectionList } from "./ConnectionList";
import { DatabaseList } from "./DatabaseList";
import { OptionChips } from "./OptionChips";
import {
  backupExtension,
  backupProblem,
  backupTool,
  basename,
  dirname,
  fileKindLabel,
  formatBytes,
  gzipApplies,
  isDumpDriver,
  loadBackupOptions,
  loadLastDir,
  loadRestoreOptions,
  pickBinary,
  restoreFileProblem,
  restoreTool,
  saveBackupOptions,
  saveLastDir,
  saveRestoreOptions,
  type BackupOptions,
  type DumpDriver,
  type RestoreOptions,
  type RestoreTool,
} from "./options";
import { Field } from "./parts";
import { backupFileName } from "./pattern";
import { PatternField } from "./PatternField";
import { pickBackupFolder, pickRestoreFile } from "./pickers";
import { RunPanel } from "./RunPanel";
import { SqlitePanel } from "./SqlitePanel";
import {
  cancelJob,
  closeBackup,
  dismissRun,
  releaseBackupWindow,
  startJob,
  useBackupStore,
  type BackupRequest,
} from "./store";
import { useBackupTarget, useDumpBinaries } from "./useBackupData";
import { VersionPicker } from "./VersionPicker";

const NO_PROFILES: ConnectionProfile[] = [];

/** The current time, ticking every second (the file name preview has seconds). */
function useClock(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function Footer({ children, left }: { children: ReactNode; left?: ReactNode }) {
  return (
    <div className="border-border bg-surface-2/60 flex min-h-12 items-center gap-2 rounded-b-xl border-t px-4 py-2">
      <div className="min-w-0 flex-1">{left}</div>
      {children}
    </div>
  );
}

function Problem({ children }: { children: ReactNode }) {
  return (
    <div role="alert" className="text-error flex min-w-0 items-start gap-1.5 text-xs">
      <CircleAlert className="mt-px size-3.5 shrink-0" />
      <span className="line-clamp-2">{children}</span>
    </div>
  );
}

const RESTORE_TOOLS: RestoreTool[] = ["psql", "pg_restore", "mysql"];

/** The window's form: connections | databases | options (backup or restore). */
function BackupForm({ request }: { request: BackupRequest }) {
  const mode = request.mode;
  const qc = useQueryClient();
  const run = useBackupStore((s) => s.run);
  const profilesQuery = useProfiles();
  const profiles = profilesQuery.data ?? NO_PROFILES;
  const dumpable = useMemo(() => profiles.filter((p) => isDumpDriver(p.driver)), [profiles]);
  const [profileId, setProfileId] = useState<string | null>(request.profileId);
  const profile = profiles.find((p) => p.id === profileId) ?? dumpable[0] ?? null;
  const driver: DumpDriver | null = profile && isDumpDriver(profile.driver) ? profile.driver : null;
  const target = useBackupTarget(driver ? profile : null);
  const binaries = useDumpBinaries(driver);
  const [dbChoice, setDbChoice] = useState<{ profileId: string; name: string } | null>(null);

  const [backupOpts, setBackupOpts] = useState<Record<DumpDriver, BackupOptions>>(() => ({
    postgres: loadBackupOptions("postgres"),
    mysql: loadBackupOptions("mysql"),
  }));
  const [restoreOpts, setRestoreOpts] = useState<Record<RestoreTool, RestoreOptions>>(() => {
    const out = {} as Record<RestoreTool, RestoreOptions>;
    for (const t of RESTORE_TOOLS) out[t] = loadRestoreOptions(t);
    return out;
  });
  const [file, setFile] = useState<RestoreFileInfo | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useClock();

  const databases = target.data?.databases ?? [];
  const database = (() => {
    if (!profile || !target.data) return null;
    if (dbChoice?.profileId === profile.id && databases.includes(dbChoice.name))
      return dbChoice.name;
    if (
      request.profileId === profile.id &&
      request.database &&
      databases.includes(request.database)
    )
      return request.database;
    const current = target.data.currentDatabase;
    if (current && databases.includes(current)) return current;
    return databases[0] ?? null;
  })();

  if (profile && profile.driver === "sqlite") {
    return (
      <SqlitePanel
        profile={profile}
        mode={mode}
        onChooseOther={dumpable[0] ? () => setProfileId(dumpable[0]!.id) : null}
      />
    );
  }

  const tool = driver
    ? mode === "backup"
      ? backupTool(driver)
      : restoreTool(driver, file?.kind ?? null)
    : null;
  const backup = driver ? backupOpts[driver] : null;
  const restore = tool && mode === "restore" ? restoreOpts[tool as RestoreTool] : null;
  const flags = (mode === "backup" ? backup?.flags : restore?.flags) ?? [];
  const remembered = (mode === "backup" ? backup?.binaryPath : restore?.binaryPath) ?? null;
  const binary = tool
    ? pickBinary(binaries.data ?? [], tool, target.data?.server ?? null, remembered)
    : null;
  const gzipOk = driver ? gzipApplies(driver, flags) : false;
  const gzip = (backup?.gzip ?? false) && gzipOk;
  const ext = driver ? backupExtension(driver, flags, gzip) : ".sql";
  const preview =
    profile && backup
      ? backupFileName(
          backup.pattern,
          {
            connection: profile.name,
            database: database ?? "database",
            tag: profile.options.tag,
            date: now,
          },
          ext,
        )
      : "";

  const setFlags = (next: string[]) => {
    if (mode === "backup" && driver)
      setBackupOpts((o) => ({ ...o, [driver]: { ...o[driver], flags: next } }));
    else if (tool)
      setRestoreOpts((o) => ({
        ...o,
        [tool as RestoreTool]: { ...o[tool as RestoreTool], flags: next },
      }));
  };
  const setBinary = (path: string) => {
    if (mode === "backup" && driver)
      setBackupOpts((o) => ({ ...o, [driver]: { ...o[driver], binaryPath: path } }));
    else if (tool)
      setRestoreOpts((o) => ({
        ...o,
        [tool as RestoreTool]: { ...o[tool as RestoreTool], binaryPath: path },
      }));
  };
  const setBackup = (patch: Partial<BackupOptions>) => {
    if (driver) setBackupOpts((o) => ({ ...o, [driver]: { ...o[driver], ...patch } }));
  };

  const running = run !== null;
  const problem: string | null = (() => {
    if (!driver || !profile) return null;
    if (mode === "backup") return backupProblem(driver, flags);
    return fileError ?? restoreFileProblem(driver, file);
  })();
  const canStart =
    !running &&
    !busy &&
    profile !== null &&
    driver !== null &&
    database !== null &&
    binary !== null &&
    binary.tool === tool &&
    problem === null &&
    (mode === "backup" || file !== null);

  const chooseFile = async () => {
    let path: string | null;
    try {
      path = await pickRestoreFile(file ? dirname(file.path) : loadLastDir());
    } catch (e) {
      toast.error("Could not open the file dialog", { description: toIpcError(e).message });
      return;
    }
    if (!path) return;
    try {
      setFile(await ipc.backup.inspectFile(path));
      setFileError(null);
    } catch (e) {
      setFile(null);
      setFileError(toIpcError(e).message);
    }
  };

  const start = async () => {
    if (!canStart || !profile || !driver || !database || !binary) return;
    if (mode === "backup" && backup) {
      setBusy(true);
      let dir: string | null;
      try {
        dir = await pickBackupFolder(loadLastDir());
      } catch (e) {
        toast.error("Could not open the folder dialog", { description: toIpcError(e).message });
        return;
      } finally {
        setBusy(false);
      }
      if (!dir) return;
      saveLastDir(dir);
      saveBackupOptions(driver, { ...backup, binaryPath: binary.path });
      const name = backupFileName(
        backup.pattern,
        { connection: profile.name, database, tag: profile.options.tag, date: new Date() },
        ext,
      );
      void startJob({
        mode,
        profile,
        database,
        binaryPath: binary.path,
        flags,
        gzip,
        path: `${dir.replace(/\/+$/, "")}/${name}`,
        queryClient: qc,
      });
    } else if (mode === "restore" && file && tool) {
      saveRestoreOptions(tool as RestoreTool, { flags, binaryPath: binary.path });
      const dir = dirname(file.path);
      if (dir) saveLastDir(dir);
      void startJob({
        mode,
        profile,
        database,
        binaryPath: binary.path,
        flags,
        gzip: false,
        path: file.path,
        queryClient: qc,
      });
    }
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    void start();
  };

  if (run) {
    return (
      <div className="flex h-[480px] flex-col">
        <RunPanel run={run} />
        <Footer>
          {run.status === "running" ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void cancelJob()}
              disabled={run.cancelling}
            >
              Cancel {run.mode === "backup" ? "Backup" : "Restore"}
            </Button>
          ) : (
            <>
              <Button type="button" variant="ghost" size="sm" onClick={dismissRun}>
                <Undo2 /> Back
              </Button>
              <Button type="button" size="sm" onClick={closeBackup}>
                Done
              </Button>
            </>
          )}
        </Footer>
      </div>
    );
  }

  const targetError = target.error ? toIpcError(target.error).message : null;
  const startLabel = mode === "backup" ? "Start Backup…" : "Start Restore";
  return (
    <form
      onSubmit={onSubmit}
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          void start();
        }
      }}
      className="flex flex-col"
    >
      <div className="grid h-[440px] min-h-0 grid-cols-[200px_210px_minmax(0,1fr)]">
        <ConnectionList
          profiles={dumpable}
          selectedId={profile?.id ?? null}
          onSelect={(id) => {
            setProfileId(id);
            setFile(null);
            setFileError(null);
          }}
        />
        <DatabaseList
          databases={databases}
          selected={database}
          current={target.data?.currentDatabase ?? null}
          loading={profile !== null && target.isPending}
          error={targetError}
          onSelect={(name) => profile && setDbChoice({ profileId: profile.id, name })}
          onRetry={() => void target.refetch()}
        />
        <div className="flex min-h-0 flex-col">
          <div className="border-border flex h-8 shrink-0 items-center border-b px-3">
            <span className="text-muted-foreground truncate text-[11px] font-semibold tracking-wide uppercase">
              Options
            </span>
            {target.data?.server && (
              <span className="text-muted-foreground ml-auto truncate text-[11px]">
                Server {target.data.server.text}
              </span>
            )}
          </div>
          {!profile ? (
            <p className="text-muted-foreground px-3 py-3 text-xs">
              Backup and restore run pg_dump / mysqldump. Add a PostgreSQL or MySQL connection
              first; SQLite databases are backed up by copying their file.
            </p>
          ) : (
            <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-3 py-3">
              {mode === "restore" && (
                <Field
                  label="File"
                  hint={
                    file ? (
                      <span>
                        {fileKindLabel(file)} · {formatBytes(file.size)}
                      </span>
                    ) : (
                      "A .sql or .sql.gz dump, or a pg_dump archive (.dump, .backup, .tar)."
                    )
                  }
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      size="xs"
                      onClick={() => void chooseFile()}
                    >
                      <FileUp /> Choose File…
                    </Button>
                    {file && (
                      <span
                        className="min-w-0 truncate font-mono text-xs"
                        title={file.path}
                        data-testid="restore-file-name"
                      >
                        {file.name}
                      </span>
                    )}
                  </div>
                </Field>
              )}
              {tool && (
                <VersionPicker
                  tool={tool}
                  binaries={binaries.data ?? []}
                  selected={binary}
                  server={target.data?.server ?? null}
                  loading={binaries.isFetching}
                  onSelect={setBinary}
                  onRescan={() => void binaries.refetch()}
                />
              )}
              {tool && (
                <Field label="Options">
                  <OptionChips tool={tool} flags={flags} onChange={setFlags} />
                </Field>
              )}
              {mode === "backup" && backup && (
                <>
                  <label
                    className={cn(
                      "flex cursor-default items-center gap-2 text-xs",
                      !gzipOk && "text-muted-foreground opacity-60",
                    )}
                    title={
                      gzipOk
                        ? undefined
                        : "Custom and tar archives are compressed by pg_dump itself"
                    }
                  >
                    <Checkbox
                      checked={gzip}
                      disabled={!gzipOk}
                      onCheckedChange={(v) => setBackup({ gzip: v === true })}
                    />
                    Compress file using Gzip
                  </label>
                  <PatternField
                    pattern={backup.pattern}
                    preview={preview}
                    onChange={(pattern) => setBackup({ pattern })}
                  />
                </>
              )}
            </div>
          )}
        </div>
      </div>
      <Footer
        left={
          problem ? (
            <Problem>{problem}</Problem>
          ) : mode === "restore" && file && database ? (
            <span className="text-muted-foreground block truncate text-xs">
              Restores {basename(file.path)} into <span className="font-mono">{database}</span>
            </span>
          ) : null
        }
      >
        <Button type="button" variant="ghost" size="sm" onClick={closeBackup}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={!canStart}>
          {busy ? <Loader2 className="animate-spin" /> : null}
          {startLabel}
          <Kbd
            combo="Mod+Enter"
            className="border-primary-foreground/30 text-primary-foreground/80 bg-transparent"
          />
        </Button>
      </Footer>
    </form>
  );
}

/** The Backup / Restore window; opened with `openBackup` (store.ts), rendered by `BackupHost`. */
export function BackupDialog() {
  const open = useBackupStore((s) => s.request !== null);
  const shown = useBackupStore((s) => s.shown);
  const session = useBackupStore((s) => s.session);
  const mode = shown?.mode ?? "backup";
  return (
    <Dialog open={open} onOpenChange={(next) => !next && closeBackup()}>
      <DialogContent
        className="gap-0 overflow-hidden p-0 sm:max-w-[880px]"
        showCloseButton
        onCloseAutoFocus={releaseBackupWindow}
      >
        <div className="border-border flex items-center gap-2 border-b px-4 py-2.5 pr-12">
          {mode === "backup" ? (
            <DatabaseBackup className="text-brand size-4" />
          ) : (
            <FileUp className="text-brand size-4" />
          )}
          <DialogTitle className="text-sm">
            {mode === "backup" ? "Backup database" : "Restore database"}
          </DialogTitle>
          <DialogDescription className="sr-only">
            {mode === "backup"
              ? "Dump a database with pg_dump or mysqldump into a file."
              : "Load a dump file into a database with psql, pg_restore or mysql."}
          </DialogDescription>
        </div>
        {shown && <BackupForm key={session} request={shown} />}
      </DialogContent>
    </Dialog>
  );
}
