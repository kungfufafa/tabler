import { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { profileKeys } from "@/features/connections/useConnections";
import {
  enqueueSshPrompt,
  presetSshPassword,
  useSshPromptStore,
} from "@/features/connections/sshPromptStore";
import { RAW_SQL_COLUMN } from "@/features/table-tab/filters";
import { filterMemoryKey } from "@/features/table-tab/filterMemory";
import {
  rememberColumnLayout,
  resetTableViewMemoryForTests,
} from "@/features/table-tab/tableViewMemory";
import { resetLazyLoad, useLazyStore } from "@/features/sidebar/lazyLoad";
import {
  ipc,
  type ConnectionInfo,
  type ConnectionProfile,
  type SshPromptRequest,
  type WindowHandoff,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import {
  connectProfile,
  createTemporary,
  dropTemporary,
  navigate,
  runPlan,
  saveTemporary,
  temporaryForm,
} from "./open";
import { planOpen, type RouteContext } from "./route";
import { answerFilterRequest, useDeepLinkStore } from "./store";

const keyPrompt = vi.hoisted(() => vi.fn());
vi.mock("@/features/connections/passwordPromptStore", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/connections/passwordPromptStore")>()),
  requestPassword: keyPrompt,
}));

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

beforeEach(() => {
  useTabsStore.setState({ tabs: [], activeId: null });
  useTableTabsStore.setState({ slices: {} });
  useDeepLinkStore.setState({ temporary: {}, pendingFile: null, filterRequests: [] });
  useSshPromptStore.setState({ queue: [] });
  keyPrompt.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("temporary connections", () => {
  it("keep the URL's passwords out of the Keychain", () => {
    const form = temporaryForm(
      {
        driver: "postgres",
        host: "db",
        user: "u",
        name: "db",
        sshEnabled: true,
        sshHost: "bastion",
        sshAuth: "password",
      },
      "dbpw",
      "sshpw",
    );
    expect(form).toMatchObject({
      passwordMode: "ask",
      sshPasswordMode: "ask",
      password: "",
      sshPassword: "",
    });
    // No password in the URL: the profile's defaults.
    expect(
      temporaryForm({ driver: "mysql", host: "h", user: "u", name: "h" }, null, null),
    ).toMatchObject({ passwordMode: "keychain" });
  });

  it("are saved marked temporary, then saved for good or removed", async () => {
    const qc = new QueryClient();
    const created = await createTemporary(
      qc,
      { driver: "sqlite", sqlitePath: "/tmp/linked.sqlite", name: "linked.sqlite" },
      null,
      null,
    );
    expect(created).not.toBeNull();
    const id = created!.id;
    expect(await ipc.deeplink.temporaryProfiles()).toContain(id);
    expect(qc.getQueryData<ConnectionProfile[]>(profileKeys.all)?.some((p) => p.id === id)).toBe(
      true,
    );
    expect(useDeepLinkStore.getState().temporary[id]).toMatchObject({ name: "linked.sqlite" });

    expect(await saveTemporary(qc, id)).toBe(true);
    expect(await ipc.deeplink.temporaryProfiles()).not.toContain(id);
    expect(useDeepLinkStore.getState().temporary[id]).toBeUndefined();

    await dropTemporary(qc, id);
    expect((await ipc.profiles.list()).some((p) => p.id === id)).toBe(false);
  });

  it("apply the link's driverVersion (MySQL Ver.) and lazyload", async () => {
    const qc = new QueryClient();
    const plan = planOpen(
      { kind: "url", url: "mysql://root@db.example.com/shop?driverVersion=1&lazyload=true" },
      { profiles: [], temporary: new Set(), activeConnectionId: null },
    );
    expect(plan).toMatchObject({
      kind: "temporary",
      lazyLoad: true,
      values: { mysqlVersion: "mariadb" },
    });
    if (plan.kind !== "temporary") return;
    const created = await createTemporary(qc, plan.values, null, null, plan.lazyLoad);
    expect(created?.options).toMatchObject({ mysqlVersion: "mariadb", lazyLoad: true });
    const stored = await ipc.profiles.get(created!.id);
    expect(stored.options).toMatchObject({ mysqlVersion: "mariadb", lazyLoad: true });
    await dropTemporary(qc, created!.id);

    // Without them: the defaults (no Ver., lazy load left to the preference).
    const plain = await createTemporary(
      qc,
      { driver: "mysql", host: "db.example.com", user: "root", name: "plain" },
      null,
      null,
    );
    expect(plain?.options.mysqlVersion ?? null).toBeNull();
    expect(plain?.options.lazyLoad ?? null).toBeNull();
    await dropTemporary(qc, plain!.id);
    // An explicit lazyload=false is kept too.
    const off = await createTemporary(
      qc,
      { driver: "postgres", host: "db.example.com", user: "u", name: "off" },
      null,
      null,
      false,
    );
    expect(off?.options.lazyLoad).toBe(false);
    await dropTemporary(qc, off!.id);
  });

  it("refuse an incomplete URL", async () => {
    const qc = new QueryClient();
    expect(
      await createTemporary(qc, { driver: "postgres", host: "", name: "x" }, null, null),
    ).toBeNull();
  });
});

describe("navigate", () => {
  it("opens the table with the link's column filter applied", () => {
    navigate("c1", {
      schema: "public",
      table: "orders",
      filter: { kind: "condition", column: "id", operator: ">=", value: "10" },
    });
    const tab = useTabsStore.getState().tabs[0]!;
    expect(tab).toMatchObject({ kind: "table", connectionId: "c1", table: { name: "orders" } });
    const slice = useTableTabsStore.getState().slices[tab.id]!;
    expect(slice.filtersVisible).toBe(true);
    expect(slice.filters).toMatchObject([{ column: "id", op: "ge", value: "10", enabled: true }]);
  });

  it("puts a raw condition in the bar unchecked until it is confirmed", () => {
    navigate("c1", { schema: null, table: "t", filter: { kind: "raw", sql: "x > 1" } });
    const tab = useTabsStore.getState().tabs[0]!;
    expect(useTableTabsStore.getState().slices[tab.id]!.filters).toMatchObject([
      { column: RAW_SQL_COLUMN, value: "x > 1", enabled: false },
    ]);
    expect(useDeepLinkStore.getState().filterRequests).toHaveLength(1);
  });

  it("never turns a link's column into a raw SQL row (column=__raw_sql__)", () => {
    navigate("c1", {
      schema: null,
      table: "t",
      filter: {
        kind: "condition",
        column: RAW_SQL_COLUMN,
        operator: "=",
        value: "1=1) AND (SELECT pg_sleep(30)) IS NOT NULL AND (1=1",
      },
    });
    const tab = useTabsStore.getState().tabs[0]!;
    // The table opens, but no filter row (enabled or not) came from the link.
    const filters = useTableTabsStore.getState().slices[tab.id]?.filters ?? [];
    expect(filters.filter((f) => f.column === RAW_SQL_COLUMN)).toEqual([]);
    expect(filters.some((f) => f.enabled)).toBe(false);
  });

  it("brings the table's remembered column layout along", () => {
    resetTableViewMemoryForTests();
    useConnectionsStore.getState().add({
      connectionId: "c1",
      profileId: "p-local-pg",
      name: "Local Postgres",
      color: null,
      driver: "postgres",
      serverVersion: "17",
      currentDatabase: "tabler_dev",
      currentUser: "me",
      tunnel: null,
      readOnly: false,
    });
    const layout = { widths: { id: 120 }, order: ["name", "id"], hidden: ["secret"] };
    rememberColumnLayout(
      filterMemoryKey("p-local-pg", "tabler_dev", { schema: "public", name: "orders" }),
      layout,
    );
    const filter = { kind: "condition", column: "id", operator: "=", value: "1" } as const;
    navigate("c1", { schema: "public", table: "orders", filter });
    const [tab] = useTabsStore.getState().tabs;
    expect(useTableTabsStore.getState().slices[tab!.id]!.columnState).toEqual(layout);

    // A table without a remembered layout keeps the default one.
    useTabsStore.setState({ tabs: [], activeId: null });
    navigate("c1", { schema: "public", table: "users", filter });
    const users = useTabsStore.getState().tabs[0]!;
    expect(useTableTabsStore.getState().slices[users.id]!.columnState).toEqual({
      widths: {},
      order: [],
      hidden: [],
    });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    resetTableViewMemoryForTests();
  });

  it("does nothing without a table", () => {
    navigate("c1", { schema: "s", table: null, filter: null });
    expect(useTabsStore.getState().tabs).toHaveLength(0);
  });
});

const info = (over: Partial<ConnectionInfo> = {}): ConnectionInfo => ({
  connectionId: "c1",
  profileId: "p-local-pg",
  name: "Local Postgres",
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: "tabler_dev",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
  ...over,
});

describe("a link's raw SQL condition (TablePlus b570)", () => {
  const link = {
    schema: "public",
    table: "orders",
    filter: { kind: "raw", sql: "total > 100" },
  } as const;
  const rows = () => {
    const tab = useTabsStore.getState().tabs[0]!;
    return useTableTabsStore.getState().slices[tab.id]!;
  };
  const flush = () => new Promise((r) => setTimeout(r, 0));

  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info());
  });
  afterEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  });

  it("asks first, showing the table, the connection and the SQL", () => {
    navigate("c1", link);
    expect(rows().filters).toMatchObject([
      { column: RAW_SQL_COLUMN, value: "total > 100", enabled: false },
    ]);
    expect(rows().filtersVisible).toBe(true);
    expect(useDeepLinkStore.getState().filterRequests).toMatchObject([
      { connection: "Local Postgres", table: "public.orders", sql: "total > 100" },
    ]);
  });

  it("applies it from the first page once the user says Apply filter", async () => {
    navigate("c1", link);
    useTableTabsStore.getState().update(useTabsStore.getState().tabs[0]!.id, { offset: 200 });
    answerFilterRequest(useDeepLinkStore.getState().filterRequests[0]!.id, true);
    await flush();
    expect(rows().filters).toMatchObject([{ column: RAW_SQL_COLUMN, enabled: true }]);
    expect(rows().offset).toBe(0);
    expect(useDeepLinkStore.getState().filterRequests).toEqual([]);
  });

  it("keeps it unchecked on Not now, and says so", async () => {
    const info = vi.spyOn(toast, "info").mockReturnValue(1);
    navigate("c1", link);
    answerFilterRequest(useDeepLinkStore.getState().filterRequests[0]!.id, false);
    await flush();
    expect(rows().filters).toMatchObject([{ enabled: false, value: "total > 100" }]);
    expect(info).toHaveBeenCalledWith(
      expect.stringContaining("SQL condition is in the filter bar"),
      expect.anything(),
    );
  });

  it("applies it at once, without asking, on a read-only connection", () => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info({ readOnly: true }));
    navigate("c1", link);
    expect(rows().filters).toMatchObject([
      { column: RAW_SQL_COLUMN, value: "total > 100", enabled: true },
    ]);
    expect(useDeepLinkStore.getState().filterRequests).toEqual([]);
  });

  it("does not touch a row the user edited, or a tab that was closed, meanwhile", async () => {
    navigate("c1", link);
    const tab = useTabsStore.getState().tabs[0]!;
    useTableTabsStore.getState().update(tab.id, (s) => ({
      filters: s.filters.map((f) => ({ ...f, value: "1 = 1" })),
    }));
    answerFilterRequest(useDeepLinkStore.getState().filterRequests[0]!.id, true);
    await flush();
    expect(rows().filters).toMatchObject([{ value: "1 = 1", enabled: false }]);

    useTabsStore.setState({ tabs: [], activeId: null });
    useTableTabsStore.setState({ slices: {} });
    navigate("c1", { ...link, table: "users" });
    useTabsStore.setState({ tabs: [], activeId: null });
    useTableTabsStore.setState({ slices: {} });
    answerFilterRequest(useDeepLinkStore.getState().filterRequests[0]!.id, true);
    await flush();
    expect(useTableTabsStore.getState().slices).toEqual({});
  });

  it("queues several links, one dialog each", async () => {
    navigate("c1", link);
    navigate("c1", { schema: "public", table: "users", filter: { kind: "raw", sql: "id < 5" } });
    expect(useDeepLinkStore.getState().filterRequests.map((r) => r.table)).toEqual([
      "public.orders",
      "public.users",
    ]);
  });

  it("leaves column conditions as they were: applied at once, no dialog", () => {
    navigate("c1", {
      schema: null,
      table: "orders",
      filter: { kind: "condition", column: "id", operator: "=", value: "5" },
    });
    expect(rows().filters).toMatchObject([{ column: "id", op: "eq", value: "5", enabled: true }]);
    expect(useDeepLinkStore.getState().filterRequests).toEqual([]);
  });
});

