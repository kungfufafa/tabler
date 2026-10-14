import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockPlugins } from "@/lib/ipc/mock/plugins";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { isPluginEnabled, isPluginStates, withoutPlugin, withPluginState } from "./pluginPrefs";
import { findCommand, pluginCommands, pluginEntries } from "./registry";
import { nativeMenuEntries } from "./menuModel";
import { keyApplies } from "./integration";
import {
  answerPluginPrompt,
  dismissPluginPrompts,
  installPluginFromFolder,
  newPluginRunId,
  openPluginManager,
  refreshPlugins,
  removePlugin,
  requestPluginPrompt,
  resetPluginsStoreForTests,
  setPluginEnabled,
  usePluginsStore,
} from "./store";
import { parseRunId, runIdOf } from "./types";

beforeEach(() => {
  resetPrefsForTests();
  resetPluginsStoreForTests();
  resetMockPlugins();
});
afterEach(() => vi.restoreAllMocks());

const entry = (id: string) =>
  pluginEntries(usePluginsStore.getState().list).find((p) => p.identifier === id)!;

async function nextPrompt() {
  await vi.waitFor(() => expect(usePluginsStore.getState().prompts.length).toBeGreaterThan(0));
  return usePluginsStore.getState().prompts[0]!.prompt;
}

