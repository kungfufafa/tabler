import type { Dialect, SafeModeLevel } from "@/lib/ipc";
import { allReads } from "./classify";
import type { GuardRequest } from "./guard";

/** What a safe-mode check needs before a request is sent. */
export type GuardDecision =
  /** Send it. */
  | "go"
  /** Confirmation alert (Alert Mode 1 / 2). */
  | "alert"
  /** The connection's password (Safe Mode 1 / 2). */
  | "password"
  /** Never: the connection is read-only and the request writes. */
  | "blocked";

/**
 * Whether `request` may change the server: commits always do; queries unless all are READs. With
 * no dialect (a driver beyond the core ones, whose SQL the core classifier cannot read) every
 * query counts as a write.
 */
export function requestWrites(request: GuardRequest, dialect: Dialect | null): boolean {
  switch (request.kind) {
    case "commit":
      return true;
    case "operation":
      return request.write;
    case "query":
      return dialect === null || !allReads(request.statements, dialect);
  }
}

/**
 * TablePlus's decision table: Silent sends, Alert 1 / Safe 1 ask for every request, Alert 2 /
 * Safe 2 only for writes. A write on a read-only connection (a Tabler extra) is refused first.
 */
export function decide(level: SafeModeLevel, writes: boolean, readOnly: boolean): GuardDecision {
  if (readOnly && writes) return "blocked";
  switch (level) {
    case "silent":
      return "go";
    case "alert":
      return "alert";
    case "alertNonSelect":
      return writes ? "alert" : "go";
    case "safe":
      return "password";
    case "safeNonSelect":
      return writes ? "password" : "go";
  }
}
