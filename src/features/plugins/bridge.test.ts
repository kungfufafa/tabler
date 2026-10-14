import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionInfo, QueryEvent, StatementResult } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { handleBridgeCall } from "./bridge";
import { PluginCancelled } from "./execute";
import { createHostServices } from "./host";
import { answerPluginPrompt, resetPluginsStoreForTests, usePluginsStore } from "./store";
import type { PluginInvocation } from "./types";
import { checkPluginUrl, MAX_PLUGIN_URL_LENGTH, resetPluginUrlLimitsForTests } from "./urls";

const mocks = vi.hoisted(() => ({
  guard: vi.fn(async () => true),
  run: vi.fn(),
  clipboard: vi.fn(async () => {}),
  openUrl: vi.fn(async () => {}),
}));
vi.mock("@/features/safety/guard", () => ({ guardExecution: mocks.guard }));
vi.mock("@/features/datagrid/clipboard", () => ({ writeClipboard: mocks.clipboard }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: mocks.openUrl }));
vi.mock("@/lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ipc")>();
  return {
    ...actual,
    ipc: { ...actual.ipc, query: { ...actual.ipc.query, run: mocks.run } },
  };
});

const INFO: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "local",
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};

const inv: PluginInvocation = {
  source: "global",
  connectionId: "c1",
  driver: "postgres",
  item: null,
  clickedRow: null,
  clickedColumn: null,
  selectedRows: [],
  editorTabId: null,
  sql: null,
};

const plugin = { identifier: "com.example.t", name: "Test Plugin" };
const services = () => createHostServices({ plugin, invocation: inv });

function result(index: number, over: Partial<StatementResult> = {}): StatementResult {
  return {
    index,
    sql: "",
    span: { start: 0, end: 0, line: 1 },
    kind: "rows",
    columns: [],
    resultSets: 1,
    rowCount: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: "",
    durationMs: 1,
    error: null,
    notices: [],
    ...over,
  };
}

beforeEach(() => {
  resetPrefsForTests();
  resetPluginsStoreForTests();
  resetPluginUrlLimitsForTests();
  useConnectionsStore.getState().add(INFO);
  mocks.guard.mockReset().mockResolvedValue(true);
  mocks.run.mockReset();
  mocks.clipboard.mockClear();
  mocks.openUrl.mockClear();
});
afterEach(() => {
  vi.useRealTimers();
  useConnectionsStore.getState().remove("c1");
});

