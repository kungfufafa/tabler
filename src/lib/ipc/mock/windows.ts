import type { WindowHandoff } from "../bindings/WindowHandoff";
import { registerMock } from "./transport";

/**
 * Mock windows (`pnpm dev:mock`): a new window is a new browser tab of the same app with
 * `?window=w-N`; its handoff waits in localStorage until that tab takes it, so "Open in New
 * Window" and "Move Tab to New Window" can be tried in a browser. Every tab has its own in-memory
 * backend, as every real window has its own renderer stores (and connects again).
 */

const HANDOFF_PREFIX = "tabler.mock.handoff.";
const COUNTER_KEY = "tabler.mock.window-counter";

/** What each mock window last reported as its connections. */
export const mockWindowConnections = new Map<string, string[]>();
/** Windows opened in this tab (tests read it; `window.open` is skipped there). */
export const mockOpenedWindows: { label: string; handoff: WindowHandoff | null }[] = [];

/** This tab's window label: `?window=` of its URL, else the main window. */
export function mockWindowLabel(): string {
  try {
    return new URLSearchParams(globalThis.location?.search ?? "").get("window") || "main";
  } catch {
    return "main";
  }
}

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function nextLabel(): string {
  const store = storage();
  const n = Number(store?.getItem(COUNTER_KEY) ?? "1") + 1;
  store?.setItem(COUNTER_KEY, String(n));
  return `w-${n}`;
}

registerMock("window_open", ({ handoff }) => {
  const label = nextLabel();
  const parked = (handoff as WindowHandoff | null | undefined) ?? null;
  if (parked) storage()?.setItem(HANDOFF_PREFIX + label, JSON.stringify(parked));
  mockOpenedWindows.push({ label, handoff: parked });
  if (import.meta.env.MODE !== "test" && typeof globalThis.open === "function") {
    const url = new URL(globalThis.location.href);
    url.searchParams.set("window", label);
    globalThis.open(url.toString(), "_blank");
  }
  return label;
});

registerMock("window_take_handoff", () => {
  const key = HANDOFF_PREFIX + mockWindowLabel();
  const raw = storage()?.getItem(key) ?? null;
  if (raw === null) return null;
  storage()?.removeItem(key);
  try {
    return JSON.parse(raw) as WindowHandoff;
  } catch {
    return null;
  }
});

registerMock("window_set_connections", ({ connectionIds }) => {
  mockWindowConnections.set(mockWindowLabel(), [...((connectionIds as string[]) ?? [])]);
  return null;
});

registerMock("window_label", () => mockWindowLabel());

/** Every mock window is its own app: the main one keeps the session (tests may change it). */
export const mockSessionOwner = { label: "main" };

registerMock("window_is_session_owner", () => mockWindowLabel() === mockSessionOwner.label);

registerMock("cancel_quit", () => null);
