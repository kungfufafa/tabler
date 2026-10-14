import { toast } from "sonner";
import { create } from "zustand";
import { ipc, isMock, type PluginList } from "@/lib/ipc";
import { getPrefs, setPref } from "@/stores/prefs";
import { isPluginEnabled, withoutPlugin, withPluginState, type PluginStates } from "./pluginPrefs";
import type { PluginEntry, PluginItem } from "./types";

/** A question a plugin (or enabling one) asks; one dialog at a time, in order. */
export type PluginPrompt =
  | { id: number; kind: "alert"; plugin: string; title: string; message: string }
  | { id: number; kind: "permission"; plugin: PluginEntry }
  | { id: number; kind: "openUrl"; plugin: string; url: string };

type WithoutId<T> = T extends unknown ? Omit<T, "id"> : never;
type PromptRequest = WithoutId<PluginPrompt>;

export interface DummyDataRequest {
  connectionId: string;
  table: PluginItem;
}

interface PluginsState {
  /** The Plugins folder's plugins; null until loaded. */
  list: PluginList | null;
  loading: boolean;
  /** Why the last load failed. */
  error: string | null;
  managerOpen: boolean;
  prompts: Array<{
    prompt: PluginPrompt;
    resolve: (ok: boolean) => void;
    /** The sandboxed run that asked (its prompts go when it ends); null: asked by the app. */
    run: number | null;
  }>;
  dummy: DummyDataRequest | null;
}

export const usePluginsStore = create<PluginsState>()(() => ({
  list: null,
  loading: false,
  error: null,
  managerOpen: false,
  prompts: [],
  dummy: null,
}));

const errorMessage = (e: unknown) =>
  e instanceof Error
    ? e.message
    : typeof e === "object" && e && "message" in e
      ? String(e.message)
      : String(e);

/** Reloads the installed plugins (Manage Plugins… ▸ Reload, after installs and removals). */
export async function refreshPlugins(): Promise<void> {
  usePluginsStore.setState({ loading: true });
  try {
    const list = await ipc.plugins.list();
    usePluginsStore.setState({ list, loading: false, error: null });
  } catch (e) {
    usePluginsStore.setState({ loading: false, error: errorMessage(e) });
  }
}

export function openPluginManager(): void {
  usePluginsStore.setState({ managerOpen: true });
  void refreshPlugins();
}

export function closePluginManager(): void {
  usePluginsStore.setState({ managerOpen: false });
}

let nextPrompt = 1;
let nextRun = 1;

/** A fresh id for one run of a sandboxed command: the dialogs it opens carry it. */
export function newPluginRunId(): number {
  return nextRun++;
}

/**
 * Queues a prompt; resolves with the user's answer (alerts resolve true once dismissed). `run`
 * tags the prompt with the sandboxed run that asked (see `dismissPluginPrompts`).
 */
export function requestPluginPrompt(
  request: PromptRequest,
  run: number | null = null,
): Promise<boolean> {
  return new Promise((resolve) => {
    const prompt = { ...request, id: nextPrompt++ } as PluginPrompt;
    usePluginsStore.setState((s) => ({ prompts: [...s.prompts, { prompt, resolve, run }] }));
  });
}

/** Answers the prompt on screen (the first one queued). */
export function answerPluginPrompt(id: number, ok: boolean): void {
  const entry = usePluginsStore.getState().prompts.find((p) => p.prompt.id === id);
  if (!entry) return;
  usePluginsStore.setState((s) => ({ prompts: s.prompts.filter((p) => p.prompt.id !== id) }));
  entry.resolve(ok);
}

/**
 * A run ended: every prompt it still has queued (or on screen) goes, answered as declined, so a
 * plugin that was stopped cannot leave a pile of dialogs behind.
 */
export function dismissPluginPrompts(run: number): void {
  const mine = usePluginsStore.getState().prompts.filter((p) => p.run === run);
  if (mine.length === 0) return;
  usePluginsStore.setState((s) => ({ prompts: s.prompts.filter((p) => p.run !== run) }));
  for (const entry of mine) entry.resolve(false);
}

/** `context.alert(title, message)`. */
export async function showPluginAlert(
  plugin: string,
  title: string,
  message: string,
  run: number | null = null,
) {
  await requestPluginPrompt({ kind: "alert", plugin, title, message }, run);
}

export function confirmOpenUrl(
  plugin: string,
  url: string,
  run: number | null = null,
): Promise<boolean> {
  return requestPluginPrompt({ kind: "openUrl", plugin, url }, run);
}

export function pluginStates(): PluginStates {
  return getPrefs().pluginStates;
}

function savePluginStates(states: PluginStates): void {
  setPref("pluginStates", states);
}

/**
 * Enables or disables a plugin. A third-party plugin's first enable lists what it can do and asks
 * for permission. Resolves whether the plugin ends up enabled.
 */
