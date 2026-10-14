import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PluginCancelled } from "./execute";
import { createHostServices } from "./host";
import {
  Deadline,
  MAX_PENDING_CALLS,
  MAX_RUN_MS,
  MAX_USER_PROMPTS,
  runInWorker,
  setPluginWorkerFactory,
  type WorkerLike,
} from "./runner";
import type { PluginSnapshot } from "./sandbox/protocol";
import { compilePlugin, createRuntime } from "./sandbox/runtime";
import { answerPluginPrompt, resetPluginsStoreForTests, usePluginsStore } from "./store";
import type { HostServices, PluginInvocation } from "./types";

const { cancel } = vi.hoisted(() => ({ cancel: vi.fn(async () => {}) }));
vi.mock("@/lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ipc")>();
  return { ...actual, ipc: { ...actual.ipc, query: { ...actual.ipc.query, cancel } } };
});

/** A "worker" running the sandbox runtime in-process (jsdom has no Worker). */
function inProcessWorker(): WorkerLike & { terminated: boolean } {
  const worker: WorkerLike & { terminated: boolean } = {
    terminated: false,
    onmessage: null,
    onerror: null,
    postMessage: (m) => queueMicrotask(() => runtime.onMessage(m)),
    terminate: () => {
      worker.terminated = true;
    },
  };
  const runtime = createRuntime({
    post: (m) => queueMicrotask(() => !worker.terminated && worker.onmessage?.({ data: m })),
    compile: (code, handler) => compilePlugin(Function, code, handler),
    later: (fn) => void setTimeout(fn, 0),
    global: {},
  });
  return worker;
}

const SNAPSHOT: PluginSnapshot = {
  source: "global",
  driver: null,
  connection: null,
  item: null,
  clickedRow: null,
  clickedColumn: null,
  selectedRows: [],
  sql: null,
};

function services(over: Partial<HostServices> = {}): HostServices {
  return {
    execute: vi.fn(),
    alert: vi.fn(async () => {}),
    insertToEditor: vi.fn(async () => {}),
    replaceSelection: vi.fn(async () => {}),
    insertToClipboard: vi.fn(async () => {}),
    openURL: vi.fn(async () => {}),
    notify: vi.fn(),
    loadFile: vi.fn(async () => ({ name: "data.csv", text: "a,b" })),
    ...over,
  };
}

const run = (code: string, s: HostServices, timeoutMs?: number) =>
  runInWorker({
    plugin: { identifier: "com.example.t", name: "T" },
    code,
    handler: null,
    snapshot: SNAPSHOT,
    services: () => s,
    timeoutMs,
  });

beforeEach(() => resetPluginsStoreForTests());
afterEach(() => {
  setPluginWorkerFactory(null);
  vi.useRealTimers();
});

describe("runInWorker", () => {
  it("answers the script's calls and resolves when it is done", async () => {
    let worker!: ReturnType<typeof inProcessWorker>;
    setPluginWorkerFactory(() => (worker = inProcessWorker()));
    const s = services();
    await run(`SystemService.insertToClipboard("x"); await context.alert("a", "b");`, s);
    expect(s.insertToClipboard).toHaveBeenCalledWith("x");
    expect(s.alert).toHaveBeenCalledWith("a", "b");
    expect(worker.terminated).toBe(true);
  });

  it("rejects with the script's error", async () => {
    setPluginWorkerFactory(inProcessWorker);
    await expect(run(`throw new TypeError("nope")`, services())).rejects.toThrow("TypeError: nope");
  });

  it("ends quietly when the user declined", async () => {
    setPluginWorkerFactory(inProcessWorker);
    const s = services({
      execute: vi.fn(async () => {
        throw new PluginCancelled();
      }),
    });
    await expect(run(`await context.execute("DROP TABLE t")`, s)).rejects.toBeInstanceOf(
      PluginCancelled,
    );
  });

  it("stops a script after the time-out and cancels its queries", async () => {
    vi.useFakeTimers();
    let worker!: ReturnType<typeof inProcessWorker>;
    setPluginWorkerFactory(() => (worker = inProcessWorker()));
    const p = runInWorker({
      plugin: { identifier: "com.example.t", name: "T" },
      code: `await context.execute("SELECT pg_sleep(60)")`,
      handler: null,
      snapshot: SNAPSHOT,
      services: ({ onQuery }) =>
        services({
          execute: vi.fn(() => {
            onQuery("c1", "plugin-q1");
            return new Promise<never>(() => {});
          }),
        }),
      timeoutMs: 30_000,
    });
    const outcome = expect(p).rejects.toThrow("stopped after 30 s");
    await vi.advanceTimersByTimeAsync(30_000);
    await outcome;
    expect(worker.terminated).toBe(true);
    expect(cancel).toHaveBeenCalledWith("c1", "plugin-q1");
  });

  it("does not count the time a dialog is open", async () => {
    vi.useFakeTimers();
    setPluginWorkerFactory(inProcessWorker);
    let close!: () => void;
    const p = runInWorker({
      plugin: { identifier: "com.example.t", name: "T" },
      code: `await context.alert("Read me", "")`,
      handler: null,
      snapshot: SNAPSHOT,
      services: ({ userWait }) =>
        services({ alert: () => userWait(new Promise<void>((r) => (close = r))) }),
      timeoutMs: 1_000,
    });
    let settled = false;
    void p.finally(() => (settled = true));
    await vi.advanceTimersByTimeAsync(5_000);
    expect(settled).toBe(false);
    close();
    await vi.advanceTimersByTimeAsync(10);
    await expect(p).resolves.toBeUndefined();
  });
});

