import { ipc, toIpcError } from "@/lib/ipc";

/** How a Touch ID request ended. "unavailable": this Mac cannot ask (or the call failed). */
export type BiometricResult = "ok" | "cancelled" | "unavailable";

let inFlight: Promise<BiometricResult> | null = null;

/**
 * Asks for Touch ID, or the macOS account password when Touch ID is not set up (the system
 * dialog reads "Tabler is trying to <reason>."). One system dialog at a time: a call while one is
 * open shares its answer.
 */
export function authenticateOwner(reason: string): Promise<BiometricResult> {
  inFlight ??= ipc.security
    .authenticate(reason)
    .then(
      (ok): BiometricResult => (ok ? "ok" : "cancelled"),
      (err: unknown): BiometricResult => {
        const e = toIpcError(err);
        if (e.code !== "UNSUPPORTED") console.warn("[lock] Touch ID failed", e);
        return e.code === "TIMEOUT" ? "cancelled" : "unavailable";
      },
    )
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}
