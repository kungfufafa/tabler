import { invoke as tauriInvoke } from "@tauri-apps/api/core";

export type InvokeArgs = Record<string, unknown>;

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** True when running in a plain browser (vite --mode mock, vitest) — commands are served by the in-memory mock. */
export const isMock: boolean = __TABLER_MOCK__ || !isTauri();

/** Error shape produced by the Rust `AppError` serializer. */
export interface ErrorPayload {
  code: string;
  message: string;
  detail?: string | null;
  hint?: string | null;
  sqlstate?: string | null;
  vendorCode?: number | null;
  position?: { offset: number; line: number; column: number } | null;
  statementIndex?: number | null;
  data?: unknown;
}

export class IpcError extends Error {
  readonly payload: ErrorPayload;
  constructor(payload: ErrorPayload) {
    super(payload.message);
    this.name = "IpcError";
    this.payload = payload;
  }
  get code(): string {
    return this.payload.code;
  }
}

export function toIpcError(err: unknown): IpcError {
  if (err instanceof IpcError) return err;
  if (err && typeof err === "object" && "code" in err && "message" in err) {
    return new IpcError(err as ErrorPayload);
  }
  const message = err instanceof Error ? err.message : String(err);
  return new IpcError({ code: "INTERNAL", message });
}

// ---- dropped-connection detection ---------------------------------------------------------

/** Commands whose failure says nothing about an open session's health (or that probe it). */
const NOT_SESSION_COMMANDS = new Set([
  "ping",
  "connect",
  "disconnect",
  "test_connection",
  "accept_host_key",
  "cancel_query",
  "cancel_export",
]);

/**
 * Error codes a dead session produces (src-tauri/src/error.rs): `NOT_CONNECTED` (PostgreSQL
 * client closed, deadpool closed, MySQL `ConnectionClosed`, a stopped SQLite actor),
 * `CONNECT_FAILED` (socket errors from tokio-postgres and mysql_async), `TIMEOUT` (a pool or
 * socket that stopped answering) and `SSH_TUNNEL` (the tunnel under the connection went away).
 * `IO` is left out: it only comes from files (export, .sql files), never from a socket.
 */
const BROKEN_SESSION_CODES = new Set(["NOT_CONNECTED", "CONNECT_FAILED", "TIMEOUT", "SSH_TUNNEL"]);

/** MySQL client errors "server has gone away" (2006) and "lost connection during query" (2013). */
const MYSQL_GONE = new Set([2006, 2013]);

/**
 * True when command `cmd`, run against `args.connectionId`, failed in a way that suggests the
 * connection itself is broken (rather than the statement): see `BROKEN_SESSION_CODES`, plus
 * server errors of SQLSTATE class 08 (connection exception), 57P01–57P03 (PostgreSQL admin /
 * crash shutdown, cannot connect now) and MySQL 2006 / 2013.
 */
export function isConnectionFailure(
  cmd: string,
  args: InvokeArgs | undefined,
  error: IpcError,
): boolean {
  if (typeof args?.connectionId !== "string" || NOT_SESSION_COMMANDS.has(cmd)) return false;
  const { code, sqlstate, vendorCode } = error.payload;
  if (BROKEN_SESSION_CODES.has(code)) return true;
  if (code !== "DB_ERROR") return false;
  if (sqlstate && (sqlstate.startsWith("08") || /^57P0[1-3]$/.test(sqlstate))) return true;
  return typeof vendorCode === "number" && MYSQL_GONE.has(vendorCode);
}

type ConnectionFailureListener = (connectionId: string, error: IpcError) => void;
const failureListeners = new Set<ConnectionFailureListener>();

/**
 * Calls `listener` whenever a command against an open connection fails like a dropped
 * connection would (`isConnectionFailure`), whether it rejected or returned the error in-band
 * (`reportConnectionFailure`). Returns the unsubscribe function.
 */
export function onConnectionFailure(listener: ConnectionFailureListener): () => void {
  failureListeners.add(listener);
  return () => void failureListeners.delete(listener);
}

/**
 * Tells the `onConnectionFailure` listeners about `error` when it looks like a dropped connection
 * (`isConnectionFailure`); returns whether it did. `invoke` calls this for every rejected command.
 * Commands that report failures in-band call it too, through their facade: `run_sql` (a
 * `StatementResult.error` / `failed` event), `apply_changes` (`ApplyResult.error`) and `apply_ddl`
 * (`DdlResult.error`) resolve successfully even when the server is gone.
 */
export function reportConnectionFailure(
  cmd: string,
  args: InvokeArgs | undefined,
  error: IpcError | ErrorPayload | null | undefined,
): boolean {
  if (!error || failureListeners.size === 0) return false;
  const ipcError = error instanceof IpcError ? error : new IpcError(error);
  if (!isConnectionFailure(cmd, args, ipcError)) return false;
  const connectionId = args!.connectionId as string;
  for (const listener of [...failureListeners]) {
    try {
      listener(connectionId, ipcError);
    } catch {
      // A broken listener must not replace the command's own error.
    }
  }
  return true;
}

export async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  try {
    if (isMock) {
      const { mockInvoke } = await import("./mock");
      return await mockInvoke<T>(cmd, args ?? {});
    }
    return await tauriInvoke<T>(cmd, args);
  } catch (err) {
    const error = toIpcError(err);
    reportConnectionFailure(cmd, args, error);
    throw error;
  }
}

/**
 * Streaming callback channel for commands that take a `tauri::ipc::Channel<T>` argument.
 * In Tauri this is a real `Channel`; in the browser mock it is a plain object whose `send`
 * the mock handler calls.
 */
export interface StreamChannel<T> {
  send?: (message: T) => void;
}

export async function makeChannel<T>(
  onMessage: (message: T) => void,
): Promise<StreamChannel<T> | object> {
  if (isMock) return { send: onMessage } satisfies StreamChannel<T>;
  const { Channel } = await import("@tauri-apps/api/core");
  const ch = new Channel<T>();
  ch.onmessage = onMessage;
  return ch;
}
