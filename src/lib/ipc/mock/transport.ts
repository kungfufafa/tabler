import type { InvokeArgs } from "../transport";

type Handler = (args: InvokeArgs) => unknown | Promise<unknown>;

const handlers = new Map<string, Handler>();

/** Register (or replace) the mock implementation of a Tauri command. */
export function registerMock(cmd: string, handler: Handler): void {
  handlers.set(cmd, handler);
}

/** The mock registered for `cmd` (a test that replaces it puts it back with `registerMock`). */
export function mockHandler(cmd: string): Handler | undefined {
  return handlers.get(cmd);
}

/** Simulated IPC latency in ms; 0 in tests. */
export let mockLatency = import.meta.env.MODE === "test" ? 0 : 40;
export function setMockLatency(ms: number): void {
  mockLatency = ms;
}

export function mockError(code: string, message: string): never {
  throw { code, message };
}

type CommandObserver = (cmd: string, args: InvokeArgs, result: unknown) => void;
const observers = new Set<CommandObserver>();

/** Calls `observer` after every mock command that succeeded (see `mock/trace.ts`). */
export function observeMock(observer: CommandObserver): () => void {
  observers.add(observer);
  return () => void observers.delete(observer);
}

export async function mockInvoke<T>(cmd: string, args: InvokeArgs): Promise<T> {
  const handler = handlers.get(cmd);
  if (!handler) mockError("NOT_FOUND", `mock: no handler registered for command "${cmd}"`);
  // Draining the SQL trace is local bookkeeping, not a server round trip.
  if (mockLatency > 0 && cmd !== "drain_sql_trace")
    await new Promise((r) => setTimeout(r, mockLatency));
  const result = await handler(args);
  for (const observer of observers) {
    try {
      observer(cmd, args, result);
    } catch {
      // An observer never changes a command's outcome.
    }
  }
  return result as T;
}

// --- events ----------------------------------------------------------------
type Listener = (payload: unknown) => void;
const listeners = new Map<string, Set<Listener>>();

export function mockListen(event: string, listener: Listener): () => void {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(listener);
  return () => set?.delete(listener);
}

export function mockEmit(event: string, payload: unknown): void {
  listeners.get(event)?.forEach((l) => l(payload));
}
