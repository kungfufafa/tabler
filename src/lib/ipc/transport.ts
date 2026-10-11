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

export async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  try {
    if (isMock) {
      const { mockInvoke } = await import("./mock");
      return await mockInvoke<T>(cmd, args ?? {});
    }
    return await tauriInvoke<T>(cmd, args);
  } catch (err) {
    throw toIpcError(err);
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
