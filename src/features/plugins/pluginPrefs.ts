/**
 * The `pluginStates` preference: per plugin identifier, whether it is enabled, whether the user
 * granted a third-party plugin its permissions, and whether it may open links without asking.
 * Kept free of app imports: `src/stores/prefs.ts` imports it.
 */

export interface PluginState {
  /** Built-in plugins are enabled unless this is false; installed ones only when it is true. */
  enabled?: boolean;
  /** A third-party plugin's permissions were granted (asked on its first enable). */
  granted?: boolean;
  /** `SystemService.openURL` was confirmed once for this plugin. */
  urlConfirmed?: boolean;
}

export type PluginStates = Readonly<Record<string, Readonly<PluginState>>>;

export const EMPTY_PLUGIN_STATES: PluginStates = Object.freeze({});

const FLAGS = ["enabled", "granted", "urlConfirmed"] as const;

/** True for a value the `pluginStates` preference accepts. */
export function isPluginStates(raw: unknown): raw is PluginStates {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return false;
  return Object.entries(raw as Record<string, unknown>).every(
    ([id, state]) =>
      id.length > 0 &&
      id.length <= 128 &&
      state !== null &&
      typeof state === "object" &&
      !Array.isArray(state) &&
      Object.entries(state as Record<string, unknown>).every(
        ([k, v]) => (FLAGS as readonly string[]).includes(k) && typeof v === "boolean",
      ),
  );
}

export function isPluginEnabled(states: PluginStates, id: string, builtin: boolean): boolean {
  const state = states[id];
  return builtin ? state?.enabled !== false : state?.enabled === true && state.granted === true;
}

/** `states` with `patch` merged into `id`'s entry (a new object: the store compares by identity). */
export function withPluginState(
  states: PluginStates,
  id: string,
  patch: PluginState,
): PluginStates {
  return { ...states, [id]: { ...states[id], ...patch } };
}

/** `states` without `id` (a removed plugin starts over when installed again). */
export function withoutPlugin(states: PluginStates, id: string): PluginStates {
  if (!(id in states)) return states;
  const next = { ...states };
  delete next[id];
  return next;
}
