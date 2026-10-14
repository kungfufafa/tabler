import { ipc } from "@/lib/ipc";
import { handleBridgeCall } from "./bridge";
import { PluginCancelled } from "./execute";
import { isBridgeMethod, type HostMessage, type PluginSnapshot } from "./sandbox/protocol";
import { dismissPluginPrompts, newPluginRunId } from "./store";
import type { HostServices, RunHooks } from "./types";

/** How long one command may run (its dialogs excluded). */
export const PLUGIN_TIMEOUT_MS = 30_000;
/**
 * The most a command may run in all, its dialogs included: the time-out above stops counting while
 * a dialog is open, so without this a plugin that keeps dialogs open would never be stopped.
 */
export const MAX_RUN_MS = 10 * 60_000;
/** Calls to the app a command may have waiting at once. */
export const MAX_PENDING_CALLS = 64;
/** Dialogs (alerts, link confirmations, the file dialog) one command run may open, answered or not. */
export const MAX_USER_PROMPTS = 20;

function duration(ms: number): string {
  return ms >= 60_000 ? `${Math.round(ms / 60_000)} min` : `${Math.round(ms / 1000)} s`;
}

/** The parts of `Worker` the runner uses (tests pass fakes). */
export interface WorkerLike {
  postMessage(message: unknown): void;
  terminate(): void;
  onmessage: ((e: { data: unknown }) => void) | null;
  onerror: ((e: { message?: string; preventDefault?: () => void }) => void) | null;
}

const defaultFactory = (): WorkerLike =>
  new Worker(new URL("./sandbox/plugin.worker.ts", import.meta.url), {
    type: "module",
    name: "tabler-plugin",
  }) as unknown as WorkerLike;

let createWorker: () => WorkerLike = defaultFactory;

/** Test-only: where workers come from (null: the real sandbox worker). */
export function setPluginWorkerFactory(factory: (() => WorkerLike) | null): void {
  createWorker = factory ?? defaultFactory;
}

/** A time-out that stops counting while the user answers a dialog. */
export class Deadline {
  private remaining: number;
  private startedAt = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private paused = 0;
  private done = false;

  constructor(
    ms: number,
    private readonly onExpire: () => void,
  ) {
    this.remaining = ms;
  }

  start(): void {
    if (this.done || this.paused > 0) return;
    this.startedAt = Date.now();
    this.timer = setTimeout(() => {
      this.done = true;
      this.onExpire();
    }, this.remaining);
  }

  pause(): void {
    if (this.done || this.paused++ > 0) return;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.remaining = Math.max(0, this.remaining - (Date.now() - this.startedAt));
  }

  resume(): void {
    if (this.done || this.paused === 0) return;
    if (--this.paused === 0) this.start();
  }

  clear(): void {
    this.done = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }
}

export interface WorkerRun {
  plugin: { identifier: string; name: string };
  code: string;
  handler: string | null;
  snapshot: PluginSnapshot;
  /**
   * The services for this run. `userWait` pauses the time-out, `onQuery` records queries, `dialog`
   * shows a counted dialog that goes when the run ends.
   */
  services: (hooks: RunHooks) => HostServices;
  timeoutMs?: number;
  /** The ceiling of the whole run, dialogs included (default `MAX_RUN_MS`). */
  maxRunMs?: number;
}

/**
 * Runs a sandboxed command in a fresh worker and answers its bridge calls. Resolves when the
 * script finished; rejects with its error, after `timeoutMs` (the worker is killed and its queries
 * cancelled), or with `PluginCancelled` when the script stopped because the user declined (Safe
 * Mode, a link). A command is also stopped when it has too many calls waiting at once, opens too
 * many dialogs, or runs longer than `maxRunMs` in all; whichever way a run ends, the dialogs it
 * still has open go with it.
 */
