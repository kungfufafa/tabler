import { isMock } from "@/lib/ipc";
import { PluginCancelled } from "./execute";
import { withPluginState } from "./pluginPrefs";
import { confirmOpenUrl, pluginStates } from "./store";
import { setPref } from "@/stores/prefs";

/** Longest link a plugin may open: data rides in a link's address, so its size is bounded. */
export const MAX_PLUGIN_URL_LENGTH = 2048;

/** A plugin may open at most this many links per window (a loop would open a tab per call). */
export const MAX_LINKS_PER_WINDOW = 3;
export const LINK_WINDOW_MS = 10_000;

/** The link as `SystemService.openURL` opens it; throws for anything but http(s). */
export function checkPluginUrl(url: string): string {
  const raw = url.trim();
  if (raw.length > MAX_PLUGIN_URL_LENGTH)
    throw new Error(`The link is longer than ${MAX_PLUGIN_URL_LENGTH} characters`);
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    throw new Error(`Not a link: ${url.slice(0, 200)}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:")
    throw new Error("Plugins can only open http:// and https:// links");
  // Normalising can lengthen the address (non-ASCII characters become %XX): what the browser
  // gets is bounded too.
  if (parsed.href.length > MAX_PLUGIN_URL_LENGTH)
    throw new Error(`The link is longer than ${MAX_PLUGIN_URL_LENGTH} characters`);
  return parsed.href;
}

/** When each plugin opened its recent links (a module-level map: it outlives a command run). */
const recentLinks = new Map<string, number[]>();

/** Test-only: forgets the links opened so far. */
export function resetPluginUrlLimitsForTests(): void {
  recentLinks.clear();
}

/**
 * Takes one of the plugin's link slots (throws when the window is used up). The returned function
 * gives it back (the user declined the link).
 */
function reserveLink(identifier: string): () => void {
  const now = Date.now();
  const recent = (recentLinks.get(identifier) ?? []).filter((t) => now - t < LINK_WINDOW_MS);
  if (recent.length >= MAX_LINKS_PER_WINDOW) {
    recentLinks.set(identifier, recent);
    throw new Error("too many links opened");
  }
  recent.push(now);
  recentLinks.set(identifier, recent);
  return () => {
    const list = recentLinks.get(identifier);
    const at = list?.indexOf(now) ?? -1;
    if (list && at >= 0) list.splice(at, 1);
  };
}

/** How `openPluginUrl` shows its confirmation (tests and built-in plugins use the defaults). */
export interface UrlDialogs {
  /** Shows a dialog for the run (see `RunHooks.dialog`). */
  dialog?: <T>(show: () => Promise<T>) => Promise<T>;
  /** The sandboxed run that asks; its confirmation is dismissed when the run ends. */
  runId?: number;
}

/**
 * `SystemService.openURL(url)`: http(s) only, at most `MAX_PLUGIN_URL_LENGTH` characters and
 * `MAX_LINKS_PER_WINDOW` links per `LINK_WINDOW_MS`, in the default browser (tauri-plugin-opener).
 * The first link a plugin opens asks first; the answer is remembered per plugin.
 */
export async function openPluginUrl(
  plugin: { identifier: string; name: string },
  url: string,
  options: UrlDialogs = {},
): Promise<void> {
  const href = checkPluginUrl(url);
  const release = reserveLink(plugin.identifier);
  if (pluginStates()[plugin.identifier]?.urlConfirmed !== true) {
    const dialog = options.dialog ?? (<T>(show: () => Promise<T>) => show());
    let ok: boolean;
    try {
      ok = await dialog(() => confirmOpenUrl(plugin.name, href, options.runId ?? null));
    } catch (e) {
      release();
      throw e;
    }
    if (!ok) {
      release();
      throw new PluginCancelled("The link was not opened");
    }
    setPref(
      "pluginStates",
      withPluginState(pluginStates(), plugin.identifier, { urlConfirmed: true }),
    );
  }
  if (isMock) {
    window.open(href, "_blank", "noopener");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(href);
}
