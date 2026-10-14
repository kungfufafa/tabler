/**
 * The plugin side of the sandbox: what runs inside the dedicated Web Worker of a command.
 *
 * `lockdown` removes every way out of the worker before plugin code runs: the network (fetch,
 * XMLHttpRequest, WebSocket, EventSource…, also through `importScripts` and nested workers),
 * storage shared with the app (IndexedDB, Cache Storage), cross-context messaging
 * (BroadcastChannel) and run-time code generation (eval, the Function constructors, string
 * timers), which would otherwise reach `import()`. `scanSource` refuses every script in which the
 * word `import` appears. What is left is plain computation plus the message bridge to the app
 * (`context.*`, `SystemService.*`), whose every call the app checks again.
 *
 * Nothing here touches the DOM: the module also loads in tests, against fake scopes.
 */
import type {
  BridgeMethod,
  HostMessage,
  PlainExecResult,
  PlainLoadedFile,
  PlainValue,
  PluginSnapshot,
  SnapshotRow,
  WorkerMessage,
} from "./protocol";

type Bag = Record<string, unknown>;

/** Globals a plugin must not reach. */
export const BLOCKED_GLOBALS = [
  // Network.
  "fetch",
  "fetchLater",
  "XMLHttpRequest",
  "XMLHttpRequestUpload",
  "XMLHttpRequestEventTarget",
  "WebSocket",
  "WebSocketStream",
  "WebTransport",
  "EventSource",
  "RTCPeerConnection",
  "webkitRTCPeerConnection",
  "importScripts",
  // Workers of its own would start with a fresh, unlocked global.
  "Worker",
  "SharedWorker",
  // Storage and messaging shared with the app's origin.
  "caches",
  "indexedDB",
  "BroadcastChannel",
  "Notification",
  "webkitRequestFileSystem",
  "requestFileSystem",
  // Code generation (see `lockCodegen` for the Function constructors).
  "eval",
  // Ending the worker by itself would leave the app waiting until the time-out.
  "close",
] as const;

/** Thrown by every blocked global. */
export class SandboxError extends Error {
  constructor(what: string) {
    super(`${what} is not available to plugins`);
    this.name = "SandboxError";
  }
}

/** Throws when called, and when used as a constructor (`new XMLHttpRequest()`). */
function stub(name: string): () => never {
  return function blocked(): never {
    throw new SandboxError(name);
  };
}

function define(target: object, name: string, value: unknown): boolean {
  try {
    Object.defineProperty(target, name, {
      value,
      writable: false,
      configurable: false,
      enumerable: false,
    });
    return true;
  } catch {
    return false;
  }
}

/**
 * Removes `name` from `scope` and its prototype chain, then pins `value` on `scope` (not writable,
 * not configurable: the plugin cannot put anything back). Returns whether `value` is what the
 * plugin now sees.
 */
export function replaceGlobal(scope: object, name: string, value: unknown): boolean {
  let obj: object | null = scope;
  while (obj && obj !== Object.prototype) {
    if (Object.getOwnPropertyDescriptor(obj, name)) {
      try {
        delete (obj as Bag)[name];
      } catch {
        // Non-configurable: `value` is defined over it below when possible.
      }
    }
    obj = Object.getPrototypeOf(obj) as object | null;
  }
  define(scope, name, value);
  return (scope as Bag)[name] === value;
}

/** Replaces `name` with a function that throws `SandboxError`. */
export function blockGlobal(scope: object, name: string): boolean {
  return replaceGlobal(scope, name, stub(name));
}

/** The prototypes whose `constructor` compiles code from strings. */
export function codegenPrototypes(): object[] {
  const protos: object[] = [Function.prototype];
  for (const make of [
    () => Object.getPrototypeOf(async function () {}),
    () => Object.getPrototypeOf(function* () {}),
    () => Object.getPrototypeOf(async function* () {}),
  ]) {
    try {
      protos.push(make() as object);
    } catch {
      // Not supported: nothing to lock.
    }
  }
  return protos;
}