describe("an encrypted SQLite file (Finder double-click, b336)", () => {
  const ENCRYPTED =
    "This database is encrypted (SQLCipher): enter its encryption key in the connection's settings";
  const WRONG = "Wrong encryption key (or not a SQLCipher database)";
  const refuse = (message: string) => ({ code: "CONNECT_FAILED", message });
  const ctx: RouteContext = { profiles: [], temporary: new Set(), activeConnectionId: null };
  const open = (path: string) => planOpen({ kind: "file", path }, ctx);
  const sqliteInfo = info({ driver: "sqlite", name: "vault.db", profileId: "x" });

  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  });
  afterEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  });

  it("asks for the key and connects again with it", async () => {
    const qc = new QueryClient();
    const connect = vi
      .spyOn(ipc.connections, "connect")
      .mockRejectedValueOnce(refuse(ENCRYPTED))
      .mockResolvedValueOnce(sqliteInfo);
    keyPrompt.mockResolvedValueOnce("s3cret");
    await runPlan(qc, open("/tmp/vault.db"));
    expect(keyPrompt).toHaveBeenCalledTimes(1);
    expect(keyPrompt).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Encryption key",
        confirmLabel: "Open",
        error: null,
        description: expect.stringContaining("vault.db"),
      }),
    );
    expect(connect).toHaveBeenCalledTimes(2);
    // The first connect had no key, the second the typed one.
    expect(connect.mock.calls.map((c) => c[1])).toEqual([null, "s3cret"]);
    expect(useConnectionsStore.getState().order).toEqual(["c1"]);
    // Still a temporary connection: nothing was saved with the key.
    const profile = (await ipc.profiles.list()).find((p) => p.sqlitePath === "/tmp/vault.db");
    expect(profile).toBeDefined();
    expect(Object.values(useDeepLinkStore.getState().temporary)[0]).toMatchObject({
      password: null,
    });
    await dropTemporary(qc, profile!.id);
  });

  it("asks again, with the reason, while the key is wrong", async () => {
    const qc = new QueryClient();
    const connect = vi
      .spyOn(ipc.connections, "connect")
      .mockRejectedValueOnce(refuse(ENCRYPTED))
      .mockRejectedValueOnce(refuse(WRONG))
      .mockResolvedValueOnce(sqliteInfo);
    keyPrompt.mockResolvedValueOnce("nope").mockResolvedValueOnce("s3cret");
    await runPlan(qc, open("/tmp/vault2.db"));
    expect(keyPrompt).toHaveBeenCalledTimes(2);
    expect(keyPrompt.mock.calls[1]![0]).toMatchObject({ error: WRONG });
    expect(connect.mock.calls.map((c) => c[1])).toEqual([null, "nope", "s3cret"]);
    expect(useConnectionsStore.getState().order).toEqual(["c1"]);
    const profile = (await ipc.profiles.list()).find((p) => p.sqlitePath === "/tmp/vault2.db");
    await dropTemporary(qc, profile!.id);
  });

  it("drops the temporary connection when the prompt is cancelled, without an error toast", async () => {
    const qc = new QueryClient();
    const error = vi.spyOn(toast, "error").mockReturnValue(1);
    const connect = vi.spyOn(ipc.connections, "connect").mockRejectedValue(refuse(ENCRYPTED));
    keyPrompt.mockResolvedValueOnce(null);
    await runPlan(qc, open("/tmp/vault3.db"));
    expect(keyPrompt).toHaveBeenCalledTimes(1);
    expect(connect).toHaveBeenCalledTimes(1);
    expect(useConnectionsStore.getState().order).toEqual([]);
    expect(error).not.toHaveBeenCalled();
    expect((await ipc.profiles.list()).some((p) => p.sqlitePath === "/tmp/vault3.db")).toBe(false);
    expect(useDeepLinkStore.getState().temporary).toEqual({});
  });

  it("opens a plain file without asking for anything", async () => {
    const qc = new QueryClient();
    const connect = vi.spyOn(ipc.connections, "connect").mockResolvedValue(sqliteInfo);
    await runPlan(qc, open("/tmp/plain.sqlite"));
    expect(keyPrompt).not.toHaveBeenCalled();
    expect(connect).toHaveBeenCalledTimes(1);
    expect(connect.mock.calls[0]![1]).toBeNull();
    const profile = (await ipc.profiles.list()).find((p) => p.sqlitePath === "/tmp/plain.sqlite");
    await dropTemporary(qc, profile!.id);
  });

  it("does not ask for a key for other connect errors, nor for another driver", async () => {
    const qc = new QueryClient();
    const error = vi.spyOn(toast, "error").mockReturnValue(1);
    vi.spyOn(ipc.connections, "connect").mockRejectedValue(refuse("file is locked"));
    await runPlan(qc, open("/tmp/locked.db"));
    expect(keyPrompt).not.toHaveBeenCalled();
    expect(error).toHaveBeenCalledTimes(1);

    // A PostgreSQL profile that happens to fail with that text is not asked either.
    const pg = await createTemporary(
      qc,
      { driver: "postgres", host: "db.example.com", user: "u", name: "pg" },
      null,
      null,
    );
    vi.spyOn(ipc.connections, "connect").mockRejectedValue(refuse(ENCRYPTED));
    expect(await connectProfile(qc, pg!.id, null, null)).toBeNull();
    expect(keyPrompt).not.toHaveBeenCalled();
    await dropTemporary(qc, pg!.id);
  });

  it("does not ask twice of a saved connection that asks for its password every time", async () => {
    const qc = new QueryClient();
    const saved = await createTemporary(
      qc,
      { driver: "sqlite", sqlitePath: "/tmp/ask.db", name: "ask.db" },
      "typed-in-url",
      null,
    );
    // The URL's password put it in "Ask every time": the prompt of that mode is the key prompt.
    expect(saved!.options.passwordMode).toBe("ask");
    vi.spyOn(ipc.connections, "connect").mockRejectedValue(refuse(WRONG));
    vi.spyOn(toast, "error").mockReturnValue(1);
    expect(await connectProfile(qc, saved!.id, "typed-in-url", null)).toBeNull();
    expect(keyPrompt).not.toHaveBeenCalled();
    await dropTemporary(qc, saved!.id);
  });

  // "No password" and "Command" ignore a typed key in the backend (connect_password): a prompt
  // would come back after every entry and never open the file. The connect error is the answer.
  it.each(["none", "command"] as const)(
    "does not loop on the key prompt of a profile whose key mode is %s: the error is shown",
    async (mode) => {
      const qc = new QueryClient();
      const temp = await createTemporary(
        qc,
        { driver: "sqlite", sqlitePath: `/tmp/mode-${mode}.db`, name: `mode-${mode}.db` },
        null,
        null,
      );
      // The same saved profile with the key mode the form offers ("none"; "command" by hand).
      const saved = await ipc.profiles.save({
        profile: {
          ...temp!,
          options: {
            ...temp!.options,
            passwordMode: mode,
            ...(mode === "command" ? { passwordCommand: "echo wrong" } : {}),
          },
        },
        password: null,
        sshPassword: null,
        sshPassphrase: null,
        clearPassword: false,
      });
      expect(saved.options.passwordMode).toBe(mode);
      qc.setQueryData<ConnectionProfile[]>(profileKeys.all, [saved]);
      const error = vi.spyOn(toast, "error").mockReturnValue(1);
      const connect = vi.spyOn(ipc.connections, "connect").mockRejectedValue(refuse(ENCRYPTED));
      // (A prompt would be cancelled, so a regression fails here instead of looping.)
      keyPrompt.mockResolvedValue(null);
      expect(await connectProfile(qc, saved.id, null, null)).toBeNull();
      // One connect, no prompt, and the original error toast (it says where to set the key).
      expect(keyPrompt).not.toHaveBeenCalled();
      expect(connect).toHaveBeenCalledTimes(1);
      expect(error).toHaveBeenCalledTimes(1);
      expect(error.mock.calls[0]![1]).toMatchObject({ description: ENCRYPTED });
      await dropTemporary(qc, saved.id);
    },
  );

  it("still asks for the key of a saved profile in the keychain mode", async () => {
    const qc = new QueryClient();
    const temp = await createTemporary(
      qc,
      { driver: "sqlite", sqlitePath: "/tmp/keychain.db", name: "keychain.db" },
      null,
      null,
    );
    expect(temp!.options.passwordMode).toBe("keychain");
    const connect = vi
      .spyOn(ipc.connections, "connect")
      .mockRejectedValueOnce(refuse(ENCRYPTED))
      .mockResolvedValueOnce(sqliteInfo);
    keyPrompt.mockResolvedValueOnce("s3cret");
    expect(await connectProfile(qc, temp!.id, null, null)).toBe("c1");
    expect(keyPrompt).toHaveBeenCalledTimes(1);
    expect(connect.mock.calls.map((c) => c[1])).toEqual([null, "s3cret"]);
    await dropTemporary(qc, temp!.id);
  });
});

