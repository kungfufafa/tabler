import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { openConnectionOf } from "@/features/connections/openProfile";
import {
  askConnectionPassword,
  isPromptCancelled,
  PasswordPromptCancelled,
  requestPassword,
} from "@/features/connections/passwordPromptStore";
import {
  connectionFormSchema,
  formToProfileInput,
  urlToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import { clearSshPassword, presetSshPassword } from "@/features/connections/sshPromptStore";
import type { UrlFilter, UrlTarget } from "@/features/connections/url";
import {
  hostKeyDeclined,
  profileKeys,
  withHostKeyTrust,
} from "@/features/connections/useConnections";
import { notAvailableFor } from "@/features/drivers/catalog";
import { dialectOf } from "@/features/editor/controller";
import { baseName, readSqlFileGuarded } from "@/features/editor/files";
import { openFileInNewQueryTab } from "@/features/editor/openSql";
import { usePaletteStore } from "@/features/palette/store";
import { filterMemoryKey } from "@/features/table-tab/filterMemory";
import { recallColumnLayout } from "@/features/table-tab/tableViewMemory";
import { handoffTab } from "@/features/tabs/windowHandoff";
import { profileLazyLoad, setLazyLoad, supportsLazyLoad } from "@/features/sidebar/lazyLoad";
import { ipc, toIpcError, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs } from "@/stores/prefs";
import { useQueryTabsStore } from "@/stores/queryTabs";
import { newFilterRow, useTableTabsStore, type FilterRowSpec } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { filterRowOf, type OpenPlan } from "./route";
import {
  askApplyFilter,
  forgetTemporary,
  rememberTemporary,
  setPendingFile,
  useDeepLinkStore,
  type TemporaryConnection,
} from "./store";

/**
 * Carries out the plans of `route.ts`: connects (a saved or a temporary connection, the URL's
 * passwords used for that connect only), opens the table with the link's filter, or opens an
 * `.sql` file in a query tab.
 */

const errorText = (e: unknown) => toIpcError(e).message;

async function profiles(qc: QueryClient): Promise<ConnectionProfile[]> {
  return qc.ensureQueryData({ queryKey: profileKeys.all, queryFn: ipc.profiles.list });
}

/** SQLite's answer to an encrypted (SQLCipher) file opened without its key (drivers/sqlite). */
const ENCRYPTED_DATABASE = /^This database is encrypted/;
/** …and to a key that does not open it (drivers/sqlite/cipher.rs). */
const WRONG_KEY = /^Wrong encryption key/;

export const isEncryptedDatabaseError = (e: unknown): boolean =>
  ENCRYPTED_DATABASE.test(errorText(e).trim());
const isWrongKeyError = (e: unknown): boolean => WRONG_KEY.test(errorText(e).trim());

/**
 * Opens `profileId`: its open connection when there is one, else a connect with the URL's
 * password (for this session only; an "Ask every time" profile without one asks). The URL's SSH
 * password answers the connect's SSH password question. An encrypted SQLite file (a Finder
 * double-click on a database nobody saved a key for) asks for its key and tries again until it
 * opens or the user cancels; that key is the connect's password: used for this connection only,
 * never stored. A profile whose key mode is "No password" or "Command" is never asked (the backend
 * ignores a typed key there): its connect error is shown as it is. Resolves with the connection id, or null when the connect failed (the user was
 * told) or was cancelled.
 */
export async function connectProfile(
  qc: QueryClient,
  profileId: string,
  password: string | null,
  sshPassword: string | null,
): Promise<string | null> {
  const open = openConnectionOf(profileId);
  if (open) {
    useConnectionsStore.getState().setActive(open);
    useUiStore.getState().setView("workspace");
    return open;
  }
  const palette = usePaletteStore.getState();
  if (!palette.beginConnect(profileId)) return null;
  const profile = (await profiles(qc)).find((p) => p.id === profileId);
  const name = profile?.name ?? null;
  try {
    if (sshPassword) presetSshPassword(profileId, sshPassword);
    const mode = profile?.options.passwordMode ?? "keychain";
    // "No password" and "Command" ignore any typed password (connect_password in the backend),
    // so asking for a key would never open the file: the connect's own error says what to fix.
    const typable = mode !== "none" && mode !== "command";
    let typed: string | null = typable ? password : null;
    // An "Ask every time" profile already asked for the key as its password: no second prompt.
    const asked = typed === null && mode === "ask" && profile !== undefined;
    if (asked) typed = await askConnectionPassword(profile.name, "connect");
    // The key prompt of an encrypted SQLite file: asked after the connect refused it.
    let keyPrompted = false;
    let info: ConnectionInfo;
    for (;;) {
      try {
        info = await withHostKeyTrust(profileId, "connect", name, () =>
          ipc.connections.connect(profileId, typed),
        );
        break;
      } catch (e) {
        const retry = isEncryptedDatabaseError(e) || (keyPrompted && isWrongKeyError(e));
        if (!profile || profile.driver !== "sqlite" || !typable || asked || !retry) throw e;
        const key = await requestPassword({
          title: "Encryption key",
          description: `“${baseName(profile.sqlitePath ?? name ?? "")}” is encrypted (SQLCipher). Enter its key to open it; the key is not stored.`,
          confirmLabel: "Open",
          error: keyPrompted
            ? isWrongKeyError(e)
              ? errorText(e)
              : "Enter the encryption key."
            : null,
        });
        if (key === null) throw new PasswordPromptCancelled();
        typed = key;
        keyPrompted = true;
      }
    }
    useConnectionsStore.getState().add(info);
    useUiStore.getState().setView("workspace");
    void qc.invalidateQueries({ queryKey: profileKeys.all });
    return info.connectionId;
  } catch (e) {
    if (!(hostKeyDeclined(e) || isPromptCancelled(e) || toIpcError(e).code === "CANCELLED"))
      toast.error(`Could not connect${name ? ` to ${name}` : ""}`, { description: errorText(e) });
    return null;
  } finally {
    clearSshPassword(profileId);
    usePaletteStore.getState().endConnect(profileId);
  }
}

/** The filter-bar row a link's filter makes (`enabled` as the caller decides). */
function rowOf(seed: Exclude<ReturnType<typeof filterRowOf>, { error: string }>, enabled: boolean) {
  return { ...newFilterRow(seed.column, seed.op, seed.value), value2: seed.value2, enabled };
}

/**
 * The link's filter as filter-bar rows, or null (nothing to filter, or the link's filter was
 * refused: the user is told). `trusted`: a raw SQL condition may apply at once (the connection is
 * read-only); otherwise it comes back unchecked.
 */
function linkFilterRows(filter: UrlFilter | null, trusted: boolean): FilterRowSpec[] | null {
  if (!filter) return null;
  const seed = filterRowOf(filter);
  if ("error" in seed) {
    toast.error("The link's filter was not applied", { description: seed.error });
    return null;
  }
  return [rowOf(seed, seed.enabled || trusted)];
}

/**
 * Opens the link's schema / table (or view) and applies its row filter. The table's remembered
 * column layout (positions, widths, hidden columns) goes along: the filter seeds the tab's state
 * before the tab mounts, so the tab's own recall would find the state made and skip it.
 *
 * A column condition applies at once (its value is a bound parameter). A raw SQL condition
 * (`condition` / `raw` / `query`) is SQL of the link's own: on a read-only connection, which
 * cannot change anything, it applies at once too; otherwise the table opens with it unchecked in
 * the filter bar and a dialog shows the SQL and asks (`confirmRawFilter`): "Apply filter" checks
 * it and reloads the table from the first page, "Not now" leaves it unchecked.
 */
export function navigate(connectionId: string, target: UrlTarget | null): void {
  if (!target) return;
  if (target.schema) useConnectionsStore.getState().setCurrentSchema(connectionId, target.schema);
  if (!target.table) return;
  const tabId = useTabsStore
    .getState()
    .openTable(connectionId, { schema: target.schema, name: target.table }, "table");
  if (!target.filter) return;
  const conn = useConnectionsStore.getState().byId[connectionId];
  const rows = linkFilterRows(target.filter, conn?.info.readOnly === true);
  if (!rows) return;
  const layout = recallColumnLayout(
    conn
      ? filterMemoryKey(conn.info.profileId, conn.info.currentDatabase, {
          schema: target.schema,
          name: target.table,
        })
      : null,
  );
  useTableTabsStore.getState().update(tabId, {
    filters: rows,
    filtersVisible: true,
    offset: 0,
    ...(layout ? { columnState: layout } : {}),
  });
  const waiting = rows.find((r) => !r.enabled);
  if (waiting) void confirmRawFilter(connectionId, tabId, waiting, target);
}

/** What `confirmRawFilter` tells the user when the condition stays unchecked. */
function unchecked(): void {
  toast.info("The link's SQL condition is in the filter bar", {
    description: "Check it to apply it: Tabler does not run a link's own SQL by itself.",
  });
}

/**
 * Asks whether to apply a link's raw SQL condition (the dialog shows the table, the connection and
 * the SQL). Yes: the row is checked and the table reloads from the first page, unless the tab, the
 * row or its text changed meanwhile. No: it stays unchecked, with a note.
 */
async function confirmRawFilter(
  connectionId: string,
  tabId: string,
  row: FilterRowSpec,
  target: UrlTarget,
): Promise<void> {
  const conn = useConnectionsStore.getState().byId[connectionId];
  const apply = await askApplyFilter({
    connection: conn?.info.name ?? "the connection",
    table: target.schema ? `${target.schema}.${target.table}` : (target.table ?? ""),
    sql: row.value,
  });
  if (!apply) {
    unchecked();
    return;
  }
  if (!useTabsStore.getState().tabs.some((t) => t.id === tabId)) return;
  const current = useTableTabsStore.getState().slices[tabId]?.filters.find((f) => f.id === row.id);
  if (!current || current.value !== row.value) return;
  useTableTabsStore.getState().update(tabId, (s) => ({
    filters: s.filters.map((f) => (f.id === row.id ? { ...f, enabled: true } : f)),
    offset: 0,
  }));
}

/** The form values of a temporary connection: nothing it was given is stored. */
export function temporaryForm(
  values: Partial<ConnectionFormValues>,
  password: string | null,
  sshPassword: string | null,
): ConnectionFormValues {
  const prefs = getPrefs();
  const form = urlToForm({ values, password: null }, prefs.connectTimeout, prefs.defaultSafeMode);
  form.password = "";
  form.sshPassword = "";
  // The URL's passwords are for this session: "Ask every time" keeps them out of the Keychain.
  if (password !== null) form.passwordMode = "ask";
  if (form.sshEnabled && form.sshAuth === "password")
    form.sshPasswordMode = sshPassword !== null ? "ask" : form.sshPasswordMode;
  return form;
}

/**
 * Creates the temporary profile of a URL that matches no saved connection: saved for the
 * session, marked temporary (removed when its connection closes, or at the next start), with
 * its passwords kept in memory only.
 */
export async function createTemporary(
  qc: QueryClient,
  values: Partial<ConnectionFormValues>,
  password: string | null,
  sshPassword: string | null,
  lazyLoad: boolean | null = null,
): Promise<ConnectionProfile | null> {
  const form = temporaryForm(values, password, sshPassword);
  const checked = connectionFormSchema.safeParse(form);
  if (!checked.success) {
    toast.error("Could not open the connection", {
      description: checked.error.issues[0]?.message ?? "The URL is incomplete",
    });
    return null;
  }
  try {
    const input = formToProfileInput(checked.data, null, crypto.randomUUID());
    // The link's `lazyload` (the form has no field for it).
    if (lazyLoad !== null && checked.data.driver !== "sqlite")
      input.profile.options.lazyLoad = lazyLoad;
    const saved = await ipc.profiles.save({ ...input, password: null, sshPassword: null });
    await ipc.deeplink.markTemporary(saved.id, true);
    rememberTemporary({
      profileId: saved.id,
      name: saved.name,
      password,
      sshPassword,
      seenConnected: false,
    });
    qc.setQueryData<ConnectionProfile[]>(profileKeys.all, (list) =>
      list ? [...list.filter((p) => p.id !== saved.id), saved] : [saved],
    );
    void qc.invalidateQueries({ queryKey: profileKeys.all });
    return saved;
  } catch (e) {
    toast.error("Could not open the connection", { description: errorText(e) });
    return null;
  }
}

/** Deletes a temporary profile (its connection closed, or its connect failed). */
export async function dropTemporary(qc: QueryClient, profileId: string): Promise<void> {
  forgetTemporary(profileId);
  try {
    await ipc.profiles.delete(profileId);
    await ipc.deeplink.markTemporary(profileId, false);
  } catch (e) {
    console.warn("[deeplink] could not remove a temporary connection", e);
  }
  void qc.invalidateQueries({ queryKey: profileKeys.all });
}

/**
 * "Save" on a temporary connection: it becomes a saved one, the URL's passwords going to the
 * Keychain.
 */
export async function saveTemporary(qc: QueryClient, profileId: string): Promise<boolean> {
  const t: TemporaryConnection | undefined = useDeepLinkStore.getState().temporary[profileId];
  try {
    const p = await ipc.profiles.get(profileId);
    const profile: ConnectionProfile = {
      ...p,
      options: {
        ...p.options,
        passwordMode: t?.password != null ? "keychain" : p.options.passwordMode,
      },
      ssh: p.ssh && t?.sshPassword != null ? { ...p.ssh, passwordMode: "keychain" } : p.ssh,
    };
    const saved = await ipc.profiles.save({
      profile,
      password: t?.password ?? null,
      sshPassword: t?.sshPassword ?? null,
      sshPassphrase: null,
      clearPassword: false,
    });
    await ipc.deeplink.markTemporary(profileId, false);
    forgetTemporary(profileId);
    qc.setQueryData<ConnectionProfile[]>(profileKeys.all, (list) =>
      list?.map((x) => (x.id === saved.id ? saved : x)),
    );
    void qc.invalidateQueries({ queryKey: profileKeys.all });
    toast.success(`Saved “${saved.name}”`);
    return true;
  } catch (e) {
    toast.error("Could not save the connection", { description: errorText(e) });
    return false;
  }
}

/** Opens an `.sql` file in a query tab of the connection (its tab when it is open already). */
export async function openSqlFile(connectionId: string, path: string): Promise<void> {
  const already = Object.entries(useQueryTabsStore.getState().slices).find(
    ([id, s]) => s.filePath === path && useTabsStore.getState().tabs.some((t) => t.id === id),
  );
  if (already) {
    useTabsStore.getState().activate(already[0]);
    useUiStore.getState().setView("workspace");
    return;
  }
  let file: Awaited<ReturnType<typeof readSqlFileGuarded>>;
  try {
    // A big file asks first; the head of one opens as a copy that is not linked to the file.
    file = await readSqlFileGuarded(path, { connectionId });
  } catch (e) {
    toast.error("Could not open the file", { description: errorText(e) });
    return;
  }
  // Cancelled, or handed to the SQL import wizard.
  if (!file) return;
  const conn = useConnectionsStore.getState().byId[connectionId];
  const dialect = dialectOf(conn?.info.driver);
  if (!dialect) {
    toast.error("Could not open the file", {
      description: conn ? notAvailableFor(conn.info.driver, "The SQL editor") : undefined,
    });
    return;
  }
  openFileInNewQueryTab(connectionId, path, file, dialect);
  useConnectionsStore.getState().setActive(connectionId);
  useUiStore.getState().setView("workspace");
}

/** How long an `.sql` file waits for the user to pick a connection. */
export const PENDING_FILE_TTL_MS = 5 * 60_000;

/** Carries out one plan. */
export async function runPlan(qc: QueryClient, plan: OpenPlan): Promise<void> {
  switch (plan.kind) {
    case "error":
      toast.error("Could not open the link", { description: plan.message });
      return;
    case "sqlFile":
      if (plan.connectionId) {
        await openSqlFile(plan.connectionId, plan.path);
        return;
      }
      // No connection yet: the file opens in the one the user picks (palette, ⇧⌘K).
      setPendingFile({ path: plan.path, since: Date.now() });
      toast.info(`Choose a connection for “${baseName(plan.path)}”`);
      usePaletteStore.getState().toggle("connections");
      return;
    case "profile":
    case "temporary": {
      let profileId: string;
      let created = false;
      if (plan.kind === "profile") {
        profileId = plan.profileId;
        // The link's `lazyload` is the sidebar setting of that saved connection.
        if (plan.lazyLoad !== null) await applyLazyLoad(qc, profileId, plan.lazyLoad);
      } else {
        const saved = await createTemporary(
          qc,
          plan.values,
          plan.password,
          plan.sshPassword,
          plan.lazyLoad,
        );
        if (!saved) return;
        profileId = saved.id;
        created = true;
      }
      if (plan.isolated) {
        // windowMode=isolated: the connection opens in a window of its own (it connects there);
        // the table and the link's row filter travel with the handoff.
        const readOnly =
          (await profiles(qc)).find((p) => p.id === profileId)?.options.readOnly === true;
        const filters = linkFilterRows(plan.target?.filter ?? null, readOnly);
        const tab = plan.target?.table
          ? handoffTab(
              {
                id: "deeplink",
                kind: "table",
                connectionId: "",
                table: { schema: plan.target.schema, name: plan.target.table },
                title: plan.target.table,
              },
              undefined,
              filters ? { filters, filtersVisible: true } : null,
            )
          : null;
        try {
          await ipc.windows.open({
            profileId,
            database: null,
            schema: plan.target?.schema ?? null,
            tab,
          });
        } catch (e) {
          toast.error("Could not open a new window", { description: errorText(e) });
          return;
        }
        // A raw condition arrives unchecked there (the new window cannot ask on this one's behalf).
        if (filters?.some((f) => !f.enabled))
          toast.info("The link's SQL condition is in the new window's filter bar", {
            description:
              "Check it there to apply it: Tabler does not run a link's own SQL by itself.",
          });
        return;
      }
      const connectionId = await connectProfile(qc, profileId, plan.password, plan.sshPassword);
      if (!connectionId) {
        if (created) await dropTemporary(qc, profileId);
        return;
      }
      navigate(connectionId, plan.target);
      if (created) {
        const name = (await profiles(qc)).find((p) => p.id === profileId)?.name ?? "the connection";
        toast(`Opened “${name}” from a link`, {
          description: "It is not saved: it goes away when you disconnect.",
          duration: 15_000,
          action: { label: "Save", onClick: () => void saveTemporary(qc, profileId) },
        });
      }
      return;
    }
  }
}

/**
 * A link's `lazyload` for a saved connection: the sidebar's per-connection lazy-load setting (the
 * one the sidebar's own toggle keeps), when the connection's driver has it. A temporary
 * connection takes it as its option instead (`createTemporary`).
 */
export async function applyLazyLoad(
  qc: QueryClient,
  profileId: string,
  value: boolean,
): Promise<void> {
  const profile = (await profiles(qc)).find((p) => p.id === profileId);
  if (!profile || !supportsLazyLoad(profile.driver)) return;
  setLazyLoad(profileId, value, profileLazyLoad(profile));
}