/**
 * Locks run-time code generation: `Function` (also as `fn.constructor`, including the async and
 * generator flavours) throws. `new Function("return this")` keeps working (bundlers use it to find
 * the global object) and returns the sandbox's global.
 *
 * The stand-in for `Function` carries the realm's real `Function.prototype`, so what libraries do
 * with the global (`Function.prototype.call` / `apply` / `bind` / `toString`, `fn instanceof
 * Function`) keeps working; only the `constructor` links point at the blocking stand-in.
 */
export function lockCodegen(scope: object, prototypes: readonly object[] = codegenPrototypes()) {
  const blocked = function Function(...args: unknown[]) {
    const body = args.length > 0 ? String(args[args.length - 1]).trim() : "";
    if (/^return\s+this;?$/.test(body)) return () => scope;
    throw new SandboxError("Creating functions from strings");
  };
  // The realm's own `Function.prototype`: the original global's, else the first locked prototype.
  const real = (scope as { Function?: { prototype?: unknown } }).Function?.prototype;
  const functionProto =
    (typeof real === "object" || typeof real === "function") && real !== null
      ? (real as object)
      : prototypes[0];
  if (functionProto)
    Object.defineProperty(blocked, "prototype", {
      value: functionProto,
      writable: false,
      enumerable: false,
      configurable: false,
    });
  for (const proto of prototypes) define(proto, "constructor", blocked);
  define(scope, "Function", blocked);
}

/** `setTimeout` / `setInterval` with a function only (a string would be compiled). */
function lockTimers(scope: Bag) {
  for (const name of ["setTimeout", "setInterval"] as const) {
    const original = scope[name];
    if (typeof original !== "function") continue;
    const safe = (handler: unknown, ...rest: unknown[]) => {
      if (typeof handler !== "function") throw new SandboxError(`${name} with a string`);
      return (original as (...a: unknown[]) => unknown).call(scope, handler, ...rest);
    };
    replaceGlobal(scope, name, safe);
  }
}

/** `navigator` without its service worker container and friends: a frozen copy of the basics. */
function lockNavigator(scope: Bag) {
  const nav = scope.navigator as Bag | undefined;
  if (!nav) return;
  replaceGlobal(
    scope,
    "navigator",
    Object.freeze({
      userAgent: nav.userAgent,
      language: nav.language,
      languages: nav.languages,
      platform: nav.platform,
      hardwareConcurrency: nav.hardwareConcurrency,
    }),
  );
}

export interface LockdownOptions {
  /** Prototypes whose `constructor` to lock (tests pass fakes: the real ones are shared). */
  prototypes?: readonly object[];
}

/** Locks the worker's global before any plugin code runs. Returns the globals still reachable. */
export function lockdown(scope: object, options: LockdownOptions = {}): string[] {
  const leaks: string[] = [];
  const bag = scope as Bag;
  for (const name of BLOCKED_GLOBALS) {
    if (!blockGlobal(scope, name)) leaks.push(name);
  }
  lockTimers(bag);
  lockNavigator(bag);
  lockCodegen(scope, options.prototypes);
  return leaks;
}

/** The word `import`, as its own word (`importer`, `reimport` and `Import` are other words). */
const IMPORT_RE = /\bimport\b/;

/**
 * Why a script cannot run in the sandbox, or null. Dynamic `import()` is syntax: it cannot be
 * stubbed like `fetch`, and it reaches the network (the worker script is outside the page's CSP).
 * So a script in which the word `import` appears anywhere is refused: in code, but also in
 * comments, strings and property names. Nothing is skipped on purpose. Telling code from comments
 * and strings takes a real tokenizer (regex literals, template strings, and in script code the
 * HTML-like comments `<!--` and `-->` all move the boundaries), and every place a text scan
 * wrongly skips is a place `import (...)` can hide. A false refusal costs a reworded comment.
 */
export function scanSource(code: string): string | null {
  if (IMPORT_RE.test(code))
    return "Plugins cannot load modules: the word import is not allowed anywhere in a plugin script (even in comments or strings)";
  return null;
}

const HANDLER_RE = /^[A-Za-z_$][\w$]{0,63}$/;

