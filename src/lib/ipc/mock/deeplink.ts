import type { OpenRequest } from "../bindings/OpenRequest";
import type { SqliteSshSync } from "../bindings/SqliteSshSync";
import { mockEmit, registerMock } from "./transport";

/**
 * Mock of opening Tabler from outside. In `pnpm dev:mock` there is no Finder or `open -a`, so a
 * request can be sent from the browser:
 * - the page URL `?open=<url-encoded connection URL or file path>` (repeatable) at start, or
 * - `window.tablerOpen("postgres://…?table=users&column=id&operation=%3E&value=10")` later
 *   (a path such as `/Users/me/app.sqlite` or `/tmp/report.sql` opens as a file).
 */

const queue: OpenRequest[] = [];
const temporary = new Set<string>();

function toRequest(text: string): OpenRequest {
  return /^[a-z][a-z0-9+.-]*:/i.test(text) && !text.startsWith("file:")
    ? { kind: "url", url: text }
    : { kind: "file", path: text.replace(/^file:\/\//, "") };
}

/** Queues a request and tells the renderer, like `RunEvent::Opened`. */
export function mockOpen(text: string): void {
  queue.push(toRequest(text));
  mockEmit("deeplink:open", null);
}

registerMock("take_pending_opens", () => queue.splice(0, queue.length));
registerMock("mark_temporary_profile", ({ profileId, temporary: on }) => {
  if (on) temporary.add(String(profileId));
  else temporary.delete(String(profileId));
});
registerMock("list_temporary_profiles", () => [...temporary]);
registerMock("sqlite_ssh_upload", (): SqliteSshSync => ({
  uploaded: true,
  conflict: false,
}));

if (typeof window !== "undefined") {
  for (const open of new URLSearchParams(window.location.search).getAll("open")) {
    queue.push(toRequest(open));
  }
  (window as unknown as { tablerOpen?: (text: string) => void }).tablerOpen = mockOpen;
}
