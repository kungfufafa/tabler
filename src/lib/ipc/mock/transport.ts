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

export async function mockInvoke<T>(cmd: string, args: InvokeArgs): Promise<T> {
  const handler = handlers.get(cmd);
  if (!handler) {
    throw { code: "NOT_FOUND", message: `mock: no handler registered for command "${cmd}"` };
  }
  if (mockLatency > 0) await new Promise((r) => setTimeout(r, mockLatency));
  return (await handler(args)) as T;
}

// --- built-in handlers -------------------------------------------------------
registerMock("app_info", () => ({ version: "0.1.0", platform: "mock", debug: true }));
