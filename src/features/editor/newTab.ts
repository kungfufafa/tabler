import { coreDialect } from "@/features/drivers/catalog";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import { seedEditor } from "./editorStates";

/**
 * "Command + T should open: SQL Query Editor tab / Empty tab" (TablePlus). A SQL Query Editor tab
 * opens with the connection's most recent script (the last query tab of its profile that was
 * closed) when none of its query tabs is open; an empty tab, or Auto Save SQL Query Tab ▸ "Do not
 * save the SQL query", always starts blank.
 */

/** localStorage: profile id → the text of its last closed query tab. */
export const LAST_SCRIPT_KEY = "tabler.editor.lastScript";
/** Longest script remembered (bigger ones are left out rather than cut). */
const SCRIPT_MAX = 256 * 1024;
const PROFILES_MAX = 50;

let memory: Record<string, string> | null = null;

function load(): Record<string, string> {
  if (memory) return memory;
  try {
    const raw = globalThis.localStorage?.getItem(LAST_SCRIPT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    memory =
      parsed && typeof parsed === "object" && !Array.isArray(parsed)
        ? Object.fromEntries(
            Object.entries(parsed as Record<string, unknown>).filter(
              (e): e is [string, string] => typeof e[1] === "string",
            ),
          )
        : {};
  } catch {
    memory = {};
  }
  return memory;
}

function store(map: Record<string, string>): void {
  memory = map;
  try {
    globalThis.localStorage?.setItem(LAST_SCRIPT_KEY, JSON.stringify(map));
  } catch {
    // Not persisted: remembered for this session only.
  }
}

/** Remembers `text` as the profile's most recent script (blank text forgets nothing). */
export function rememberLastScript(profileId: string, text: string): void {
  if (!text.trim() || text.length > SCRIPT_MAX) return;
  const { [profileId]: _old, ...rest } = load();
  const keys = Object.keys(rest);
  // Oldest first (insertion order): keep the newest ones.
  const kept = Object.fromEntries(
    keys.slice(Math.max(0, keys.length - PROFILES_MAX + 1)).map((k) => [k, rest[k]!]),
  );
  store({ ...kept, [profileId]: text });
}

export function lastScript(profileId: string): string | null {
  return load()[profileId] ?? null;
}

/** What a new query tab of the connection starts with: its last script, or nothing. */
export function newTabScript(connectionId: string): string | null {
  const prefs = getPrefs();
  if (prefs.newTabOpens === "empty" || prefs.autoSaveQueryTabs === "none") return null;
  const profileId = useConnectionsStore.getState().byId[connectionId]?.info.profileId;
  if (!profileId) return null;
  const openQueryTab = useTabsStore
    .getState()
    .tabs.some((t) => t.kind === "query" && t.connectionId === connectionId);
  return openQueryTab ? null : lastScript(profileId);
}

/** ⌘T / ⌘E: a new query tab of the connection, as "Command + T should open" says. */
export function openNewQueryTab(connectionId: string): string {
  const script = newTabScript(connectionId);
  const id = useTabsStore.getState().newQuery(connectionId);
  const driver = useConnectionsStore.getState().byId[connectionId]?.info.driver;
  // A connection without a core dialect has no SQL editor to start the script in.
  const dialect = driver ? coreDialect(driver) : "postgres";
  if (script && dialect) seedEditor(id, script, { dialect });
  return id;
}

/** Test helper. */
export function resetLastScriptsForTests(): void {
  memory = null;
  try {
    globalThis.localStorage?.removeItem(LAST_SCRIPT_KEY);
  } catch {
    // ignore
  }
}
