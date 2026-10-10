/** "3 minutes ago", "yesterday", "12 Oct" — compact relative time for lists. */
export function relativeTime(iso: string | null | undefined, now: Date = new Date()): string {
  if (!iso) return "never";
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return "";
  const diff = (now.getTime() - then.getTime()) / 1000;
  if (diff < 45) return "just now";
  if (diff < 3600) return `${Math.round(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.round(diff / 3600)} h ago`;
  if (diff < 172800) return "yesterday";
  if (diff < 7 * 86400) return `${Math.round(diff / 86400)} days ago`;
  return then.toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)} s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.round((ms % 60_000) / 1000);
  return `${m}m ${s}s`;
}

export function formatCount(n: number | null | undefined, approx = false): string {
  if (n === null || n === undefined) return "";
  const text = new Intl.NumberFormat().format(n);
  return approx ? `~${text}` : text;
}