describe("the plugin bridge", () => {
  it("runs execute through Safe Mode and the normal query path", async () => {
    mocks.run.mockImplementation(
      async (
        _c: string,
        _q: string,
        _sql: string,
        _o: unknown,
        onEvent: (e: QueryEvent) => void,
      ) => {
        onEvent({
          event: "columns",
          data: {
            index: 1,
            set: 0,
            columns: [{ name: "n", dataType: "int8", kind: "bigInt", nullable: null, table: null }],
          },
        });
        onEvent({
          event: "rows",
          data: { index: 1, set: 0, rows: [[{ t: "bigInt", v: "9007199254740993" }]] },
        });
        return [result(0, { kind: "affected", affected: 2 }), result(1)];
      },
    );
    const out = await handleBridgeCall(services(), "execute", [
      "UPDATE t SET a = 1; SELECT count(*) AS n FROM t",
    ]);
    expect(mocks.guard).toHaveBeenCalledWith("c1", {
      kind: "query",
      statements: ["UPDATE t SET a = 1", "SELECT count(*) AS n FROM t"],
    });
    expect(mocks.run).toHaveBeenCalledWith(
      "c1",
      expect.stringMatching(/^plugin-/),
      "UPDATE t SET a = 1; SELECT count(*) AS n FROM t",
      { stopOnError: true },
      expect.any(Function),
    );
    // Big integers stay exact (as text).
    expect(out).toEqual({ columns: ["n"], rows: [["9007199254740993"]], rowsAffected: null });
  });

  it("sends nothing when the run was stopped while the Safe Mode prompt was open", async () => {
    let stopped = false;
    mocks.guard.mockImplementation(async () => {
      // The user confirms after the command's time ran out.
      stopped = true;
      return true;
    });
    const host = createHostServices({ plugin, invocation: inv, stopped: () => stopped });
    await expect(host.execute("DELETE FROM t")).rejects.toBeInstanceOf(PluginCancelled);
    expect(mocks.run).not.toHaveBeenCalled();
  });

  it("sends nothing when the guard declines (Safe Mode, read-only)", async () => {
    mocks.guard.mockResolvedValue(false);
    await expect(handleBridgeCall(services(), "execute", ["DELETE FROM t"])).rejects.toBeInstanceOf(
      PluginCancelled,
    );
    expect(mocks.run).not.toHaveBeenCalled();
  });

  it("reports a statement's error", async () => {
    mocks.run.mockResolvedValue([
      result(0, { error: { code: "DB", message: 'relation "t" does not exist' } as never }),
    ]);
    await expect(handleBridgeCall(services(), "execute", ["SELECT * FROM t"])).rejects.toThrow(
      'relation "t" does not exist',
    );
  });

  it("checks the arguments untrusted code sends", async () => {
    await expect(handleBridgeCall(services(), "execute", [42])).rejects.toThrow(/must be text/);
    await expect(handleBridgeCall(services(), "alert", ["only a title"])).rejects.toThrow(
      /argument 2/,
    );
  });

  it("shows alerts until dismissed", async () => {
    const done = handleBridgeCall(services(), "alert", ["Title", "Body"]);
    await vi.waitFor(() => expect(usePluginsStore.getState().prompts).toHaveLength(1));
    const prompt = usePluginsStore.getState().prompts[0]!.prompt;
    expect(prompt).toMatchObject({
      kind: "alert",
      plugin: "Test Plugin",
      title: "Title",
      message: "Body",
    });
    answerPluginPrompt(prompt.id, true);
    await expect(done).resolves.toBeNull();
  });

  it("writes to the clipboard", async () => {
    await handleBridgeCall(services(), "insertToClipboard", ["hello"]);
    expect(mocks.clipboard).toHaveBeenCalledWith("hello");
  });

  it("opens http(s) links only, asking once per plugin", async () => {
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    await expect(handleBridgeCall(services(), "openURL", ["file:///etc/passwd"])).rejects.toThrow(
      /http/,
    );
    const first = handleBridgeCall(services(), "openURL", ["https://example.com/a"]);
    await vi.waitFor(() => expect(usePluginsStore.getState().prompts).toHaveLength(1));
    const prompt = usePluginsStore.getState().prompts[0]!.prompt;
    expect(prompt).toMatchObject({ kind: "openUrl", url: "https://example.com/a" });
    answerPluginPrompt(prompt.id, true);
    await first;
    expect(getPrefs().pluginStates[plugin.identifier]?.urlConfirmed).toBe(true);
    // The second link opens without asking.
    await handleBridgeCall(services(), "openURL", ["https://example.com/b"]);
    expect(usePluginsStore.getState().prompts).toHaveLength(0);
    expect(open).toHaveBeenCalledTimes(2);
    open.mockRestore();
  });

  describe("what a link can carry out", () => {
    const confirmed = () =>
      setPref("pluginStates", { [plugin.identifier]: { enabled: true, urlConfirmed: true } });

    it("refuses a link longer than 2048 characters, before asking anything", async () => {
      const open = vi.spyOn(window, "open").mockImplementation(() => null);
      const long = `https://attacker.example/c?d=${"x".repeat(3000)}`;
      await expect(handleBridgeCall(services(), "openURL", [long])).rejects.toThrow(/2048/);
      expect(usePluginsStore.getState().prompts).toHaveLength(0);
      // Also once the plugin may open links freely.
      confirmed();
      await expect(handleBridgeCall(services(), "openURL", [long])).rejects.toThrow(/2048/);
      expect(open).not.toHaveBeenCalled();
      // At the limit it still opens.
      const atLimit = `https://example.com/?d=${"x".repeat(MAX_PLUGIN_URL_LENGTH - 25)}`;
      expect(atLimit).toHaveLength(MAX_PLUGIN_URL_LENGTH - 2);
      await handleBridgeCall(services(), "openURL", [atLimit]);
      expect(open).toHaveBeenCalledTimes(1);
      open.mockRestore();
    });

    it("counts the address the browser gets (non-ASCII text becomes %XX)", () => {
      const raw = `https://example.com/${"é".repeat(600)}`;
      expect(raw.length).toBeLessThan(MAX_PLUGIN_URL_LENGTH);
      expect(() => checkPluginUrl(raw)).toThrow(/2048/);
      expect(checkPluginUrl("  https://example.com/a b  ")).toBe("https://example.com/a%20b");
    });

    it("opens at most three links per ten seconds, then again once the window passed", async () => {
      vi.useFakeTimers();
      confirmed();
      const open = vi.spyOn(window, "open").mockImplementation(() => null);
      const go = (n: number) =>
        handleBridgeCall(services(), "openURL", [`https://example.com/${n}`]);
      await go(1);
      await go(2);
      await go(3);
      await expect(go(4)).rejects.toThrow("too many links opened");
      expect(open).toHaveBeenCalledTimes(3);
      await vi.advanceTimersByTimeAsync(9_000);
      await expect(go(5)).rejects.toThrow("too many links opened");
      // The first link left the window.
      await vi.advanceTimersByTimeAsync(1_001);
      await go(6);
      expect(open).toHaveBeenCalledTimes(4);
      open.mockRestore();
    });

    it("limits each plugin on its own", async () => {
      confirmed();
      setPref("pluginStates", {
        [plugin.identifier]: { urlConfirmed: true },
        "com.example.other": { urlConfirmed: true },
      });
      const open = vi.spyOn(window, "open").mockImplementation(() => null);
      const other = createHostServices({
        plugin: { identifier: "com.example.other", name: "Other" },
        invocation: inv,
      });
      for (let i = 0; i < 3; i++)
        await handleBridgeCall(services(), "openURL", ["https://a.example/"]);
      await expect(handleBridgeCall(services(), "openURL", ["https://a.example/"])).rejects.toThrow(
        "too many links",
      );
      await handleBridgeCall(other, "openURL", ["https://b.example/"]);
      expect(open).toHaveBeenCalledTimes(4);
      open.mockRestore();
    });

    it("gives the slot back when the user declines the link", async () => {
      const open = vi.spyOn(window, "open").mockImplementation(() => null);
      for (let i = 0; i < 5; i++) {
        const p = handleBridgeCall(services(), "openURL", ["https://example.com/"]);
        await vi.waitFor(() => expect(usePluginsStore.getState().prompts).toHaveLength(1));
        answerPluginPrompt(usePluginsStore.getState().prompts[0]!.prompt.id, false);
        // Declined five times in a row: never "too many links".
        await expect(p).rejects.toBeInstanceOf(PluginCancelled);
      }
      expect(open).not.toHaveBeenCalled();
      open.mockRestore();
    });
  });

  it("does not open a declined link", async () => {
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    const p = handleBridgeCall(services(), "openURL", ["https://example.com"]);
    await vi.waitFor(() => expect(usePluginsStore.getState().prompts).toHaveLength(1));
    answerPluginPrompt(usePluginsStore.getState().prompts[0]!.prompt.id, false);
    await expect(p).rejects.toBeInstanceOf(PluginCancelled);
    expect(open).not.toHaveBeenCalled();
    open.mockRestore();
  });
});
