import { isTauri } from "@/lib/ipc/transport";
import { isWindows } from "@/lib/platform";
import {
  loadSavedShape,
  planReshape,
  type Reshape,
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
  /** Its monitor's work area, without the taskbar (logical pixels; null: not known). */
  workArea(): Promise<WindowFrame | null>;
  isFullscreen(): Promise<boolean>;
  isMaximized(): Promise<boolean>;
  isVisible(): Promise<boolean>;
  unmaximize(): Promise<void>;
  maximize(): Promise<void>;
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
 * macOS: a zoomed window on screen stays a workspace window (the list is drawn inside it).
 * Windows: like TablePlus for Windows, a maximized workspace window still becomes the small
 * Welcome window, and is maximized again when it next becomes a workspace window.
 */
const KEEP_ZOOMED: boolean = !isWindows;

/**
 * Gives the window the shape `to` and resolves with the shape it has then. A full-screen window
 * (or, `keepZoomed`, a zoomed one on screen) is not made the Welcome window: it stays a workspace
 * window and the connection list is drawn inside it. While its size changes the window is hidden,
 * so neither layout is seen at the other's size; `onSized` runs once the size is right and before
 * it shows again (the renderer switches its layout there).
 */
export async function applyShape(
  to: WindowShape,
  driver: WindowDriver,
  onSized: (shape: WindowShape) => void = () => undefined,
  pause: (ms: number) => Promise<void> = sleep,
  { keepZoomed = KEEP_ZOOMED }: { keepZoomed?: boolean } = {},
): Promise<WindowShape> {
  const saved = loadSavedShape(driver.label);
  const visible = await driver.isVisible();
  /** Hidden before anything else: a maximized window is restored out of sight. */
  let hiddenFirst = false;
  let maximized = false;
  if (to === "welcome" && (saved?.shape ?? "workspace") === "workspace") {
    if (await driver.isFullscreen()) return "workspace";
    if (await driver.isMaximized()) {
      if (visible && keepZoomed) return "workspace";
      if (visible) {
        await driver.hide();
        hiddenFirst = true;
      }
      await driver.unmaximize();
      maximized = !keepZoomed;
    }
  }
  const current = await driver.frame().catch(() => null);
  const plan = planReshape(saved, to, fitsWorkspace(current) ? current : null, maximized);
  storeSavedShape(driver.label, plan.saved);
  // A workspace window found at less than its minimum (the saved geometry was the Welcome
  // window's, e.g. after a crash) takes the default size.
  const change: Reshape =
    to === "workspace" && plan.change.kind === "keep" && !fitsWorkspace(current)
      ? { kind: "toWorkspace", frame: plan.saved.frame }
      : plan.change;
  const hidden = hiddenFirst || (change.kind !== "keep" && visible);
  if (hidden && !hiddenFirst) await driver.hide();
  try {
    if (to === "welcome") {
      await driver.setMinSize(null);
      await driver.setSize(WELCOME_SIZE.width, WELCOME_SIZE.height);
      // A Welcome window built hidden is centred here too: it is placed before it has its size.
      if (change.kind === "toWelcome" || !visible) await driver.center();
      await driver.setResizable(false);
      await driver.setMaximizable(false);
    } else {
      // Maximized again when it was maximized as a workspace window, or (still hidden, at
      // startup) when it was when the app last closed or its window was built maximized.
      let maximize =
        change.kind === "toWorkspace"
          ? change.maximized === true
          : !visible && (plan.saved.maximized === true || (await driver.isMaximized()));
      await driver.setResizable(true);
      await driver.setMaximizable(true);
      if (change.kind === "toWorkspace") {
        // The frame it gets here is where it goes when it is restored.
        if (fitsWorkspace(change.frame)) {
          await driver.setSize(change.frame.width, change.frame.height);
          await driver.setPosition(change.frame.x, change.frame.y);
        } else {
          // The default size; a screen too small for it (1280×820 is taller than a 768 px
          // screen's work area) gets a window that fits, maximized (`fit_to_work_area`).
          const area = await driver.workArea().catch(() => null);
          const fits =
            !area || (WORKSPACE_SIZE.width <= area.width && WORKSPACE_SIZE.height <= area.height);
          await driver.setSize(
            fits ? WORKSPACE_SIZE.width : Math.min(WORKSPACE_SIZE.width, area.width),
            fits ? WORKSPACE_SIZE.height : Math.min(WORKSPACE_SIZE.height, area.height),
          );
          await driver.center();
          if (!fits) maximize = true;
        }
      }
      await driver.setMinSize({
        width: WORKSPACE_SIZE.minWidth,
        height: WORKSPACE_SIZE.minHeight,
      });
      // Last, right before it shows: a hidden window is maximized only once it shows, and a
      // resize before that (the minimum size above) would drop it.
      if (maximize) await driver.maximize();
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
    const { currentMonitor, getCurrentWindow, LogicalPosition, LogicalSize } =
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
      workArea: async () => {
        const monitor = await currentMonitor();
        if (!monitor) return null;
        const { position, size } = monitor.workArea;
        const scale = monitor.scaleFactor;
        return {
          x: position.x / scale,
          y: position.y / scale,
          width: size.width / scale,
          height: size.height / scale,
        };
      },
      isFullscreen: () => win.isFullscreen(),
      isMaximized: () => win.isMaximized(),
      isVisible: () => win.isVisible(),
      unmaximize: () => win.unmaximize(),
      maximize: () => win.maximize(),
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
