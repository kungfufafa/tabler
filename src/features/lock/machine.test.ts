import { describe, expect, it } from "vitest";
import {
  attemptsLeft,
  canAttempt,
  COOLDOWN_MS,
  cooldownLeft,
  cooldownMessage,
  MAX_ATTEMPTS,
  NO_ATTEMPTS,
  recordFailure,
  type Attempts,
} from "./machine";

describe("lock attempts", () => {
  it("allows 5 wrong passcodes, then a 30 s cooldown, then starts over", () => {
    let a: Attempts = NO_ATTEMPTS;
    for (let i = 1; i < MAX_ATTEMPTS; i++) {
      a = recordFailure(a, 1000);
      expect(a.failures).toBe(i);
      expect(canAttempt(a, 1000)).toBe(true);
      expect(attemptsLeft(a)).toBe(MAX_ATTEMPTS - i);
    }
    a = recordFailure(a, 2000);
    expect(a).toEqual({ failures: 0, cooldownUntil: 2000 + COOLDOWN_MS });
    expect(canAttempt(a, 2000)).toBe(false);
    expect(cooldownLeft(a, 2000 + 10_000)).toBe(20_000);
    expect(canAttempt(a, 2000 + COOLDOWN_MS)).toBe(true);
    // After the cooldown the count starts over.
    a = recordFailure(a, 2000 + COOLDOWN_MS + 1);
    expect(a).toEqual({ failures: 1, cooldownUntil: null });
  });

  it("words the cooldown in whole seconds", () => {
    expect(cooldownMessage(29_100)).toBe("Too many attempts. Try again in 30 s.");
    expect(cooldownMessage(1)).toBe("Too many attempts. Try again in 1 s.");
  });
});
