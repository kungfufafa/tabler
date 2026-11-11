import { create } from "zustand";
import { isMac } from "@/lib/keys/combo";

/**
 * The shape a window takes (macOS, like TablePlus): the small fixed **Welcome** window while it
 * has no connection open, the resizable **workspace** window once it does. One window changes
 * shape rather than two windows trading places, so everything that opens a connection in a
 * window (deep links, Open Recent, the session restore, the palette) keeps working as it is.
 *
 * This file is the plain part: what shape a window should have, and what taking it involves. The
 * native part (resizing the window) is `app/WindowShapeHost`.
 */
export type WindowShape = "welcome" | "workspace";

/** Windows and Linux keep the connection list inside the one window for now. */
export const shapedWindows: boolean = isMac;

/** The Welcome window (logical pixels): fixed, like TablePlus's. */
export const WELCOME_SIZE = { width: 760, height: 480 } as const;
/** The workspace window's size and minimum: tauri.conf.json's main window. */
export const WORKSPACE_SIZE = { width: 1280, height: 820, minWidth: 960, minHeight: 600 } as const;

/** A window's place on screen, in logical pixels. */
export interface WindowFrame {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * What is remembered of a window (by label, across restarts): the shape it had last, since the
 * geometry the window-state plugin restores at startup is that shape's, and where its workspace
 * shape was.
 */
export interface SavedShape {
  shape: WindowShape;
  frame: WindowFrame | null;
}

/** What a window has to do to take a shape. */
export type Reshape =
  /** It has that shape already: only its constraints are set again. */
  | { kind: "keep" }
  /** Workspace → Welcome. */
  | { kind: "toWelcome" }
  /** Welcome → workspace: back to where its workspace shape was (null: the default size). */
  | { kind: "toWorkspace"; frame: WindowFrame | null };

/**
 * The window takes the shape `to`; `current` is where it is now. Leaving the workspace shape
 * remembers that frame, coming back to it returns it. A window that never took a shape is a
 * workspace window (the configured size).
 */
export function planReshape(
  saved: SavedShape | null,
  to: WindowShape,
  current: WindowFrame | null,
): { saved: SavedShape; change: Reshape } {
  const from = saved?.shape ?? "workspace";
  const kept = saved?.frame ?? null;
  if (from === "workspace" && to === "welcome")
    return { saved: { shape: to, frame: current ?? kept }, change: { kind: "toWelcome" } };
  if (from === "welcome" && to === "workspace")
    return { saved: { shape: to, frame: kept }, change: { kind: "toWorkspace", frame: kept } };
  return { saved: { shape: to, frame: kept }, change: { kind: "keep" } };
}

/**
 * The shape the window should have (null: not known yet, leave it). A window with a connection,
 * or one about to open some (the startup restore, a handed-over connection), or with the
 * standalone SQL Viewer up is a workspace window; one with nothing is the Welcome window, once
 * the startup has decided there is nothing to open.
 */
export function desiredShape(s: {
  connections: number;
  opening: boolean;
  decided: boolean;
  /** Something that needs the room of a workspace window without a connection. */
  roomy?: boolean;
}): WindowShape | null {
  if (s.connections > 0 || s.opening || s.roomy) return "workspace";
  return s.decided ? "welcome" : null;
}

const KEY_PREFIX = "tabler.windowShape.";

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

const isNumber = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

function parseFrame(raw: unknown): WindowFrame | null {
  if (typeof raw !== "object" || raw === null) return null;
  const f = raw as Record<string, unknown>;
  if (!isNumber(f.x) || !isNumber(f.y) || !isNumber(f.width) || !isNumber(f.height)) return null;
  if (f.width < 200 || f.height < 200) return null;
  return { x: f.x, y: f.y, width: f.width, height: f.height };
}

/** What is remembered of the window `label` (null: nothing, or not readable). */
export function loadSavedShape(label: string): SavedShape | null {
  try {
    const raw: unknown = JSON.parse(storage()?.getItem(KEY_PREFIX + label) ?? "null");
    if (typeof raw !== "object" || raw === null) return null;
    const { shape, frame } = raw as Record<string, unknown>;
    if (shape !== "welcome" && shape !== "workspace") return null;
    return { shape, frame: parseFrame(frame) };
  } catch {
    return null;
  }
}

export function storeSavedShape(label: string, saved: SavedShape): void {
  try {
    storage()?.setItem(KEY_PREFIX + label, JSON.stringify(saved));
  } catch {
    // Not remembered: the window takes the default size next time.
  }
}

/** Labels of the windows that were Welcome-shaped last (open or not). */
export function savedWelcomeLabels(): string[] {
  const store = storage();
  if (!store) return [];
  const labels: string[] = [];
  for (let i = 0; i < store.length; i++) {
    const key = store.key(i);
    if (!key?.startsWith(KEY_PREFIX)) continue;
    const label = key.slice(KEY_PREFIX.length);
    if (loadSavedShape(label)?.shape === "welcome") labels.push(label);
  }
  return labels.sort();
}

interface WindowShapeState {
  /** The shape this window has now (null: not shaped; the connection list is drawn in-window). */
  shape: WindowShape | null;
  /** The startup knows whether it opens something (`startupOpening` / `startupDone`). */
  decided: boolean;
  /** The startup is opening workspaces: the window is a workspace window meanwhile. */
  opening: boolean;
  setShape: (shape: WindowShape | null) => void;
}

export const useWindowShape = create<WindowShapeState>()((set) => ({
  shape: null,
  decided: false,
  opening: false,
  setShape: (shape) => set({ shape }),
}));

/** The startup restore (or a handed-over connection) is about to connect. */
export function startupOpening(): void {
  useWindowShape.setState({ decided: true, opening: true });
}

/** The startup is over: whatever it opened is open, or there was nothing to open. */
export function startupDone(): void {
  useWindowShape.setState({ decided: true, opening: false });
}

/** Whether this window is the Welcome window (its own toolbar, the list edge to edge). */
export const useIsWelcomeWindow = (): boolean => useWindowShape((s) => s.shape === "welcome");
