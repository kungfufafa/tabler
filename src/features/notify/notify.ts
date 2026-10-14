import { isMock } from "@/lib/ipc";
import { getPrefs } from "@/stores/prefs";

/** Set once Tabler asked macOS for permission to post notifications (it asks only once). */
export const NOTIFY_ASKED_KEY = "tabler.notify.asked";

export interface OsNotice {
  title: string;
  body?: string;
}

/** Whether the window is in the background (another app is active, or it is hidden). */
export function appInBackground(doc: Pick<Document, "hasFocus" | "visibilityState"> = document) {
  try {
    return doc.visibilityState === "hidden" || !doc.hasFocus();
  } catch {
    return false;
  }
}

function askedBefore(): boolean {
  try {
    return localStorage.getItem(NOTIFY_ASKED_KEY) === "1";
  } catch {
    return true;
  }
}

function markAsked(): void {
  try {
    localStorage.setItem(NOTIFY_ASKED_KEY, "1");
  } catch {
    // Storage blocked: macOS remembers the answer itself.
  }
}

/** The plugin's calls, injectable for tests. */
export interface NotifyApi {
  isPermissionGranted: () => Promise<boolean>;
  requestPermission: () => Promise<string>;
  sendNotification: (options: { title: string; body?: string }) => void;
}

const pluginApi = async (): Promise<NotifyApi> => import("@tauri-apps/plugin-notification");

/**
 * Posts a native macOS notification for a long export, import, backup or restore that finished
 * (or failed) while Tabler was in the background, when Settings ▸ "Show notifications" is on.
 * Permission is asked once; clicking the notification brings Tabler to the front (macOS
 * activates the app that posted it). Resolves with whether one was posted.
 */
export async function notifyInBackground(
  notice: OsNotice,
  {
    api = pluginApi,
    background = appInBackground,
  }: { api?: () => Promise<NotifyApi>; background?: () => boolean } = {},
): Promise<boolean> {
  if (!getPrefs().showNotifications || !background()) return false;
  if (isMock && api === pluginApi) return false;
  try {
    const n = await api();
    let granted = await n.isPermissionGranted();
    if (!granted && !askedBefore()) {
      markAsked();
      granted = (await n.requestPermission()) === "granted";
    }
    if (!granted) return false;
    n.sendNotification(notice.body ? { title: notice.title, body: notice.body } : notice);
    return true;
  } catch (e) {
    console.warn("[notify] could not post the notification", e);
    return false;
  }
}

/** Shows a file in Finder (no-op in the browser mock). */
export async function revealInFinder(path: string): Promise<void> {
  if (isMock) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}
