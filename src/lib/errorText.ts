import { toIpcError } from "@/lib/ipc";

/**
 * An error as the user reads it: the message, then the server's or the app's hint when it has one
 * ("Query timed out after 300 s" + where to change the limit). The grid's load and count errors
 * show this, so a statement stopped by the query time out says so and how to change it.
 */
export function errorWithHint(err: unknown): string {
  const { message, payload } = toIpcError(err);
  const hint = payload.hint?.trim();
  return hint ? `${message} ${hint}` : message;
}