/** The handoff of the first window a test opened. */
function handed(open: { mock: { calls: unknown[][] } }): WindowHandoff {
  const handoff = open.mock.calls[0]?.[0] as WindowHandoff | null | undefined;
  if (!handoff) throw new Error("no window was opened with something to show");
  return handoff;
}

describe("windowMode=isolated keeps the link's row filter, and lazyload reaches saved connections", () => {
  const url = (query: string) => `postgres://app@db.example.com:6543/shop?${query}`;
  const ctx = (profiles: ConnectionProfile[] = []): RouteContext => ({
    profiles,
    temporary: new Set(),
    activeConnectionId: null,
  });

  beforeEach(() => {
    localStorage.clear();
    resetLazyLoad();
  });

  it("hands the table and its column filter to the new window", async () => {
    const qc = new QueryClient();
    const windowOpen = vi.spyOn(ipc.windows, "open").mockResolvedValue("w1");
    const plan = planOpen(
      {
        kind: "url",
        url: url("windowMode=isolated&schema=public&table=orders&column=id&operation=%3D&value=5"),
      },
      ctx(),
    );
    await runPlan(qc, plan);
    expect(windowOpen).toHaveBeenCalledTimes(1);
    const handoff = handed(windowOpen);
    expect(handoff).toMatchObject({
      schema: "public",
      tab: {
        kind: "table",
        table: { schema: "public", name: "orders" },
        filtersVisible: true,
        filters: [{ column: "id", op: "eq", value: "5", value2: "", enabled: true }],
      },
    });
    await dropTemporary(qc, handoff.profileId!);
  });

  it("hands a raw condition over unchecked, and tells this window", async () => {
    const qc = new QueryClient();
    const windowOpen = vi.spyOn(ipc.windows, "open").mockResolvedValue("w1");
    const info = vi.spyOn(toast, "info").mockReturnValue(1);
    await runPlan(
      qc,
      planOpen(
        { kind: "url", url: url("windowMode=isolated&table=orders&condition=total%20%3E%20100") },
        ctx(),
      ),
    );
    const handoff = handed(windowOpen);
    expect(handoff.tab).toMatchObject({
      filters: [{ column: RAW_SQL_COLUMN, value: "total > 100", enabled: false }],
    });
    expect(info).toHaveBeenCalledWith(
      expect.stringContaining("new window's filter bar"),
      expect.anything(),
    );
    await dropTemporary(qc, handoff.profileId!);
  });

  it("opens a table without a filter as before", async () => {
    const qc = new QueryClient();
    const windowOpen = vi.spyOn(ipc.windows, "open").mockResolvedValue("w1");
    await runPlan(
      qc,
      planOpen({ kind: "url", url: url("windowMode=isolated&table=orders") }, ctx()),
    );
    const handoff = handed(windowOpen);
    expect(handoff.tab).toEqual({
      kind: "table",
      table: { schema: null, name: "orders" },
      view: "table",
      title: "orders",
    });
    await dropTemporary(qc, handoff.profileId!);
  });

  it("applies lazyload to a saved connection as its sidebar setting", async () => {
    const qc = new QueryClient();
    vi.spyOn(ipc.windows, "open").mockResolvedValue("w1");
    const saved = (await createTemporary(
      qc,
      {
        driver: "postgres",
        host: "db.example.com",
        port: "6543",
        user: "app",
        database: "shop",
        name: "shop",
      },
      null,
      null,
    ))!;
    const plan = planOpen(
      { kind: "url", url: url("windowMode=isolated&lazyload=true") },
      ctx([saved]),
    );
    expect(plan).toMatchObject({ kind: "profile", profileId: saved.id, lazyLoad: true });
    await runPlan(qc, plan);
    expect(useLazyStore.getState().overrides[saved.id]).toEqual({ value: true, base: null });

    // lazyload=false is a setting too; a link without it leaves the setting alone.
    await runPlan(
      qc,
      planOpen({ kind: "url", url: url("windowMode=isolated&lazyload=false") }, ctx([saved])),
    );
    expect(useLazyStore.getState().overrides[saved.id]).toEqual({ value: false, base: null });
    await runPlan(qc, planOpen({ kind: "url", url: url("windowMode=isolated") }, ctx([saved])));
    expect(useLazyStore.getState().overrides[saved.id]).toEqual({ value: false, base: null });
    await dropTemporary(qc, saved.id);
  });

  it("leaves SQLite connections alone (their files always load whole)", async () => {
    const qc = new QueryClient();
    vi.spyOn(ipc.windows, "open").mockResolvedValue("w1");
    const saved = (await createTemporary(
      qc,
      { driver: "sqlite", sqlitePath: "/tmp/lazy.sqlite", name: "lazy" },
      null,
      null,
    ))!;
    await runPlan(qc, {
      kind: "profile",
      profileId: saved.id,
      password: null,
      sshPassword: null,
      target: null,
      isolated: true,
      lazyLoad: true,
    });
    expect(useLazyStore.getState().overrides[saved.id]).toBeUndefined();
    await dropTemporary(qc, saved.id);
  });
});

