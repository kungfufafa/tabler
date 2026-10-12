import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { create } from "zustand";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import {
  ipc,
  toIpcError,
  type BackupEvent,
  type BackupJobRequest,
  type BackupJobResult,
  type BackupLogLine,
  type ConnectionProfile,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { guardJob } from "./guard";
import { basename, formatBytes, formatElapsed } from "./options";
import { revealInFinder } from "./pickers";

export type BackupMode = "backup" | "restore";

export interface BackupRequest {
  mode: BackupMode;
  /** Preselected saved connection (profile id); null = pick it in the dialog. */
  profileId: string | null;
  /** Preselected database; null = the connection's default. */
  database: string | null;
}

/** Lines kept in the window's log (the backend caps what it sends, too). */
export const MAX_LOG_LINES = 2_000;

export type BackupRunStatus = "running" | "failed" | "done";

/** A backup / restore that runs (or just ended) in the window. */
export interface BackupRun {
  jobId: string;
  mode: BackupMode;
  profileId: string;
  profileName: string;
  database: string;
  /** The file written (backup, once started) or read (restore). */
  path: string | null;
  /** The binary's command line (never holds the password). */
  command: string | null;
  startedAt: number;
  /** Ended at (for the elapsed time of a finished run). */
  endedAt: number | null;
  bytes: number;
  /** Restore: size of the file being read; null when the binary reads it itself. */
  total: number | null;
  log: BackupLogLine[];
  cancelling: boolean;
  status: BackupRunStatus;
  error: string | null;
  /** What to try next (the backend's hint: exit code, a known remedy). */
  errorHint: string | null;
  /** Error lines in the output of a run that still succeeded (psql without ON_ERROR_STOP). */
  errors: number;
}

interface BackupState {
  /** The open window's request; null while it is closed. */
  request: BackupRequest | null;
  /** What the window shows: kept after it closes so its content stays during the exit animation. */
  shown: BackupRequest | null;
  /** Bumped on every open so the window starts from the remembered options again. */
  session: number;
  run: BackupRun | null;
}

export const useBackupStore = create<BackupState>()(() => ({
  request: null,
  shown: null,
  session: 0,
  run: null,
}));

/**
 * Opens the Backup / Restore window (welcome screen buttons, File ▸ Backup… / Restore…). While a
 * job runs the window reopens on it instead.
 */
export function openBackup(request: BackupRequest): void {
  useBackupStore.setState((s) => {
    const run = s.run;
    if (run?.status === "running") {
      const current = { mode: run.mode, profileId: run.profileId, database: run.database };
      return { request: current, shown: s.request ? s.shown : current };
    }
    return { request, shown: request, session: s.session + 1, run: null };
  });
}

/** Closes the window; a running job keeps going and reports through a toast. */
export function closeBackup(): void {
  useBackupStore.setState((s) => ({
    request: null,
    run: s.run?.status === "running" ? s.run : null,
  }));
}

/** Lets go of the closed window's content once it has animated out. */
export function releaseBackupWindow(): void {
  useBackupStore.setState((s) => (s.request ? s : { shown: null }));
}

/** Back from a finished (failed) run to the options. */
export function dismissRun(): void {
  useBackupStore.setState((s) => (s.run?.status === "running" ? s : { run: null }));
}

function updateRun(jobId: string, patch: (run: BackupRun) => Partial<BackupRun>): void {
  useBackupStore.setState((s) =>
    s.run?.jobId === jobId ? { run: { ...s.run, ...patch(s.run) } } : s,
  );
}

function onEvent(jobId: string, e: BackupEvent): void {
  if (e.event === "started") {
    updateRun(jobId, () => ({ command: e.data.command, path: e.data.path }));
  } else if (e.event === "log") {
    updateRun(jobId, (r) => {
      const log = r.log.concat(e.data.lines);
      return { log: log.length > MAX_LOG_LINES ? log.slice(log.length - MAX_LOG_LINES) : log };
    });
  } else {
    updateRun(jobId, () => ({ bytes: e.data.bytes, total: e.data.total }));
  }
}

export interface StartJobArgs {
  mode: BackupMode;
  profile: ConnectionProfile;
  database: string;
  binaryPath: string;
  flags: string[];
  gzip: boolean;
  /** Backup: the file to write. Restore: the file to read. */
  path: string;
  /** The app's query client: open connections on a restored database reload their schema. */
  queryClient: QueryClient;
  now?: () => number;
}

const isOpen = () => useBackupStore.getState().request !== null;

function finishedToast(mode: BackupMode, result: BackupJobResult, database: string): void {
  const name = basename(result.path);
  const details = `${name} · ${formatBytes(result.bytes)} · ${formatElapsed(result.durationMs)}`;
  const reveal = {
    label: "Show in Finder",
    onClick: () =>
      void revealInFinder(result.path).catch((e: unknown) =>
        toast.error("Could not show the file", { description: toIpcError(e).message }),
      ),
  };
  if (mode === "backup") {
    toast.success(`Backed up ${database}`, { description: details, action: reveal });
  } else if (result.errors > 0) {
    toast.warning(
      `Restored ${database} with ${result.errors} error${result.errors === 1 ? "" : "s"}`,
      {
        description: `${name}: see the log in the Restore window.`,
      },
    );
  } else {
    toast.success(`Restored ${database}`, { description: details });
  }
}

/**
 * After a restore into `database`: every open connection of the profile that shows it reloads
 * its schema, the backend's cache first, then the sidebar, completion and open tables (tables
 * with staged edits keep their page and are reported). A MySQL connection lists every database
 * of the server, so all of the profile's connections reload.
 */
export async function refreshRestoredDatabase(
  qc: QueryClient,
  profile: Pick<ConnectionProfile, "id" | "driver">,
  database: string,
): Promise<void> {
  const ids = Object.values(useConnectionsStore.getState().byId)
    .filter(
      (c) =>
        c.info.profileId === profile.id &&
        (profile.driver === "mysql" || c.info.currentDatabase === database),
    )
    .map((c) => c.info.connectionId);
  const held = await Promise.all(
    ids.map(async (id) => {
      try {
        await ipc.schema.invalidate(id);
      } catch (e) {
        console.warn("[backup] could not reset the schema cache", e);
      }
      return invalidateConnectionSchema(qc, id);
    }),
  );
  reportHeldTables(held.flat());
}

/**
 * Starts a backup or restore: the connection's safe-mode check first (a restore writes, a backup
 * reads), then the binary, its log and byte counts tracked in the store. Success closes the
 * window with a toast ("Show in Finder"); a restore whose output reported errors stays open on its
 * log. A failure stays on the log while the window is open (a toast otherwise). Resolves with the
 * result, or null when it did not complete.
 */
export async function startJob(args: StartJobArgs): Promise<BackupJobResult | null> {
  const { mode, profile, database, path } = args;
  const now = args.now ?? Date.now;
  if (useBackupStore.getState().run?.status === "running") return null;
  const label =
    mode === "backup" ? `Backup ${database}` : `Restore ${basename(path)} into ${database}`;
  let allowed: boolean;
  try {
    allowed = await guardJob(profile, { kind: "operation", label, write: mode === "restore" });
  } catch (e) {
    toast.error(`Could not connect to ${profile.name}`, { description: toIpcError(e).message });
    return null;
  }
  if (!allowed) return null;
  const jobId = crypto.randomUUID();
  useBackupStore.setState({
    run: {
      jobId,
      mode,
      profileId: profile.id,
      profileName: profile.name,
      database,
      path: mode === "restore" ? path : null,
      command: null,
      startedAt: now(),
      endedAt: null,
      bytes: 0,
      total: null,
      log: [],
      cancelling: false,
      status: "running",
      error: null,
      errorHint: null,
      errors: 0,
    },
  });
  const req: BackupJobRequest = {
    jobId,
    kind: mode,
    profileId: profile.id,
    database,
    binaryPath: args.binaryPath,
    flags: args.flags,
    gzip: mode === "backup" && args.gzip,
    path,
  };
  try {
    const result = await ipc.backup.run(req, (e) => onEvent(jobId, e));
    const keepOpen = mode === "restore" && result.errors > 0 && isOpen();
    updateRun(jobId, () => ({
      status: "done",
      endedAt: now(),
      path: result.path,
      errors: result.errors,
      bytes: result.bytes,
    }));
    if (!keepOpen) useBackupStore.setState({ request: null, run: null });
    finishedToast(mode, result, database);
    if (mode === "restore") await refreshRestoredDatabase(args.queryClient, profile, database);
    return result;
  } catch (err) {
    const e = toIpcError(err);
    const what = mode === "backup" ? "Backup" : "Restore";
    if (e.code === "CANCELLED") {
      useBackupStore.setState((s) => (s.run?.jobId === jobId ? { run: null } : s));
      toast.message(`${what} cancelled`, {
        description: mode === "backup" ? "The partial file was removed." : undefined,
      });
    } else if (isOpen()) {
      updateRun(jobId, () => ({
        status: "failed",
        endedAt: now(),
        error: e.message,
        errorHint: e.payload.hint ?? null,
      }));
    } else {
      useBackupStore.setState((s) => (s.run?.jobId === jobId ? { run: null } : s));
      toast.error(`${what} failed`, { description: e.message });
    }
    return null;
  }
}

/** Kills the running job's binary (a partial backup file is removed). */
export async function cancelJob(): Promise<void> {
  const run = useBackupStore.getState().run;
  if (!run || run.status !== "running" || run.cancelling) return;
  updateRun(run.jobId, () => ({ cancelling: true }));
  try {
    await ipc.backup.cancel(run.jobId);
  } catch (err) {
    updateRun(run.jobId, () => ({ cancelling: false }));
    toast.error("Could not cancel", { description: toIpcError(err).message });
  }
}

/** Test helper. */
export function resetBackupStore(): void {
  useBackupStore.setState({ request: null, shown: null, session: 0, run: null });
}

/** "3.4 MB written" (backup) / "3.4 MB of 12 MB read" (restore). */
export function bytesLabel(run: Pick<BackupRun, "mode" | "bytes" | "total">): string {
  if (run.mode === "backup") return `${formatBytes(run.bytes)} written`;
  if (run.total !== null) return `${formatBytes(run.bytes)} of ${formatBytes(run.total)} read`;
  return run.bytes > 0 ? `${formatBytes(run.bytes)} read` : "reading the archive";
}
