import { create } from "zustand";
import { ipc, toIpcError } from "@/lib/ipc";
import { broadcastEvent, listenEvent, RENDERER_ID } from "@/lib/ipc/events";
import { authenticateOwner } from "./biometric";
import {
  attemptsLeft,
  canAttempt,
  cooldownLeft,
  cooldownMessage,
  NO_ATTEMPTS,
  recordFailure,
  type Attempts,
} from "./machine";

/**
 * The app lock (Settings ▸ Security ▸ Passcode). "locked" renders the lock screen over the whole
 * window and keeps every action from running (see `LockHost`). A passcode set in an earlier
 * session is mirrored to localStorage, so the window starts locked before the backend answered.
 */

export type LockStatus = "locked" | "unlocked";
/** Why the window locked: startup, the idle timer, the system (sleep / screen lock), the user. */
export type LockReason = "startup" | "idle" | "system" | "user";

/** localStorage flag: a passcode is set (the backend is the source of truth). */
export const LOCK_LOCAL_KEY = "tabler.lock.enabled";

function readLocal(): boolean {
  try {
    return globalThis.localStorage?.getItem(LOCK_LOCAL_KEY) === "1";
  } catch {
    return false;
  }
}

function writeLocal(enabled: boolean): void {
  try {
    if (enabled) globalThis.localStorage?.setItem(LOCK_LOCAL_KEY, "1");
    else globalThis.localStorage?.removeItem(LOCK_LOCAL_KEY);
  } catch {
    // Only the first paint depends on it.
  }
}

export interface LockState {
  status: LockStatus;
  reason: LockReason | null;
  /** A passcode is set. */
  enabled: boolean;
  /** Touch ID (or the account password) can be asked on this Mac. */
  biometrics: boolean;
  /** The backend's status has been read. */
  checked: boolean;
  attempts: Attempts;
  /** Bumped by every lock: the lock screen offers Touch ID once per lock. */
  lockCount: number;
}

function initialState(): LockState {
  const enabled = readLocal();
  return {
    status: enabled ? "locked" : "unlocked",
    reason: enabled ? "startup" : null,
    enabled,
    biometrics: false,
    checked: false,
    attempts: NO_ATTEMPTS,
    lockCount: enabled ? 1 : 0,
  };
}

export const useLockStore = create<LockState>()(() => initialState());

/** Test helper: a fresh store (reads the localStorage flag again). */
export function resetLockForTests(): void {
  useLockStore.setState(initialState(), true);
}

/**
 * Reads whether a passcode is set: the window locks at startup when one is, and unlocks when the
 * localStorage flag was stale. Without an answer (the backend failed) a locked window stays
 * locked.
 */
export async function loadLockStatus(): Promise<void> {
  try {
    const { enabled, biometrics } = await ipc.security.passcodeStatus();
    writeLocal(enabled);
    useLockStore.setState((s) => {
      if (enabled && !s.checked && s.status === "unlocked")
        return {
          enabled,
          biometrics,
          checked: true,
          status: "locked",
          reason: "startup",
          lockCount: s.lockCount + 1,
        };
      if (!enabled) return { enabled, biometrics, checked: true, status: "unlocked", reason: null };
      return { enabled, biometrics, checked: true };
    });
  } catch (err) {
    console.warn("[lock] could not read the passcode status", err);
    useLockStore.setState({ checked: true });
  }
}

/** Locks now (nothing happens without a passcode). Returns whether the window is locked. */
export function lockNow(reason: LockReason): boolean {
  const s = useLockStore.getState();
  if (!s.enabled) return false;
  if (s.status === "locked") return true;
  useLockStore.setState({ status: "locked", reason, lockCount: s.lockCount + 1 });
  return true;
}

function unlock(): void {
  useLockStore.setState({ status: "unlocked", reason: null, attempts: NO_ATTEMPTS });
}

/** Result of an unlock attempt: ok, or the message the lock screen shows. */
export type UnlockResult = { ok: true } | { ok: false; message: string };

/** Checks a typed passcode (refused without checking during the cooldown). */
export async function unlockWithPasscode(
  passcode: string,
  now: () => number = Date.now,
): Promise<UnlockResult> {
  const before = useLockStore.getState().attempts;
  if (!canAttempt(before, now()))
    return { ok: false, message: cooldownMessage(cooldownLeft(before, now())) };
  let ok: boolean;
  try {
    ok = await ipc.security.verifyPasscode(passcode);
  } catch (err) {
    return { ok: false, message: `Could not check the passcode: ${toIpcError(err).message}` };
  }
  if (ok) {
    unlock();
    return { ok: true };
  }
  const attempts = recordFailure(useLockStore.getState().attempts, now());
  useLockStore.setState({ attempts });
  if (!canAttempt(attempts, now()))
    return { ok: false, message: cooldownMessage(cooldownLeft(attempts, now())) };
  const left = attemptsLeft(attempts);
  return {
    ok: false,
    message: `Wrong passcode. ${left} attempt${left === 1 ? "" : "s"} left.`,
  };
}

/** Touch ID / the account password instead of the passcode. */
export async function unlockWithBiometric(): Promise<UnlockResult> {
  const result = await authenticateOwner("unlock Tabler");
  if (result === "ok") {
    unlock();
    return { ok: true };
  }
  if (result === "unavailable") {
    useLockStore.setState({ biometrics: false });
    return { ok: false, message: "Touch ID is not available. Enter the passcode." };
  }
  return { ok: false, message: "" };
}

function applyPasscodeEnabled(enabled: boolean): void {
  writeLocal(enabled);
  useLockStore.setState(
    enabled ? { enabled } : { enabled, status: "unlocked", reason: null, attempts: NO_ATTEMPTS },
  );
}

/** After Settings ▸ Security changed the passcode; the other windows follow (`installLockSync`). */
export function setPasscodeEnabled(enabled: boolean): void {
  applyPasscodeEnabled(enabled);
  void broadcastEvent("lock:changed", { from: RENDERER_ID, enabled }).catch((err: unknown) =>
    console.warn("[lock] could not tell the other windows", err),
  );
}

/**
 * Follows the passcode set, changed or turned off in another window (`lock:changed`): Lock Tabler
 * and the idle timer work here at once, and turning it off unlocks. Returns the uninstall function.
 */
export function installLockSync(): () => void {
  let gone = false;
  let unlisten: (() => void) | null = null;
  void listenEvent("lock:changed", ({ from, enabled }) => {
    if (from !== RENDERER_ID) applyPasscodeEnabled(enabled);
  }).then((u) => {
    if (gone) u();
    else unlisten = u;
  });
  return () => {
    gone = true;
    unlisten?.();
  };
}
