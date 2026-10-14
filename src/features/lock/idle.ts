/**
 * "Auto lock if away for": calls `onIdle` once nothing happened in the window (keys, mouse,
 * wheel, touch) for `idleMs`. A timer that fired late (the Mac slept, a hidden window throttled
 * it) is caught up by the next activity or visibility change: activity after the deadline locks
 * instead of counting as presence.
 */

/** Events that count as the user being there. */
export const ACTIVITY_EVENTS = [
  "keydown",
  "pointerdown",
  "pointermove",
  "mousedown",
  "mousemove",
  "wheel",
  "touchstart",
] as const;

type Target = Pick<Window, "addEventListener" | "removeEventListener">;
type VisibilityDoc = Pick<Document, "visibilityState" | "addEventListener" | "removeEventListener">;

export interface IdleTimerOptions {
  idleMs: number;
  onIdle: () => void;
  now?: () => number;
  target?: Target;
  doc?: VisibilityDoc;
}

export interface IdleTimer {
  /** Records activity (the listeners call it). */
  activity: () => void;
  stop: () => void;
}

export function startIdleTimer(opts: IdleTimerOptions): IdleTimer {
  const now = opts.now ?? Date.now;
  const target = opts.target ?? window;
  const doc = opts.doc ?? document;
  let last = now();
  let timer: ReturnType<typeof setTimeout> | null = null;
  let done = false;

  const fire = () => {
    if (done) return;
    done = true;
    stop();
    opts.onIdle();
  };
  const schedule = (ms: number) => {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(check, Math.max(0, ms));
  };
  const check = () => {
    timer = null;
    const elapsed = now() - last;
    if (elapsed >= opts.idleMs) fire();
    else schedule(opts.idleMs - elapsed);
  };
  const activity = () => {
    if (done) return;
    if (now() - last >= opts.idleMs) {
      fire();
      return;
    }
    last = now();
  };
  const onVisibility = () => {
    if (doc.visibilityState === "visible") check();
  };
  const options = { capture: true, passive: true } as const;
  for (const e of ACTIVITY_EVENTS) target.addEventListener(e, activity, options);
  doc.addEventListener("visibilitychange", onVisibility);
  schedule(opts.idleMs);

  function stop() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    for (const e of ACTIVITY_EVENTS) target.removeEventListener(e, activity, options);
    doc.removeEventListener("visibilitychange", onVisibility);
  }

  return {
    activity,
    stop: () => {
      done = true;
      stop();
    },
  };
}