describe("handed-over SSH passwords", () => {
  const request = (over: Partial<SshPromptRequest>): SshPromptRequest => ({
    requestId: "r1",
    profileId: "p1",
    label: "x",
    purpose: "connect",
    user: "ops",
    host: "bastion",
    port: 22,
    name: "SSH password",
    instructions: "",
    prompts: [{ prompt: "Password:", echo: false }],
    kind: "password",
    ...over,
  });

  it("answer the connect's password question once, without the dialog", () => {
    const answer = vi.spyOn(ipc.connections, "answerSshPrompt").mockResolvedValue(undefined);
    presetSshPassword("p1", "sshpw");
    enqueueSshPrompt(request({}));
    expect(answer).toHaveBeenCalledWith("r1", ["sshpw"]);
    expect(useSshPromptStore.getState().queue).toHaveLength(0);
    // Used once: the next question is shown.
    enqueueSshPrompt(request({ requestId: "r2" }));
    expect(useSshPromptStore.getState().queue).toHaveLength(1);
    // A 2FA code is never answered with it.
    presetSshPassword("p1", "sshpw");
    enqueueSshPrompt(
      request({
        requestId: "r3",
        kind: undefined,
        name: "",
        prompts: [{ prompt: "Verification code: ", echo: false }],
      }),
    );
    expect(useSshPromptStore.getState().queue.map((r) => r.requestId)).toEqual(["r2", "r3"]);
    answer.mockRestore();
  });
});
