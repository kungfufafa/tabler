import type { UpdateCheck } from "../bindings/UpdateCheck";
import type { UpdateProgress } from "../bindings/UpdateProgress";
import { mockError, mockLatency, registerMock } from "./transport";

// Mock handlers for Check for Updates…: update_check, update_download, update_cancel,
// update_install. Mirrors `src-tauri/src/updater.rs`. The browser build is up to date unless a
// test publishes an update (`setMockUpdate`) or the page is opened with `?mockUpdate=<version>`.

/** The version the mock build reports (`app_info`) and compares updates against. */
export const MOCK_APP_VERSION = "0.1.0";

export interface MockUpdate {
  version: string;
  notes?: string | null;
  date?: string | null;
  /** Size of the download in bytes. */
  size?: number;
  /** The download fails with this message. */
  failDownload?: string;
  /** The install is refused with this message (another window has unsaved work, say). */
  failInstall?: string;
}

const DEFAULT_SIZE = 48 * 1024 * 1024;
const CHUNKS = 24;

function fromUrl(): MockUpdate | null {
  if (typeof window === "undefined") return null;
  const version = new URLSearchParams(window.location.search).get("mockUpdate");
  if (!version) return null;
  return {
    version,
    date: "2026-11-12T08:00:00Z",
    notes: [
      version,
      "",
      "New",
      "- Updates install from inside the app",
      "",
      "Fixed",
      "- The sidebar keeps its width",
      "",
      MOCK_APP_VERSION,
      "",
      "New",
      "- What this build already has (not shown: it is not news)",
    ].join("\n"),
  };
}

let published: MockUpdate | null = fromUrl();
let downloaded = false;
let launchChecked = false;
let cancelDownload: (() => void) | null = null;

/** The versions `update_install` installed, in order. */
export const mockUpdateInstalls: string[] = [];

/** Test helper: publishes `update` (null: the build is up to date again). */
export function setMockUpdate(update: MockUpdate | null): void {
  published = update;
  downloaded = false;
}

/** Test helper: back to an up-to-date build that has not checked at launch. */
export function resetMockUpdater(): void {
  published = null;
  downloaded = false;
  launchChecked = false;
  cancelDownload = null;
  mockUpdateInstalls.length = 0;
}

registerMock("update_check", ({ launch }): UpdateCheck => {
  if (launch) {
    if (launchChecked) return { state: "skipped" };
    launchChecked = true;
  }
  if (!published) return { state: "upToDate", version: MOCK_APP_VERSION };
  return {
    state: "available",
    version: published.version,
    currentVersion: MOCK_APP_VERSION,
    notes: published.notes ?? null,
    date: published.date ?? null,
    downloaded,
  };
});

registerMock("update_download", async ({ onProgress }) => {
  const update = published;
  if (!update) mockError("INVALID_INPUT", "Check for updates first: no update was found.");
  const send = (onProgress as { send?: (p: UpdateProgress) => void } | undefined)?.send;
  const total = update.size ?? DEFAULT_SIZE;
  if (downloaded) {
    send?.({ downloaded: total, total });
    return null;
  }
  if (cancelDownload) mockError("INVALID_INPUT", "The update is already downloading.");
  let cancelled = false;
  cancelDownload = () => {
    cancelled = true;
  };
  try {
    for (let i = 1; i <= CHUNKS; i++) {
      // The browser shows the bar moving; tests (no latency) only yield between chunks.
      await new Promise((r) => setTimeout(r, mockLatency > 0 ? 90 : 0));
      if (cancelled) mockError("CANCELLED", "query cancelled");
      if (update.failDownload && i === 2) mockError("INTERNAL", update.failDownload);
      send?.({ downloaded: Math.round((total * i) / CHUNKS), total });
    }
  } finally {
    cancelDownload = null;
  }
  downloaded = published === update;
  return null;
});

registerMock("update_cancel", () => {
  cancelDownload?.();
  return null;
});

registerMock("update_install", () => {
  const update = published;
  if (!update || !downloaded) mockError("INVALID_INPUT", "The update has not been downloaded yet.");
  if (update.failInstall) mockError("INVALID_INPUT", update.failInstall);
  mockUpdateInstalls.push(update.version);
  published = null;
  downloaded = false;
  // The browser stand-in for the restart into the new build.
  if (import.meta.env.MODE !== "test") window.location.reload();
  return null;
});