const INVOCATION: PluginInvocation = {
  source: "global",
  connectionId: null,
  driver: null,
  item: null,
  clickedRow: null,
  clickedColumn: null,
  selectedRows: [],
  editorTabId: null,
  sql: null,
};

/** A run whose services are the real ones (alerts reach the plugins store, as in the app). */
const runWithRealServices = (code: string, extra: { timeoutMs?: number; maxRunMs?: number } = {}) =>
  runInWorker({
    plugin: { identifier: "com.example.t", name: "T" },
    code,
    handler: null,
    snapshot: SNAPSHOT,
    services: (hooks) =>
      createHostServices({
        plugin: { identifier: "com.example.t", name: "T" },
        invocation: INVOCATION,
        ...hooks,
      }),
    ...extra,
  });

const queued = () => usePluginsStore.getState().prompts.length;

/** Answers every dialog the moment it shows; counts them. */
function answerEveryDialog() {
  let answered = 0;
  const off = usePluginsStore.subscribe((s) => {
    const first = s.prompts[0];
    if (!first) return;
    queueMicrotask(() => {
      if (!usePluginsStore.getState().prompts.some((p) => p.prompt.id === first.prompt.id)) return;
      answered++;
      answerPluginPrompt(first.prompt.id, true);
    });
  });
  return { off, answered: () => answered };
}