describe("the plugin manager's store", () => {
  it("loads the Plugins folder (built-ins first, broken folders listed)", async () => {
    openPluginManager();
    expect(usePluginsStore.getState().managerOpen).toBe(true);
    await vi.waitFor(() => expect(usePluginsStore.getState().list).not.toBeNull());
    const list = usePluginsStore.getState().list!;
    expect(list.plugins.map((p) => p.manifest.name)).toEqual(["Row Counter", "Sandbox Demo"]);
    expect(list.invalid).toHaveLength(1);
    const entries = pluginEntries(list);
    expect(entries.slice(0, 5).every((p) => p.builtin)).toBe(true);
    expect(entries.map((p) => p.name)).toContain("SQL Formatter");
  });

  it("asks for permission on a third-party plugin's first enable", async () => {
    await refreshPlugins();
    const rowCounter = entry("com.example.row-counter");
    expect(isPluginEnabled(getPrefs().pluginStates, rowCounter.identifier, false)).toBe(false);

    // Declined: stays disabled.
    const declined = setPluginEnabled(rowCounter, true);
    const ask = await nextPrompt();
    expect(ask).toMatchObject({ kind: "permission", plugin: { name: "Row Counter" } });
    answerPluginPrompt(ask.id, false);
    expect(await declined).toBe(false);
    expect(isPluginEnabled(getPrefs().pluginStates, rowCounter.identifier, false)).toBe(false);

    // Granted: enabled, and later toggles do not ask again.
    const granted = setPluginEnabled(rowCounter, true);
    answerPluginPrompt((await nextPrompt()).id, true);
    expect(await granted).toBe(true);
    expect(getPrefs().pluginStates[rowCounter.identifier]).toEqual({
      enabled: true,
      granted: true,
    });
    await setPluginEnabled(rowCounter, false);
    expect(await setPluginEnabled(rowCounter, true)).toBe(true);
    expect(usePluginsStore.getState().prompts).toHaveLength(0);
  });

  it("enables built-ins by default and lets them be disabled", async () => {
    const formatter = entry("tabler.sql-formatter");
    expect(isPluginEnabled(getPrefs().pluginStates, formatter.identifier, true)).toBe(true);
    await setPluginEnabled(formatter, false);
    expect(isPluginEnabled(getPrefs().pluginStates, formatter.identifier, true)).toBe(false);
    expect(pluginCommands(null, getPrefs().pluginStates).map((c) => c.name)).not.toContain(
      "SQL Format",
    );
  });

  it("installs from a folder, then asks to enable it", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/hello-world");
    const done = installPluginFromFolder();
    const ask = await nextPrompt();
    expect(ask).toMatchObject({
      kind: "permission",
      plugin: { identifier: "com.example.hello-world" },
    });
    answerPluginPrompt(ask.id, true);
    await done;
    const names = usePluginsStore.getState().list!.plugins.map((p) => p.manifest.name);
    expect(names).toContain("Hello World");
    expect(
      findCommand(
        runIdOf("com.example.hello-world", 0),
        usePluginsStore.getState().list,
        getPrefs().pluginStates,
      )?.name,
    ).toBe("Say Hello");
  });

  describe("installing over a plugin with the same identifier", () => {
    const ROW_COUNTER = "com.example.row-counter";
    const granted = {
      [ROW_COUNTER]: { enabled: true, granted: true, urlConfirmed: true },
    };

    it("drops the state of a plugin by another author and asks for permission again", async () => {
      setPref("pluginStates", granted);
      await refreshPlugins();
      const success = vi.spyOn(toast, "success").mockImplementation(() => "");
      // The mock install of a folder named "row-counter": the same identifier, no author.
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/row-counter");
      const done = installPluginFromFolder();
      const ask = await nextPrompt();
      expect(ask).toMatchObject({ kind: "permission", plugin: { identifier: ROW_COUNTER } });
      // Not enabled, not granted, links not confirmed: nothing is inherited.
      expect(getPrefs().pluginStates[ROW_COUNTER]).toBeUndefined();
      expect(isPluginEnabled(getPrefs().pluginStates, ROW_COUNTER, false)).toBe(false);
      expect(success).toHaveBeenCalledWith(
        expect.stringContaining("Installed Row Counter"),
        expect.objectContaining({ description: expect.stringContaining("Replaces Row Counter") }),
      );
      answerPluginPrompt(ask.id, true);
      await done;
      expect(getPrefs().pluginStates[ROW_COUNTER]).toEqual({ enabled: true, granted: true });
    });

    it("keeps the state of an update (same author and name)", async () => {
      setPref("pluginStates", granted);
      await refreshPlugins();
      const current = usePluginsStore
        .getState()
        .list!.plugins.find((p) => p.manifest.identifier === ROW_COUNTER)!;
      vi.spyOn(ipc.plugins, "install").mockResolvedValue({
        manifest: { ...current.manifest, version: "1.3" },
        folder: current.folder,
      });
      const success = vi.spyOn(toast, "success").mockImplementation(() => "");
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/row-counter");
      await installPluginFromFolder();
      expect(usePluginsStore.getState().prompts).toHaveLength(0);
      expect(getPrefs().pluginStates[ROW_COUNTER]).toEqual(granted[ROW_COUNTER]);
      expect(success).toHaveBeenCalledWith("Installed Row Counter 1.3", undefined);
    });

    it("treats a different name as a different plugin too", async () => {
      setPref("pluginStates", granted);
      await refreshPlugins();
      const current = usePluginsStore
        .getState()
        .list!.plugins.find((p) => p.manifest.identifier === ROW_COUNTER)!;
      vi.spyOn(ipc.plugins, "install").mockResolvedValue({
        manifest: { ...current.manifest, name: "Row Counter Pro" },
        folder: current.folder,
      });
      vi.spyOn(toast, "success").mockImplementation(() => "");
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/row-counter");
      const done = installPluginFromFolder();
      const ask = await nextPrompt();
      expect(ask).toMatchObject({ kind: "permission", plugin: { name: "Row Counter Pro" } });
      expect(getPrefs().pluginStates[ROW_COUNTER]).toBeUndefined();
      answerPluginPrompt(ask.id, false);
      await done;
      expect(isPluginEnabled(getPrefs().pluginStates, ROW_COUNTER, false)).toBe(false);
    });

    describe("with no installed plugin to compare to", () => {
      const HELLO = "com.example.hello-world";
      const stale = { [HELLO]: { enabled: true, granted: true, urlConfirmed: true } };

      async function installHello(folder = "hello-world") {
        vi.spyOn(toast, "success").mockImplementation(() => "");
        vi.spyOn(window, "prompt").mockReturnValue(`/Users/me/Downloads/${folder}`);
        const done = installPluginFromFolder();
        const ask = await nextPrompt();
        return { done, ask };
      }

      it("drops a stale grant when its folder is gone from the Plugins folder", async () => {
        // The plugin was removed in Finder: the list no longer holds it, its state is left over.
        setPref("pluginStates", stale);
        await refreshPlugins();
        expect(
          usePluginsStore.getState().list!.plugins.some((p) => p.manifest.identifier === HELLO),
        ).toBe(false);
        const { done, ask } = await installHello();
        expect(ask).toMatchObject({ kind: "permission", plugin: { identifier: HELLO } });
        // Nothing is inherited: not enabled, not granted, links not confirmed.
        expect(getPrefs().pluginStates[HELLO]).toBeUndefined();
        expect(isPluginEnabled(getPrefs().pluginStates, HELLO, false)).toBe(false);
        answerPluginPrompt(ask.id, true);
        await done;
        expect(getPrefs().pluginStates[HELLO]).toEqual({ enabled: true, granted: true });
      });

      it("drops a stale grant when the old manifest is broken and listed as invalid", async () => {
        const BROKEN = "com.example.broken-plugin";
        setPref("pluginStates", { [BROKEN]: { enabled: true, granted: true, urlConfirmed: true } });
        await refreshPlugins();
        expect(usePluginsStore.getState().list!.invalid).toHaveLength(1);
        const { done, ask } = await installHello("broken-plugin");
        expect(ask).toMatchObject({ kind: "permission", plugin: { identifier: BROKEN } });
        expect(getPrefs().pluginStates[BROKEN]).toBeUndefined();
        answerPluginPrompt(ask.id, false);
        await done;
        expect(isPluginEnabled(getPrefs().pluginStates, BROKEN, false)).toBe(false);
      });

      it("drops a stale grant stored under another case of the identifier", async () => {
        setPref("pluginStates", {
          "com.Example.Hello-World": { enabled: true, granted: true, urlConfirmed: true },
          "com.example.sandbox-demo": { enabled: true },
        });
        await refreshPlugins();
        const { done, ask } = await installHello();
        expect(Object.keys(getPrefs().pluginStates)).toEqual(["com.example.sandbox-demo"]);
        answerPluginPrompt(ask.id, false);
        await done;
      });

      it("does not trust a list that could not be read", async () => {
        // A plain update of Row Counter, but the Plugins folder cannot be read just now: the
        // list held from before is no evidence of what is installed, so the grant is not inherited.
        setPref("pluginStates", granted);
        await refreshPlugins();
        const current = usePluginsStore
          .getState()
          .list!.plugins.find((p) => p.manifest.identifier === ROW_COUNTER)!;
        vi.spyOn(ipc.plugins, "list").mockRejectedValue(new Error("the Plugins folder is locked"));
        vi.spyOn(ipc.plugins, "install").mockResolvedValue({
          manifest: { ...current.manifest, version: "1.3" },
          folder: current.folder,
        });
        vi.spyOn(toast, "success").mockImplementation(() => "");
        vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/row-counter");
        const done = installPluginFromFolder();
        const ask = await nextPrompt();
        expect(ask).toMatchObject({ kind: "permission", plugin: { identifier: ROW_COUNTER } });
        expect(getPrefs().pluginStates[ROW_COUNTER]).toBeUndefined();
        answerPluginPrompt(ask.id, false);
        await done;
      });

      it("leaves a first install with nothing stored alone", async () => {
        await refreshPlugins();
        const { done, ask } = await installHello();
        expect(ask).toMatchObject({ kind: "permission", plugin: { identifier: HELLO } });
        expect(getPrefs().pluginStates).toEqual({});
        answerPluginPrompt(ask.id, true);
        await done;
        expect(getPrefs().pluginStates[HELLO]).toEqual({ enabled: true, granted: true });
      });
    });

    it("leaves other plugins' state alone", async () => {
      setPref("pluginStates", { ...granted, "com.example.sandbox-demo": { enabled: true } });
      await refreshPlugins();
      vi.spyOn(toast, "success").mockImplementation(() => "");
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/row-counter");
      const done = installPluginFromFolder();
      answerPluginPrompt((await nextPrompt()).id, false);
      await done;
      expect(getPrefs().pluginStates["com.example.sandbox-demo"]).toEqual({ enabled: true });
    });
  });

  it("removes a plugin and forgets its state", async () => {
    await refreshPlugins();
    const demo = entry("com.example.sandbox-demo");
    const enabling = setPluginEnabled(demo, true);
    answerPluginPrompt((await nextPrompt()).id, true);
    await enabling;
    await removePlugin(demo);
    expect(getPrefs().pluginStates[demo.identifier]).toBeUndefined();
    expect(
      usePluginsStore.getState().list!.plugins.map((p) => p.manifest.identifier),
    ).not.toContain(demo.identifier);
  });
});

