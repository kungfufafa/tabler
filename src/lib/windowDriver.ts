import { isTauri } from "@/lib/ipc/transport";
import {
  loadSavedShape,
  planReshape,
  savedWelcomeLabels,
  storeSavedShape,
  WELCOME_SIZE,
  WORKSPACE_SIZE,
  type WindowFrame,
  type WindowShape,
} from "@/stores/windowShape";

/**
 * The native side of a window's shape (`stores/windowShape`): what resizing a window takes, as an
 * interface, so the steps (`applyShape`) are tested without a window.
 */
export interface WindowDriver {
  readonly label: string;
  /** Where the window is now (logical pixels). */
  frame(): Promise<WindowFrame>;
  isFullscreen(): Promise<boolean>;
  isMaximized(): Promise<boolean>;
  isVisible(): Promise<boolean>;
  unmaximize(): Promise<void>;
  hide(): Promise<void>;
  /** Shows the window and gives it the focus. */
  reveal(): Promise<void>;
  setSize(width: number, height: number): Promise<void>;
  setPosition(x: number, y: number): Promise<void>;
  center(): Promise<void>;
  setResizable(resizable: boolean): Promise<void>;
  setMaximizable(maximizable: boolean): Promise<void>;
  setMinSize(size: { width: number; height: number } | null): Promise<void>;
}

/** How long a hidden window is given to lay its new content out before it shows again. */
const SETTLE_MS = 80;
const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Whether `frame` can be a workspace window's (not the Welcome window's, not a broken one). */
const fitsWorkspace = (frame: WindowFrame | null): frame is WindowFrame =>
  frame !== null &&
  frame.width >= WORKSPACE_SIZE.minWidth &&
  frame.height >= WORKSPACE_SIZE.minHeight;

/**
 * Gives the window the shape `to` and resolves with the shape it has then. A full-screen window
 * (or a zoomed one on screen) is not made the Welcome window: it stays a workspace window and the
 * connection list is drawn inside it. While its size changes the window is hidden, so neither
 * layout is seen at the other's size; `onSized` runs once the size is right and before it shows
 * again (the renderer switches its layout there).
 */
export async function applyShape(
  to: WindowShape,
  driver: WindowDriver,
  onSized: (shape: WindowShape) => void = () => undefined,
  pause: (ms: number) => Promise<void> = sleep,
): Promise<WindowShape> {
  const saved = loadSavedShape(driver.label);
  const visible = await driver.isVisible();
  if (to === "welcome" && (saved?.shape ?? "workspace") === "workspace") {
    if (await driver.isFullscreen()) return "workspace";
    if (await driver.isMaximized()) {
      if (visible) return "workspace";
      await driver.unmaximize();
    }
  }
  const current = await driver.frame().catch(() => null);
  const plan = planReshape(saved, to, fitsWorkspace(current) ? current : null);
  storeSavedShape(driver.label, plan.saved);
  // A workspace window found at less than its minimum (the saved geometry was the Welcome
  // window's, e.g. after a crash) takes the default size.
  const change =
    to === "workspace" && plan.change.kind === "keep" && !fitsWorkspace(current)
      ? ({ kind: "toWorkspace", frame: plan.saved.frame } as const)
      : plan.change;
  const hidden = change.kind !== "keep" && visible;
  if (hidden) await driver.hide();
  try {
    if (to === "welcome") {
      await driver.setMinSize(null);
      await driver.setSize(WELCOME_SIZE.width, WELCOME_SIZE.height);
      // A Welcome window built hidden is centred here too: it is placed before it has its size.
      if (change.kind === "toWelcome" || !visible) await driver.center();
      await driver.setResizable(false);
      await driver.setMaximizable(false);
    } else {
      await driver.setResizable(true);
      await driver.setMaximizable(true);
      if (change.kind === "toWorkspace") {
        if (fitsWorkspace(change.frame)) {
          await driver.setSize(change.frame.width, change.frame.height);
          await driver.setPosition(change.frame.x, change.frame.y);
        } else {
          await driver.setSize(WORKSPACE_SIZE.width, WORKSPACE_SIZE.height);
          await driver.center();
        }
      }
      await driver.setMinSize({
        width: WORKSPACE_SIZE.minWidth,
        height: WORKSPACE_SIZE.minHeight,
      });
    }
    onSized(to);
    // A window built hidden (the Welcome window) is given the same time before it first shows.
    if (hidden || !visible) await pause(SETTLE_MS);
  } finally {
    // Whatever happened, the window is never left hidden.
    if (hidden || !(await driver.isVisible().catch(() => false))) await driver.reveal();
  }
  return to;
}

let driver: Promise<WindowDriver | null> | null = null;

/** This window's driver (null outside Tauri: the browser mock has no window to resize). */
export function windowDriver(): Promise<WindowDriver | null> {
  driver ??= (async (): Promise<WindowDriver | null> => {
    if (!isTauri()) return null;
    const { getCurrentWindow, LogicalPosition, LogicalSize } =
      await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    return {
      label: win.label,
      frame: async () => {
        const [pos, size, scale] = await Promise.all([
          win.outerPosition(),
          win.innerSize(),
          win.scaleFactor(),
        ]);
        return {
          x: pos.x / scale,
          y: pos.y / scale,
          width: size.width / scale,
          height: size.height / scale,
        };
      },
      isFullscreen: () => win.isFullscreen(),
      isMaximized: () => win.isMaximized(),
      isVisible: () => win.isVisible(),
      unmaximize: () => win.unmaximize(),
      hide: () => win.hide(),
      reveal: async () => {
        await win.show();
        await win.setFocus();
      },
      setSize: (width, height) => win.setSize(new LogicalSize(width, height)),
      setPosition: (x, y) => win.setPosition(new LogicalPosition(x, y)),
      center: () => win.center(),
      setResizable: (resizable) => win.setResizable(resizable),
      setMaximizable: (maximizable) => win.setMaximizable(maximizable),
      setMinSize: (size) => win.setMinSize(size ? new LogicalSize(size.width, size.height) : null),
    };
  })().catch((e: unknown) => {
    console.warn("[window] no driver", e);
    return null;
  });
  return driver;
}

/**
 * Brings an open Welcome window other than this one to the front (File ▸ New Window shows the
 * connection list: there is one Welcome window, not one per ⌘N). False when there is none.
 */
export async function focusWelcomeWindow(): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    const { getCurrentWindow, Window } = await import("@tauri-apps/api/window");
    const own = getCurrentWindow().label;
    for (const label of savedWelcomeLabels()) {
      if (label === own) continue;
      // Remembered as the Welcome window, but closed since: not listed.
      const other = await Window.getByLabel(label);
      if (!other) continue;
      await other.show();
      await other.setFocus();
      return true;
    }
  } catch (e) {
    console.warn("[window] could not show the Welcome window", e);
  }
  return false;
}
