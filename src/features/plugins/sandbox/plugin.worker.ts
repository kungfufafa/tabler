/**
 * The dedicated worker one plugin command runs in. The global is locked before the first
 * message arrives; the app then sends the script (`run`) and answers its bridge calls.
 */
import { compilePlugin, createRuntime, lockdown } from "./runtime";

/** The parts of `DedicatedWorkerGlobalScope` used here (the DOM lib types `self` as a window). */
interface WorkerScope {
  postMessage(message: unknown): void;
  setTimeout(fn: () => void, ms: number): unknown;
  addEventListener(type: "message", fn: (e: MessageEvent) => void): void;
  addEventListener(type: "unhandledrejection", fn: (e: PromiseRejectionEvent) => void): void;
  addEventListener(type: "error", fn: (e: ErrorEvent) => void): void;
}

const scope = self as unknown as WorkerScope;
// Kept before the lockdown takes them away from the plugin.
const FunctionCtor = Function;
const post = scope.postMessage.bind(scope);
const timeout = scope.setTimeout.bind(scope);

const leaks = lockdown(scope);
if (leaks.length > 0) console.warn("[plugins] could not block", leaks.join(", "));

const runtime = createRuntime({
  post: (message) => post(message),
  compile: (code, handler) => compilePlugin(FunctionCtor, code, handler),
  later: (fn) => void timeout(fn, 0),
  global: scope,
});

scope.addEventListener("message", (e: MessageEvent) => runtime.onMessage(e.data));
scope.addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
  e.preventDefault();
  runtime.fail(e.reason);
});
scope.addEventListener("error", (e: ErrorEvent) => {
  e.preventDefault();
  runtime.fail(e.error ?? e.message);
});