describe("the dialogs of a run", () => {
  const alert = (title: string, run: number | null) =>
    requestPluginPrompt({ kind: "alert", plugin: "P", title, message: "" }, run);

  it("removes a run's queued dialogs and answers them as declined", async () => {
    const run = newPluginRunId();
    const other = newPluginRunId();
    expect(other).not.toBe(run);
    const mine = [alert("a", run), alert("b", run), alert("c", run)];
    const theirs = alert("d", other);
    const app = alert("e", null);
    expect(usePluginsStore.getState().prompts).toHaveLength(5);

    dismissPluginPrompts(run);
    expect(await Promise.all(mine)).toEqual([false, false, false]);
    const left = usePluginsStore
      .getState()
      .prompts.map((p) => (p.prompt as { title: string }).title);
    expect(left).toEqual(["d", "e"]);

    // Another run's, and the app's own, are still waiting for their answers.
    answerPluginPrompt(usePluginsStore.getState().prompts[0]!.prompt.id, true);
    expect(await theirs).toBe(true);
    dismissPluginPrompts(run);
    expect(usePluginsStore.getState().prompts).toHaveLength(1);
    answerPluginPrompt(usePluginsStore.getState().prompts[0]!.prompt.id, true);
    expect(await app).toBe(true);
  });

  it("does nothing for a run without dialogs", () => {
    const before = usePluginsStore.getState().prompts;
    dismissPluginPrompts(newPluginRunId());
    expect(usePluginsStore.getState().prompts).toBe(before);
  });
});

