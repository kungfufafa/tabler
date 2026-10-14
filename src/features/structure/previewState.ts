import { toIpcError, type ErrorPayload } from "@/lib/ipc";

export interface DdlErrorInfo {
  code: string;
  message: string;
  detail: string | null;
  hint: string | null;
}

export function errorInfo(e: unknown): DdlErrorInfo {
  const err = toIpcError(e);
  return {
    code: err.code,
    message: err.message,
    detail: err.payload.detail ?? null,
    hint: err.payload.hint ?? null,
  };
}

export function payloadInfo(p: ErrorPayload): DdlErrorInfo {
  return { code: p.code, message: p.message, detail: p.detail, hint: p.hint };
}

/** False for session / transaction control statements (PRAGMA, SET, BEGIN, COMMIT…). */
export function changesSchema(sql: string): boolean {
  return !/^\s*(pragma|set|begin|commit|rollback|start\s+transaction)\b/i.test(sql);
}
