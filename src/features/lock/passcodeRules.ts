import type { AutoLockMinutes } from "@/stores/prefs";

/** Shortest passcode (the backend's `passcode::MIN_LENGTH`). */
export const MIN_PASSCODE_LENGTH = 4;

export const AUTO_LOCK_LABEL: Record<AutoLockMinutes, string> = {
  0: "Never",
  1: "1 minute",
  5: "5 minutes",
  15: "15 minutes",
  30: "30 minutes",
  60: "1 hour",
};

/** What is wrong with the new passcode fields (null = fine). */
export function newPasscodeError(next: string, confirm: string): string | null {
  if ([...next].length < MIN_PASSCODE_LENGTH)
    return `The passcode needs at least ${MIN_PASSCODE_LENGTH} characters.`;
  if (next !== confirm) return "The passcodes do not match.";
  return null;
}
