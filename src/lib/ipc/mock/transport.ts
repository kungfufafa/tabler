import type { InvokeArgs } from "../transport";

type Handler = (args: InvokeArgs) => unknown | Promise<unknown>;

const handlers = new Map<string, Handler>();

/** Register (or replace) the mock implementation of a Tauri command. */
export function registerMock(cmd: string, handler: Handler): void {
  handlers.set(cmd, handler);
}

/** Simulated IPC latency in ms; 0 in tests. */
export let mockLatency = import.meta.env.MODE === "test" ? 0 : 40;
export function setMockLatency(ms: number): void {
  mockLatency = ms;
}

export function mockError(code: string, message: string): never {
  throw { code, message };
}

export async function mockInvoke<T>(cmd: string, args: InvokeArgs): Promise<T> {
  const handler = handlers.get(cmd);
  if (!handler) mockError("NOT_FOUND", `mock: no handler registered for command "${cmd}"`);
  if (mockLatency > 0) await new Promise((r) => setTimeout(r, mockLatency));
  return (await handler(args)) as T;
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
