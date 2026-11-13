import type { UpdateProgress } from "@/lib/ipc";
import { formatBytes } from "@/lib/values";

// What the update dialog says about a download and a release's date.

/** "12.4 MB of 48.0 MB", or only what came in while the server has not said the size. */
export function progressLabel(progress: UpdateProgress | null): string {
  if (!progress) return "Starting the download…";
  const got = formatBytes(progress.downloaded);
  return progress.total ? `${got} of ${formatBytes(progress.total)}` : got;
}

/** 0–100 for the bar; null while the size is unknown (the bar then only pulses). */
export function progressPercent(progress: UpdateProgress | null): number | null {
  if (!progress?.total) return null;
  return Math.min(100, Math.round((progress.downloaded / progress.total) * 100));
}

/** "12 November 2026" from the release's RFC 3339 date; "" when it has none or it is not a date. */
export function publishedLabel(date: string | null): string {
  if (!date) return "";
  const at = new Date(date);
  if (Number.isNaN(at.getTime())) return "";
  return at.toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" });
}

/** A line that is nothing but a version opens that version's part of the notes. */
const VERSION_LINE = /^\d+\.\d+\.\d+$/;

/**
 * The notes of the versions newer than `current`. A release's notes list the newest versions,
 * newest first, each under a line with its number (dev/scripts/release-notes.mjs); what the
 * running version and older ones brought is not news to this user. Notes in another shape are
 * shown whole.
 */
export function notesSince(notes: string | null, current: string): string | null {
  if (!notes) return null;
  const lines = notes.split("\n");
  const at = lines.findIndex((line) => VERSION_LINE.test(line.trim()) && line.trim() === current);
  const news = (at > 0 ? lines.slice(0, at) : lines).join("\n").trim();
  return news || null;
}
