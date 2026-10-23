import type { SafeModeLevel } from "@/lib/ipc";

/**
 * TablePlus's safe-mode levels, in its menu order, with its labels and descriptions. `summary` is
 * Tabler's own plain-language line shown next to the label: the numbered names alone do not say
 * what a level does.
 */
export const SAFE_MODE_OPTIONS = [
  {
    level: "silent",
    label: "Silent Mode",
    summary: "No confirmation",
    description: "Send queries to the server without any warnings",
  },
  {
    level: "alert",
    label: "Alert Mode 1",
    summary: "Confirm every query",
    description: "Warn before sending queries to the server",
  },
  {
    level: "alertNonSelect",
    label: "Alert Mode 2",
    summary: "Confirm queries that change data",
    description: "Warn before sending queries to the server, except SELECT/EXPLAIN/SHOW queries",
  },
  {
    level: "safe",
    label: "Safe Mode 1",
    summary: "Password for every query",
    description: "Prompt for password before sending queries to the server",
  },
  {
    level: "safeNonSelect",
    label: "Safe Mode 2",
    summary: "Password for queries that change data",
    description:
      "Prompt for password before sending queries to the server, except SELECT/EXPLAIN/SHOW queries",
  },
] as const satisfies readonly {
  level: SafeModeLevel;
  label: string;
  summary: string;
  description: string;
}[];

export const SAFE_MODE_LABEL = Object.fromEntries(
  SAFE_MODE_OPTIONS.map((o) => [o.level, o.label]),
) as Record<SafeModeLevel, string>;

export const SAFE_MODE_SUMMARY = Object.fromEntries(
  SAFE_MODE_OPTIONS.map((o) => [o.level, o.summary]),
) as Record<SafeModeLevel, string>;

export const SAFE_MODE_DESCRIPTION = Object.fromEntries(
  SAFE_MODE_OPTIONS.map((o) => [o.level, o.description]),
) as Record<SafeModeLevel, string>;

/** "Alert Mode 2 · Confirm queries that change data": the label with what the level does. */
export function safeModeText(level: SafeModeLevel): string {
  return `${SAFE_MODE_LABEL[level]} · ${SAFE_MODE_SUMMARY[level]}`;
}

/** A profile's level; profiles saved before safe mode existed are silent. */
export function effectiveLevel(level: SafeModeLevel | null | undefined): SafeModeLevel {
  return level ?? "silent";
}

/** Safe Mode 1 / 2: the password is asked. */
export function isPasswordLevel(level: SafeModeLevel): boolean {
  return level === "safe" || level === "safeNonSelect";
}

/** Alert Mode 1 / 2: a confirmation is asked. */
export function isAlertLevel(level: SafeModeLevel): boolean {
  return level === "alert" || level === "alertNonSelect";
}

/** How much a level asks: silent < Alert 2 < Alert 1 < Safe 2 < Safe 1. */
const STRICTNESS: Record<SafeModeLevel, number> = {
  silent: 0,
  alertNonSelect: 1,
  alert: 2,
  safeNonSelect: 3,
  safe: 4,
};

/**
 * Whether switching `from` → `to` needs the connection's password first: leaving a Safe level for
 * one that asks less (Safe 1 → Safe 2 too). Moving up never asks.
 */
export function lowersSafeLevel(from: SafeModeLevel, to: SafeModeLevel): boolean {
  return isPasswordLevel(from) && STRICTNESS[to] < STRICTNESS[from];
}
