/**
 * The lock screen's attempt counter: 5 wrong passcodes in a row start a 30 s cooldown during which
 * nothing is checked; the count starts over after it.
 */

export const MAX_ATTEMPTS = 5;
export const COOLDOWN_MS = 30_000;

export interface Attempts {
  /** Wrong passcodes since the last success or cooldown. */
  failures: number;
  /** Epoch ms until which no attempt is checked; null = none. */
  cooldownUntil: number | null;
}

export const NO_ATTEMPTS: Attempts = Object.freeze({ failures: 0, cooldownUntil: null });

/** Milliseconds of cooldown left at `now` (0 = an attempt may be made). */
export function cooldownLeft(a: Attempts, now: number): number {
  return a.cooldownUntil === null ? 0 : Math.max(0, a.cooldownUntil - now);
}

export function canAttempt(a: Attempts, now: number): boolean {
  return cooldownLeft(a, now) === 0;
}

/** After a wrong passcode at `now`: one more failure, or the cooldown at the fifth. */
export function recordFailure(a: Attempts, now: number): Attempts {
  const base = canAttempt(a, now) && a.cooldownUntil !== null ? NO_ATTEMPTS : a;
  const failures = base.failures + 1;
  if (failures >= MAX_ATTEMPTS) return { failures: 0, cooldownUntil: now + COOLDOWN_MS };
  return { failures, cooldownUntil: null };
}

/** Wrong passcodes left before the cooldown. */
export function attemptsLeft(a: Attempts): number {
  return MAX_ATTEMPTS - a.failures;
}

/** "Try again in 27 s". */
export function cooldownMessage(ms: number): string {
  return `Too many attempts. Try again in ${Math.max(1, Math.ceil(ms / 1000))} s.`;
}