describe("the plugins' menus", () => {
  it("shows one-command plugins directly and the others as submenus", async () => {
    await refreshPlugins();
    const states = withPluginState(
      withPluginState(getPrefs().pluginStates, "com.example.row-counter", {
        enabled: true,
        granted: true,
      }),
      "tabler.dummy-data",
      { enabled: false },
    );
    const entries = nativeMenuEntries(pluginCommands(usePluginsStore.getState().list, states));
    expect(entries[0]).toEqual({
      kind: "command",
      run: "tabler.sql-formatter#0",
      label: "SQL Format",
      accelerator: "Ctrl+I",
    });
    expect(entries.map((e) => ("label" in e ? e.label : e.kind))).toEqual([
      "SQL Format",
      "Open URL",
      "Copy as UPDATE Statement",
      "Insert UUID",
      "separator",
      "Row Counter",
    ]);
    const rowCounter = entries[5];
    expect(rowCounter).toMatchObject({
      kind: "submenu",
      items: [
        { kind: "command", run: "com.example.row-counter#0", accelerator: "CmdOrCtrl+Shift+U" },
        { kind: "command", run: "com.example.row-counter#1", accelerator: null },
      ],
    });
  });

  it("answers a key only where its contexts allow", () => {
    expect(keyApplies(["editor"], "editor")).toBe(true);
    expect(keyApplies(["editor"], "grid")).toBe(false);
    expect(keyApplies(["global"], "sidebar")).toBe(true);
    expect(keyApplies(["global"], "dialog")).toBe(false);
    expect(keyApplies(["grid"], "grid-edit")).toBe(false);
  });

  it("names commands by run id", () => {
    expect(parseRunId("com.example.x#2")).toEqual({ identifier: "com.example.x", index: 2 });
    expect(parseRunId("nope")).toBeNull();
    expect(parseRunId("a#-1")).toBeNull();
  });
});

describe("the pluginStates preference", () => {
  it("validates stored states", () => {
    expect(isPluginStates({})).toBe(true);
    expect(isPluginStates({ "com.x": { enabled: true, granted: false, urlConfirmed: true } })).toBe(
      true,
    );
    expect(isPluginStates({ "com.x": { enabled: "yes" } })).toBe(false);
    expect(isPluginStates({ "com.x": { admin: true } })).toBe(false);
    expect(isPluginStates([])).toBe(false);
    expect(isPluginStates(null)).toBe(false);
  });

  it("needs both enabled and granted for third-party plugins", () => {
    expect(isPluginEnabled({ a: { enabled: true } }, "a", false)).toBe(false);
    expect(isPluginEnabled({ a: { enabled: true, granted: true } }, "a", false)).toBe(true);
    expect(isPluginEnabled({}, "b", true)).toBe(true);
    expect(isPluginEnabled({ b: { enabled: false } }, "b", true)).toBe(false);
    expect(withoutPlugin({ a: {}, b: {} }, "a")).toEqual({ b: {} });
  });
});
