import { toast } from "sonner";

/** What the progress toast of a job running behind its closed window says. */
export interface JobToastContent {
  title: string;
  description: string;
}

/** Progress text changes at most this often (a stream reports many times a second). */
export const JOB_TOAST_THROTTLE_MS = 500;

/**
 * The toast that stays while an export or a backup runs with its window closed: a closed window
 * would otherwise leave no trace of a job that can take minutes (TablePlus keeps the progress on
 * screen until the job ends). `sync` takes what it should say now, or null once the window is
 * open again or the job ended; the job's own done / canceled / failed notice follows on its own.
 * "Show" reopens the window on the job's progress.
 */
export function createJobToast({
  id,
  onShow,
  throttleMs = JOB_TOAST_THROTTLE_MS,
  now = Date.now,
}: {
  /** Toast id: one job of a kind runs at a time. */
  id: string;
  onShow: () => void;
  throttleMs?: number;
  now?: () => number;
}) {
  let shown: JobToastContent | null = null;
  /** The latest content while a throttled update waits. */
  let pending: JobToastContent | null = null;
  let at = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** The user closed the toast: no update brings it back until the window opens again. */
  let dismissedByUser = false;
  /**
   * Bumped each time we take the toast down: sonner reports our own `toast.dismiss` through
   * `onDismiss` as well, a render later, which must not count as the user closing a toast that
   * was shown again in the meantime.
   */
  let generation = 0;

  const show = (content: JobToastContent) => {
    const mine = generation;
    shown = content;
    pending = null;
    at = now();
    toast.loading(content.title, {
      id,
      description: content.description,
      duration: Number.POSITIVE_INFINITY,
      action: { label: "Show", onClick: onShow },
      onDismiss: () => {
        if (shown && mine === generation) dismissedByUser = true;
      },
    });
  };

  const clearTimer = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };

  return {
    sync(content: JobToastContent | null): void {
      if (content === null) {
        clearTimer();
        pending = null;
        dismissedByUser = false;
        generation += 1;
        if (shown) {
          shown = null;
          toast.dismiss(id);
        }
        return;
      }
      if (dismissedByUser) return;
      if (shown && shown.title === content.title && shown.description === content.description) {
        pending = null;
        return;
      }
      const wait = shown ? at + throttleMs - now() : 0;
      if (wait <= 0) {
        clearTimer();
        show(content);
        return;
      }
      pending = content;
      timer ??= setTimeout(() => {
        timer = null;
        if (pending && shown && !dismissedByUser) show(pending);
      }, wait);
    },
  };
}
