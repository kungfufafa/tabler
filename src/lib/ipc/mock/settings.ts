import { mockError, registerMock } from "./transport";

/**
 * In-memory stand-in for the app DB `settings` table (key → JSON value). Mirrors the Rust side:
 * `get_settings` returns every key, `set_setting` with `null` deletes the key.
 */
const settings = new Map<string, unknown>();

/** Values cross the IPC boundary as JSON; cloning keeps callers from mutating stored state. */
const clone = (v: unknown): unknown => (v === undefined ? null : JSON.parse(JSON.stringify(v)));

registerMock("get_settings", () =>
  Object.fromEntries([...settings].map(([k, v]) => [k, clone(v)])),
);

registerMock("set_setting", ({ key, value }) => {
  if (typeof key !== "string" || key.length === 0)
    mockError("INVALID_INPUT", "setting key required");
  if (value === null || value === undefined) settings.delete(key);
  else settings.set(key, clone(value));
  return null;
});

/** Test helper: wipe the in-memory settings. */
export function resetMockSettings(): void {
  settings.clear();
}