export async function setPluginEnabled(plugin: PluginEntry, enabled: boolean): Promise<boolean> {
  const states = pluginStates();
  if (!enabled) {
    savePluginStates(withPluginState(states, plugin.identifier, { enabled: false }));
    return false;
  }
  if (!plugin.builtin && states[plugin.identifier]?.granted !== true) {
    if (!(await requestPluginPrompt({ kind: "permission", plugin }))) return false;
    savePluginStates(
      withPluginState(pluginStates(), plugin.identifier, { enabled: true, granted: true }),
    );
    return true;
  }
  savePluginStates(withPluginState(states, plugin.identifier, { enabled: true }));
  return true;
}

export function pluginEnabled(plugin: Pick<PluginEntry, "identifier" | "builtin">): boolean {
  return isPluginEnabled(pluginStates(), plugin.identifier, plugin.builtin);
}

async function pickFolder(): Promise<string | null> {
  if (isMock) {
    const path = window.prompt(
      "Install from Folder — plugin folder path",
      "/Users/me/Downloads/row-counter",
    );
    return path?.trim() || null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Install Plugin from Folder",
    directory: true,
    multiple: false,
  });
  return typeof picked === "string" ? picked : null;
}

/** Identifiers name one plugin whatever their case (the backend finds a plugin that way). */
const sameIdentifier = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** "Name" or, when two plugins share a name, "Name (author)". */
function pluginLabel(name: string, author: string, other: { name: string }): string {
  return name === other.name && author ? `${name} (${author})` : name;
}

/**
 * Install from Folder…: copies a plugin folder in, then asks to enable it (its permissions).
 * An update of an installed plugin (same identifier, author and name) keeps its state. Another
 * plugin that merely uses the same identifier replaces it without inheriting the grant: its state
 * is dropped, so it asks for permission (and for links) like a first install.
 */
export async function installPluginFromFolder(): Promise<void> {
  const path = await pickFolder();
  if (!path) return;
  try {
    // What is installed now (read again: the Plugins folder can change outside the app), to tell an
    // update from another plugin with the same identifier. A list that could not be read is no
    // evidence of what was installed.
    await refreshPlugins();
    const { list: before, error } = usePluginsStore.getState();
    const installed = await ipc.plugins.install(path);
    const m = installed.manifest;
    const old = (error === null ? before : null)?.plugins.find((p) =>
      sameIdentifier(p.manifest.identifier, m.identifier),
    )?.manifest;
    // Only the same plugin (author and name) inherits what was granted. A stored state with no
    // installed plugin to compare to (its folder was deleted or moved, or its manifest is broken
    // and listed as invalid) is a stale grant: the plugin now installed is a stranger to it.
    const replaced = old
      ? old.author !== m.author || old.name !== m.name
      : Object.keys(pluginStates()).some((id) => sameIdentifier(id, m.identifier));
    if (replaced) {
      let states = pluginStates();
      for (const id of Object.keys(states)) {
        if (sameIdentifier(id, m.identifier)) states = withoutPlugin(states, id);
      }
      savePluginStates(states);
    }
    await refreshPlugins();
    toast.success(
      `Installed ${m.name} ${m.version}`,
      old && replaced
        ? {
            description: `Replaces ${pluginLabel(old.name, old.author, m)} by ${pluginLabel(m.name, m.author, old)}: a different plugin, so it needs your permission again.`,
          }
        : undefined,
    );
    const entry: PluginEntry = {
      identifier: m.identifier,
      name: m.name,
      version: m.version,
      author: m.author,
      description: m.description,
      builtin: false,
      commands: m.commands,
      folder: installed.folder,
    };
    if (!pluginEnabled(entry)) await setPluginEnabled(entry, true);
  } catch (e) {
    toast.error("Could not install the plugin", { description: errorMessage(e) });
  }
}

/** Remove: deletes the plugin's folder and forgets its state. */
export async function removePlugin(plugin: PluginEntry): Promise<void> {
  if (plugin.builtin) return;
  try {
    await ipc.plugins.remove(plugin.identifier);
    savePluginStates(withoutPlugin(pluginStates(), plugin.identifier));
    toast.success(`Removed ${plugin.name}`);
  } catch (e) {
    toast.error(`Could not remove ${plugin.name}`, { description: errorMessage(e) });
  }
  await refreshPlugins();
}

export async function revealPluginsFolder(): Promise<void> {
  try {
    await ipc.plugins.revealFolder();
  } catch (e) {
    toast.error("Could not open the Plugins folder", { description: errorMessage(e) });
  }
}

/** Dummy Data ▸ Generate Dummy Data…: the dialog for `request`'s table. */
export function openDummyData(request: DummyDataRequest): void {
  usePluginsStore.setState({ dummy: request });
}

export function closeDummyData(): void {
  usePluginsStore.setState({ dummy: null });
}

/** Test-only: back to an empty store. */
export function resetPluginsStoreForTests(): void {
  usePluginsStore.setState({
    list: null,
    loading: false,
    error: null,
    managerOpen: false,
    prompts: [],
    dummy: null,
  });
}