describe("the limits of a run", () => {
  it("stops a plugin that floods the app with requests, and removes its dialogs", async () => {
    let worker!: ReturnType<typeof inProcessWorker>;
    setPluginWorkerFactory(() => (worker = inProcessWorker()));
    // A few real dialogs, then more calls than may wait at once (queries that never answer).
    const p = runInWorker({
      plugin: { identifier: "com.example.t", name: "T" },
      code: `for (let i = 0; i < 5; i++) context.alert("x", "y"); for (let i = 0; i < 100; i++) context.execute("SELECT 1"); await new Promise(() => {});`,
      handler: null,
      snapshot: SNAPSHOT,
      services: (hooks) => ({
        ...createHostServices({
          plugin: { identifier: "com.example.t", name: "T" },
          invocation: INVOCATION,
          ...hooks,
        }),
        execute: vi.fn(() => new Promise<never>(() => {})),
      }),
    });
    await expect(p).rejects.toThrow("stopped: the plugin made too many requests at once");
    expect(worker.terminated).toBe(true);
    // The alerts already queued went with the run; none came after it.
    expect(queued()).toBe(0);
    await Promise.resolve();
    expect(queued()).toBe(0);
    expect(MAX_PENDING_CALLS).toBe(64);
  });

  it("cancels the queries of a run it stops", async () => {
    cancel.mockClear();
    setPluginWorkerFactory(inProcessWorker);
    const p = runInWorker({
      plugin: { identifier: "com.example.t", name: "T" },
      code: `context.execute("SELECT 1"); for (let i = 0; i < 100; i++) context.alert("x", ""); await new Promise(() => {});`,
      handler: null,
      snapshot: SNAPSHOT,
      services: ({ onQuery }) =>
        services({
          execute: vi.fn(() => {
            onQuery("c1", "plugin-q9");
            return new Promise<never>(() => {});
          }),
          // Never answers: the calls pile up.
          alert: vi.fn(() => new Promise<void>(() => {})),
        }),
    });
    await expect(p).rejects.toThrow("too many requests");
    expect(cancel).toHaveBeenCalledWith("c1", "plugin-q9");
  });

  it("stops a plugin that keeps opening dialogs one after the other", async () => {
    setPluginWorkerFactory(inProcessWorker);
    const dialogs = answerEveryDialog();
    const p = runWithRealServices(
      `for (let i = 0; i < 25; i++) await context.alert("n", String(i));`,
    );
    await expect(p).rejects.toThrow("stopped: the plugin opened too many dialogs");
    dialogs.off();
    expect(dialogs.answered()).toBe(MAX_USER_PROMPTS);
    expect(queued()).toBe(0);
  });

  it("stops a plugin that opens dialogs without waiting for the user to answer them", async () => {
    let worker!: ReturnType<typeof inProcessWorker>;
    setPluginWorkerFactory(() => (worker = inProcessWorker()));
    // Nothing answers: all of these would sit in the queue, under the limit on pending calls.
    let mostQueued = 0;
    const off = usePluginsStore.subscribe((s) => {
      mostQueued = Math.max(mostQueued, s.prompts.length);
    });
    const p = runWithRealServices(
      `for (let i = 0; i < 30; i++) context.alert("x", String(i)); await new Promise(() => {});`,
    );
    await expect(p).rejects.toThrow("stopped: the plugin opened too many dialogs");
    off();
    expect(worker.terminated).toBe(true);
    // The 21st dialog ended the run: never more than the limit was on the user's queue.
    expect(mostQueued).toBe(MAX_USER_PROMPTS);
    expect(queued()).toBe(0);
    await Promise.resolve();
    expect(queued()).toBe(0);
  });

  it("counts link confirmations with the alerts", async () => {
    setPluginWorkerFactory(inProcessWorker);
    let mostQueued = 0;
    const off = usePluginsStore.subscribe((s) => {
      mostQueued = Math.max(mostQueued, s.prompts.length);
    });
    // Three links (the most a plugin may open in a while) and alerts, none of them awaited.
    const p = runWithRealServices(
      `for (let i = 0; i < 3; i++) SystemService.openURL("https://example.com/" + i).catch(() => {});
       for (let i = 0; i < 30; i++) context.alert("x", String(i));
       await new Promise(() => {});`,
    );
    await expect(p).rejects.toThrow("stopped: the plugin opened too many dialogs");
    off();
    expect(mostQueued).toBe(MAX_USER_PROMPTS);
    expect(queued()).toBe(0);
  });

  it("lets a plugin that stays within its dialogs finish", async () => {
    setPluginWorkerFactory(inProcessWorker);
    const dialogs = answerEveryDialog();
    await runWithRealServices(`for (let i = 0; i < 20; i++) await context.alert("n", String(i));`);
    dialogs.off();
    expect(dialogs.answered()).toBe(20);
  });

  it("still pauses the time-out for the one dialog on screen", async () => {
    vi.useFakeTimers();
    setPluginWorkerFactory(inProcessWorker);
    const p = runWithRealServices(`await context.alert("Read me", "")`, { timeoutMs: 1_000 });
    let settled = false;
    void p.finally(() => (settled = true));
    await vi.advanceTimersByTimeAsync(5_000);
    expect(settled).toBe(false);
    expect(queued()).toBe(1);
    answerPluginPrompt(usePluginsStore.getState().prompts[0]!.prompt.id, true);
    await vi.advanceTimersByTimeAsync(10);
    await expect(p).resolves.toBeUndefined();
  });

  it("stops a run that holds its time-out off for longer than the ceiling", async () => {
    vi.useFakeTimers();
    let worker!: ReturnType<typeof inProcessWorker>;
    setPluginWorkerFactory(() => (worker = inProcessWorker()));
    const p = runWithRealServices(`await context.alert("never closed", "")`, {
      timeoutMs: 1_000,
      maxRunMs: 5_000,
    });
    const outcome = expect(p).rejects.toThrow("stopped: the plugin ran longer than 5 s in all");
    await vi.advanceTimersByTimeAsync(4_900);
    // The 1 s time-out is paused by the open dialog; the ceiling is not.
    expect(worker.terminated).toBe(false);
    expect(queued()).toBe(1);
    await vi.advanceTimersByTimeAsync(200);
    await outcome;
    expect(worker.terminated).toBe(true);
    expect(queued()).toBe(0);
  });

  it("has a ceiling of ten minutes unless told otherwise", async () => {
    expect(MAX_RUN_MS).toBe(10 * 60_000);
    vi.useFakeTimers();
    setPluginWorkerFactory(inProcessWorker);
    const p = runWithRealServices(`await context.alert("never closed", "")`);
    const outcome = expect(p).rejects.toThrow("longer than 10 min in all");
    await vi.advanceTimersByTimeAsync(MAX_RUN_MS - 1);
    expect(queued()).toBe(1);
    await vi.advanceTimersByTimeAsync(1);
    await outcome;
    expect(queued()).toBe(0);
  });

  it("removes the dialogs a failed run left open", async () => {
    setPluginWorkerFactory(inProcessWorker);
    const p = runWithRealServices(`context.alert("open", ""); throw new Error("boom");`);
    await expect(p).rejects.toThrow("boom");
    expect(queued()).toBe(0);
  });

  it("tells its services that the run was stopped", async () => {
    setPluginWorkerFactory(inProcessWorker);
    let stopped!: () => boolean;
    const execute = vi.fn();
    const p = runInWorker({
      plugin: { identifier: "com.example.t", name: "T" },
      code: `await new Promise(() => {});`,
      handler: null,
      snapshot: SNAPSHOT,
      maxRunMs: 50,
      services: (hooks) => {
        stopped = hooks.stopped;
        return services({ execute });
      },
    });
    await expect(p).rejects.toThrow("longer than");
    expect(stopped()).toBe(true);
    expect(execute).not.toHaveBeenCalled();
  });
});

describe("Deadline", () => {
  it("pauses and resumes", () => {
    vi.useFakeTimers();
    const expire = vi.fn();
    const d = new Deadline(100, expire);
    d.start();
    vi.advanceTimersByTime(60);
    d.pause();
    vi.advanceTimersByTime(1_000);
    expect(expire).not.toHaveBeenCalled();
    d.resume();
    vi.advanceTimersByTime(39);
    expect(expire).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expire).toHaveBeenCalledTimes(1);
  });
});
