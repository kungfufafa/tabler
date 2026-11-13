import { toast } from "sonner";
import { create } from "zustand";
import { flushFileAutoSaves } from "@/features/editor/fileAutoSave";
import { ipc, toIpcError, type UpdateCheck, type UpdateProgress } from "@/lib/ipc";
import { flushPrefs } from "@/stores/prefs";
import { hasUnsaved } from "@/stores/tabs";

/** A newer build a check found. */
export type AvailableUpdate = Extract<UpdateCheck, { state: "available" }>;

/**
 * Where the update dialog is: closed, checking, one of the check's answers, then the download
 * (with its progress), the downloaded update waiting for the restart, and the install. A failure
 * keeps the update it happened on, so the dialog can offer to try again.
 */
export type UpdatePhase =
  | { kind: "closed" }
  | { kind: "checking" }
  | { kind: "upToDate"; version: string }
  | { kind: "unsupported"; reason: string }
  | { kind: "available"; update: AvailableUpdate }
  | { kind: "downloading"; update: AvailableUpdate; progress: UpdateProgress | null }
  | { kind: "ready"; update: AvailableUpdate }
  | { kind: "installing"; update: AvailableUpdate }
  | { kind: "error"; message: string; update: AvailableUpdate | null };

export const useUpdaterStore = create<{ phase: UpdatePhase }>()(() => ({
  phase: { kind: "closed" },
}));

const setPhase = (phase: UpdatePhase) => useUpdaterStore.setState({ phase });
const phase = () => useUpdaterStore.getState().phase;

/** The dialog phase a check's answer opens on (`skipped` never reaches the dialog). */
function phaseOf(result: UpdateCheck): UpdatePhase {
  switch (result.state) {
    case "available":
      return result.downloaded
        ? { kind: "ready", update: result }
        : { kind: "available", update: result };
    case "upToDate":
      return { kind: "upToDate", version: result.version };
    case "unsupported":
      return { kind: "unsupported", reason: result.reason };
    case "skipped":
      return { kind: "closed" };
  }
}

/** While a download or an install runs, the dialog stays and a second check waits. */
export function isBusy(p: UpdatePhase): boolean {
  return p.kind === "checking" || p.kind === "downloading" || p.kind === "installing";
}

/** Check for Updates… (the app menu, Settings ▸ General): opens the dialog and asks. */
export async function checkForUpdates(): Promise<void> {
  if (isBusy(phase())) return;
  setPhase({ kind: "checking" });
  try {
    setPhase(phaseOf(await ipc.updater.check(false)));
  } catch (e) {
    setPhase({ kind: "error", message: toIpcError(e).message, update: null });
  }
}

/** Toast id of the launch check's notice (one notice, however many times it is raised). */
export const UPDATE_TOAST_ID = "update-available";

/**
 * The quiet check at launch: says nothing unless a newer build is published, and then only a
 * notice whose button opens the dialog. Failures (offline, a release being published) stay in
 * the console; only the first window of a run gets an answer.
 */
export async function checkAtLaunch(): Promise<void> {
  let result: UpdateCheck;
  try {
    result = await ipc.updater.check(true);
  } catch (e) {
    console.warn("[updater] the check at launch failed", toIpcError(e).message);
    return;
  }
  if (result.state !== "available") return;
  toast.message(`Tabler ${result.version} is available`, {
    id: UPDATE_TOAST_ID,
    description: `You have ${result.currentVersion}.`,
    duration: 20_000,
    action: {
      label: "Update…",
      onClick: () => {
        if (!isBusy(phase())) setPhase(phaseOf(result));
      },
    },
  });
}

/** Downloads the update on screen; the dialog follows the progress, then offers the restart. */
export async function downloadUpdate(): Promise<void> {
  const current = phase();
  const update = current.kind === "available" || current.kind === "error" ? current.update : null;
  if (!update) return;
  setPhase({ kind: "downloading", update, progress: null });
  try {
    await ipc.updater.download((progress) => {
      // A message that arrives after a cancel must not bring the bar back.
      if (phase().kind === "downloading") setPhase({ kind: "downloading", update, progress });
    });
    setPhase({ kind: "ready", update: { ...update, downloaded: true } });
  } catch (e) {
    const error = toIpcError(e);
    if (error.code === "CANCELLED") setPhase({ kind: "available", update });
    else setPhase({ kind: "error", message: error.message, update });
  }
}

/** Stops the download (the dialog goes back to the offer once the backend confirms). */
export async function cancelDownload(): Promise<void> {
  try {
    await ipc.updater.cancel();
  } catch (e) {
    console.warn("[updater] cancel failed", toIpcError(e).message);
  }
}

/**
 * Installs the downloaded update and restarts into it. Not while this window holds unsaved work
 * (the backend refuses for the other windows): a restart must not drop it.
 */
export async function installUpdate(): Promise<void> {
  const current = phase();
  if (current.kind !== "ready") return;
  const { update } = current;
  if (hasUnsaved()) {
    toast.message("Save or close the unsaved work first", {
      description: "Then install the update: Tabler restarts to finish it.",
    });
    return;
  }
  setPhase({ kind: "installing", update });
  flushFileAutoSaves();
  await flushPrefs();
  try {
    await ipc.updater.install();
  } catch (e) {
    // Still downloaded: the dialog goes back to the restart offer.
    setPhase({ kind: "ready", update });
    toast.error("Could not install the update", { description: toIpcError(e).message });
  }
}

/** Closes the dialog, unless a download or an install is running (those have their own buttons). */
export function closeUpdateDialog(): void {
  if (isBusy(phase())) return;
  setPhase({ kind: "closed" });
}

/** Test-only: back to a closed dialog. */
export function resetUpdaterStore(): void {
  setPhase({ kind: "closed" });
}