export type PluginEntryFn = (...args: unknown[]) => Promise<unknown>;

/**
 * Compiles a script into `(context, SystemService, global, exports, module, require) => Promise`:
 * the script body runs inside an async function (top-level `await` works); with `handler`, the
 * promise resolves to the function of that name the script declares.
 */
export function compilePlugin(
  FunctionCtor: FunctionConstructor,
  code: string,
  handler: string | null,
): PluginEntryFn {
  if (handler !== null && !HANDLER_RE.test(handler))
    throw new Error(`"${handler}" is not a function name`);
  const tail = handler
    ? `\n;return typeof ${handler} === "function" ? ${handler} : undefined;`
    : "";
  return new FunctionCtor(
    "context",
    "SystemService",
    "global",
    "exports",
    "module",
    "require",
    `return (async () => {\n${code}${tail}\n})();`,
  ) as PluginEntryFn;
}

export function errorText(e: unknown): string {
  if (e instanceof Error) {
    const name = e.name && e.name !== "Error" && e.name !== "SandboxError" ? `${e.name}: ` : "";
    return `${name}${e.message}`;
  }
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e) ?? String(e);
  } catch {
    return String(e);
  }
}

export interface PluginRow {
  readonly columns: readonly string[];
  readonly values: Readonly<Record<string, PlainValue>>;
  /** The value of `column` (TablePlus's `row.raw(name)`); undefined for an unknown column. */
  raw(column: string): PlainValue | undefined;
}

export function makeRow(row: SnapshotRow): PluginRow {
  const values: Record<string, PlainValue> = Object.create(null) as Record<string, PlainValue>;
  row.columns.forEach((c, i) => {
    values[c] = row.values[i] ?? null;
  });
  Object.freeze(values);
  return Object.freeze({
    columns: Object.freeze([...row.columns]),
    values,
    raw: (column: string) => (column in values ? values[column] : undefined),
  });
}

export interface PluginExecResult {
  readonly columns: readonly string[];
  readonly rows: readonly PluginRow[];
  readonly rowsAffected: number | null;
}

type Call = (method: BridgeMethod, args: unknown[]) => Promise<unknown>;

const str = (v: unknown): string => (v === undefined || v === null ? "" : String(v));
const nothing = (): void => undefined;

/** The `context` and `SystemService` objects a script receives. */
export function makeApi(snapshot: PluginSnapshot, call: Call) {
  const execute = (sql: unknown, callback?: unknown): Promise<PluginExecResult> => {
    const result = call("execute", [str(sql)]).then((v) => {
      const r = v as PlainExecResult;
      return Object.freeze({
        columns: Object.freeze([...r.columns]),
        rows: Object.freeze(r.rows.map((values) => makeRow({ columns: r.columns, values }))),
        rowsAffected: r.rowsAffected,
      });
    });
    if (typeof callback !== "function") return result;
    return result.then((r) => {
      (callback as (r: PluginExecResult) => unknown)(r);
      return r;
    });
  };
  const context = Object.freeze({
    currentItem: () => (snapshot.item ? { ...snapshot.item } : null),
    clickedRow: () => (snapshot.clickedRow ? makeRow(snapshot.clickedRow) : null),
    selectedRows: () => snapshot.selectedRows.map(makeRow),
    clickedColumn: () => (snapshot.clickedColumn ? { ...snapshot.clickedColumn } : null),
    currentSql: () => snapshot.sql?.text ?? "",
    selectedSql: () => snapshot.sql?.selected ?? "",
    currentStatement: () => snapshot.sql?.statement ?? "",
    driver: () => snapshot.driver,
    source: () => snapshot.source,
    connection: () => (snapshot.connection ? { ...snapshot.connection } : null),
    execute,
    alert: (title: unknown, message?: unknown) =>
      call("alert", [str(title), str(message)]).then(nothing),
    insertToEditor: (text: unknown) => call("insertToEditor", [str(text)]).then(nothing),
    replaceSelection: (text: unknown) => call("replaceSelection", [str(text)]).then(nothing),
    copyToClipboard: (text: unknown) => call("insertToClipboard", [str(text)]).then(nothing),
    /** Opens a file dialog: resolves to the text file the user picks (`{ name, text }`). */
    loadFile: () =>
      call("loadFile", []).then((v) => {
        const f = v as PlainLoadedFile;
        return Object.freeze({ name: f.name, text: f.text });
      }),
  });
  const SystemService = Object.freeze({
    insertToClipboard: (text: unknown) => call("insertToClipboard", [str(text)]).then(nothing),
    openURL: (url: unknown) => call("openURL", [str(url)]).then(nothing),
    notify: (title: unknown, message?: unknown) =>
      call("notify", [str(title), str(message)]).then(nothing),
  });
  return { context, SystemService };
}