export function runInWorker(run: WorkerRun): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const worker = createWorker();
    const runId = newPluginRunId();
    const timeoutMs = run.timeoutMs ?? PLUGIN_TIMEOUT_MS;
    const maxRunMs = run.maxRunMs ?? MAX_RUN_MS;
    const queries: Array<{ connectionId: string; queryId: string }> = [];
    let settled = false;
    let declined: string | null = null;
    let inFlight = 0;
    let dialogsOpened = 0;
    const end = (error: Error | null) => {
      if (settled) return;
      settled = true;
      deadline.clear();
      clearTimeout(ceiling);
      worker.onmessage = null;
      worker.onerror = null;
      worker.terminate();
      // Dialogs the run left open (a flood of alerts) are answered as dismissed and removed.
      dismissPluginPrompts(runId);
      if (error) reject(error);
      else resolve();
    };
    /** Ends the run with `message`: its queries are cancelled, its worker killed. */
    const stop = (message: string): Error => {
      for (const q of queries) void ipc.query.cancel(q.connectionId, q.queryId).catch(() => {});
      const error = new Error(message);
      end(error);
      return error;
    };
    const deadline = new Deadline(
      timeoutMs,
      () => void stop(`stopped after ${duration(timeoutMs)}`),
    );
    // Counts paused time too: a run cannot hold its own time-out off for ever.
    const ceiling = setTimeout(
      () => void stop(`stopped: the plugin ran longer than ${duration(maxRunMs)} in all`),
      maxRunMs,
    );
    const userWait = async <T>(p: Promise<T>): Promise<T> => {
      deadline.pause();
      try {
        return await p;
      } finally {
        deadline.resume();
      }
    };
    let services: HostServices;
    try {
      services = run.services({
        userWait,
        onQuery: (connectionId, queryId) => queries.push({ connectionId, queryId }),
        runId,
        dialog: async <T>(show: () => Promise<T>): Promise<T> => {
          if (settled) throw new Error("The command already finished");
          // Counted when it is opened, not when it is answered: a script that fires its alerts
          // without awaiting them has all of them queued before the user answers one.
          if (dialogsOpened >= MAX_USER_PROMPTS)
            throw stop("stopped: the plugin opened too many dialogs");
          dialogsOpened++;
          return await userWait(show());
        },
        stopped: () => settled,
      });
    } catch (e) {
      end(e instanceof Error ? e : new Error(String(e)));
      return;
    }
    const post = (message: HostMessage) => {
      if (!settled) worker.postMessage(message);
    };
    worker.onmessage = (e) => {
      const msg = e.data as { type?: unknown; id?: unknown; method?: unknown; args?: unknown };
      if (settled || !msg || typeof msg !== "object") return;
      if (msg.type === "done") return end(null);
      if (msg.type === "error") {
        const text = String((msg as { message?: unknown }).message ?? "failed");
        return end(
          declined !== null && text === declined ? new PluginCancelled(text) : new Error(text),
        );
      }
      if (msg.type !== "call" || typeof msg.id !== "number") return;
      const id = msg.id;
      if (!isBridgeMethod(msg.method)) {
        post({ type: "reply", id, ok: false, error: `Unknown call ${String(msg.method)}` });
        return;
      }
      // Calls the app has not answered yet (an alert waits for its OK): bounded, so a loop that
      // never awaits cannot queue work without end.
      if (inFlight >= MAX_PENDING_CALLS) {
        stop("stopped: the plugin made too many requests at once");
        return;
      }
      inFlight++;
      const args = Array.isArray(msg.args) ? msg.args : [];
      handleBridgeCall(services, msg.method, args).then(
        (value) => {
          inFlight--;
          post({ type: "reply", id, ok: true, value });
        },
        (err: unknown) => {
          inFlight--;
          const text = err instanceof Error ? err.message : String(err);
          if (err instanceof PluginCancelled) declined = text;
          post({ type: "reply", id, ok: false, error: text });
        },
      );
    };
    worker.onerror = (e) => {
      e.preventDefault?.();
      end(new Error(`the plugin sandbox failed: ${e.message ?? "unknown error"}`));
    };
    deadline.start();
    post({
      type: "run",
      plugin: run.plugin,
      code: run.code,
      handler: run.handler,
      snapshot: run.snapshot,
    });
  });
}