export interface RuntimeEnv {
  post: (message: WorkerMessage) => void;
  compile: (code: string, handler: string | null) => PluginEntryFn;
  /** Runs `fn` after the pending microtasks (the worker's own `setTimeout(fn, 0)`). */
  later: (fn: () => void) => void;
  /** Where bundled scripts may have put the handler (`global.onRun = …` through `this`). */
  global: object;
}

/**
 * Runs one command: answers the app's `run` and `reply` messages. The command is done once its
 * code returned and no bridge call is pending (callback-style `context.execute(sql, cb)` keeps it
 * running until `cb` ran). The first error ends it.
 */
export function createRuntime(env: RuntimeEnv) {
  let nextId = 1;
  const calls = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  let pending = 0;
  let started = false;
  let returned = false;
  let finished = false;
  let checking = false;

  const finish = (message: WorkerMessage) => {
    if (finished) return;
    finished = true;
    env.post(message);
  };
  const fail = (e: unknown) => finish({ type: "error", message: errorText(e) });
  const check = () => {
    if (checking) return;
    checking = true;
    env.later(() => {
      checking = false;
      if (!finished && returned && pending === 0) finish({ type: "done" });
    });
  };
  const call: Call = (method, args) => {
    if (finished) return Promise.reject(new Error("The command already finished"));
    const id = nextId++;
    pending++;
    return new Promise((resolve, reject) => {
      calls.set(id, { resolve, reject });
      env.post({ type: "call", id, method, args });
    });
  };

  const run = (msg: Extract<HostMessage, { type: "run" }>) => {
    const problem = scanSource(msg.code);
    if (problem) return fail(new Error(problem));
    let entry: PluginEntryFn;
    try {
      entry = env.compile(msg.code, msg.handler);
    } catch (e) {
      return fail(e);
    }
    const { context, SystemService } = makeApi(msg.snapshot, call);
    const global: Bag = {};
    const module = { exports: {} as Bag };
    const require = (name: unknown) => {
      throw new SandboxError(`require(${JSON.stringify(name)})`);
    };
    Promise.resolve()
      .then(() => entry(context, SystemService, global, module.exports, module, require))
      .then(async (declared) => {
        if (msg.handler === null) return;
        const h = msg.handler;
        const fn =
          declared ??
          global[h] ??
          module.exports[h] ??
          (module.exports as unknown as Bag)[h] ??
          (env.global as Bag)[h];
        if (typeof fn !== "function") throw new Error(`The script does not define ${h}()`);
        await (fn as (c: unknown) => unknown)(context);
      })
      .then(() => {
        returned = true;
        check();
      }, fail);
  };

  return {
    onMessage(data: unknown) {
      const msg = data as HostMessage;
      if (!msg || typeof msg !== "object") return;
      if (msg.type === "run") {
        if (started) return;
        started = true;
        run(msg);
      } else if (msg.type === "reply") {
        const pendingCall = calls.get(msg.id);
        if (!pendingCall) return;
        calls.delete(msg.id);
        pending--;
        if (msg.ok) pendingCall.resolve(msg.value);
        else pendingCall.reject(new Error(msg.error));
        check();
      }
    },
    /** An error the script did not catch (a rejected promise nobody awaited, a timer's throw). */
    fail,
    get finished() {
      return finished;
    },
  };
}
