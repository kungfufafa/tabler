import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { applyToEditor, getEditorState, seedEditor } from "./editorStates";
import {
  autoSaveFile,
  FILE_AUTOSAVE_DELAY_MS,
  installFileAutoSave,
  resetFileAutoSaveForTests,
} from "./fileAutoSave";
import { installQueryTabLifecycle } from "./lifecycle";
import {
  lastScript,
  newTabScript,
  openNewQueryTab,
  rememberLastScript,
  resetLastScriptsForTests,
} from "./newTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

let info: ConnectionInfo;

beforeAll(async () => {
  info = await ipc.connections.connect("p-local-pg");
  installQueryTabLifecycle();
  installFileAutoSave();
});

afterAll(async () => {
  await ipc.connections.disconnect(info.connectionId);
});

beforeEach(() => {
  resetMockQueryState({ emptyHistory: true });
  resetPrefsForTests();
  resetLastScriptsForTests();
  resetFileAutoSaveForTests();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    closeRequest: null,
    queryCounter: 0,
  });
  useQueryTabsStore.setState({ slices: {} });
  toast.error.mockClear();
});

const doc = (id: string) => getEditorState(id)?.doc.toString();
const dirty = () => {
  const tab = useTabsStore.getState().tabs[0];
  return tab?.kind === "query" ? tab.dirty === true : false;
};
const type = (id: string, text: string) =>
  applyToEditor(id, (s) => ({ changes: { from: s.doc.length, insert: text } }));

describe("Command + T should open", () => {
  it("brings back the last closed script when no query tab of the connection is open", () => {
    const first = openNewQueryTab(info.connectionId);
    expect(doc(first) ?? "").toBe("");
    seedEditor(first, "select 'kept';");
    useTabsStore.getState().close(first);
    expect(lastScript(info.profileId)).toBe("select 'kept';");

    const second = openNewQueryTab(info.connectionId);
    expect(doc(second)).toBe("select 'kept';");
    // One is open now: the next tab starts empty.
    const third = openNewQueryTab(info.connectionId);
    expect(doc(third) ?? "").toBe("");
  });

  it("opens an empty tab with “Empty tab”, or when query tabs are not saved", () => {
    rememberLastScript(info.profileId, "select 1;");
    setPref("newTabOpens", "empty");
    expect(newTabScript(info.connectionId)).toBeNull();
    setPref("newTabOpens", "query");
    setPref("autoSaveQueryTabs", "none");
    expect(newTabScript(info.connectionId)).toBeNull();
    setPref("autoSaveQueryTabs", "all");
    expect(newTabScript(info.connectionId)).toBe("select 1;");
    // Blank text is not remembered.
    rememberLastScript(info.profileId, "   ");
    expect(lastScript(info.profileId)).toBe("select 1;");
  });
});

describe("Automatically save queries while editing", () => {
  it("writes a file-backed tab shortly after an edit", async () => {
    vi.useFakeTimers();
    try {
      const write = vi.spyOn(ipc.sqlFiles, "write");
      const id = useTabsStore.getState().newQuery(info.connectionId);
      seedEditor(id, "select 1;");
      updateQueryTab(id, { filePath: "/tmp/auto.sql" });
      type(id, "\nselect 2;");
      expect(dirty()).toBe(true);
      await vi.advanceTimersByTimeAsync(FILE_AUTOSAVE_DELAY_MS - 100);
      expect(write).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(200);
      expect(write).toHaveBeenCalledExactlyOnceWith("/tmp/auto.sql", "select 1;\nselect 2;");
      expect(dirty()).toBe(false);
      expect(await ipc.sqlFiles.read("/tmp/auto.sql")).toBe("select 1;\nselect 2;");

      // Off: edits wait for ⌘S.
      setPref("editorAutoSaveFiles", false);
      type(id, "\nselect 3;");
      await vi.advanceTimersByTimeAsync(FILE_AUTOSAVE_DELAY_MS * 2);
      expect(write).toHaveBeenCalledOnce();
      expect(dirty()).toBe(true);
      // Back on: the pending edits save.
      setPref("editorAutoSaveFiles", true);
      await vi.advanceTimersByTimeAsync(50);
      expect(write).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("leaves untitled tabs alone and warns once when a write fails", async () => {
    const id = useTabsStore.getState().newQuery(info.connectionId);
    seedEditor(id, "select 1;");
    type(id, " -- x");
    expect(await autoSaveFile(id)).toBe(false);
    updateQueryTab(id, { filePath: "/tmp/fails.sql" });
    vi.spyOn(ipc.sqlFiles, "write").mockRejectedValue(new Error("disk full"));
    expect(await autoSaveFile(id)).toBe(false);
    expect(await autoSaveFile(id)).toBe(false);
    expect(toast.error).toHaveBeenCalledOnce();
  });

  /** `ipc.sqlFiles.write` whose calls stay pending until the test finishes them. */
  function deferredWrites() {
    const calls: Array<{ path: string; text: string; finish: (fail?: Error) => void }> = [];
    const write = vi.spyOn(ipc.sqlFiles, "write").mockImplementation(
      (path, text) =>
        new Promise<void>((resolve, reject) => {
          calls.push({ path, text, finish: (fail) => (fail ? reject(fail) : resolve()) });
        }),
    );
    return { calls, write };
  }

  it("never overlaps two writes of one file, and the last write holds the latest text", async () => {
    vi.useFakeTimers();
    try {
      const { calls, write } = deferredWrites();
      const id = useTabsStore.getState().newQuery(info.connectionId);
      seedEditor(id, "select 1;");
      updateQueryTab(id, { filePath: "/tmp/serial.sql" });
      type(id, "\nselect 2;");
      await vi.advanceTimersByTimeAsync(FILE_AUTOSAVE_DELAY_MS + 50);
      expect(write).toHaveBeenCalledOnce();
      expect(calls[0]!.text).toBe("select 1;\nselect 2;");

      // Typing goes on and the next timers fire while the first write has not finished.
      type(id, "\nselect 3;");
      await vi.advanceTimersByTimeAsync(FILE_AUTOSAVE_DELAY_MS + 50);
      type(id, "\nselect 4;");
      await vi.advanceTimersByTimeAsync(FILE_AUTOSAVE_DELAY_MS + 50);
      expect(write).toHaveBeenCalledOnce();

      // The first one ends: the next write starts then, with the latest text.
      calls[0]!.finish();
      await vi.advanceTimersByTimeAsync(0);
      expect(write).toHaveBeenCalledTimes(2);
      expect(calls[1]!.text).toBe("select 1;\nselect 2;\nselect 3;\nselect 4;");
      expect(dirty()).toBe(true);
      calls[1]!.finish();
      await vi.advanceTimersByTimeAsync(0);
      expect(dirty()).toBe(false);
      // Nothing more is written, however long we wait.
      await vi.advanceTimersByTimeAsync(FILE_AUTOSAVE_DELAY_MS * 5);
      expect(write).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("makes a save asked for during a write wait for it, then resolve once the tab is saved", async () => {
    vi.useFakeTimers();
    try {
      const { calls, write } = deferredWrites();
      const id = useTabsStore.getState().newQuery(info.connectionId);
      seedEditor(id, "select 1;");
      updateQueryTab(id, { filePath: "/tmp/flush.sql" });
      type(id, " -- a");
      const first = autoSaveFile(id);
      type(id, " -- b");
      // A flush (the window is going away) while the write runs.
      const second = autoSaveFile(id);
      expect(second).toBe(first);
      expect(write).toHaveBeenCalledOnce();
      let settled = false;
      void second.then(() => (settled = true));
      calls[0]!.finish();
      await vi.advanceTimersByTimeAsync(0);
      expect(write).toHaveBeenCalledTimes(2);
      expect(settled).toBe(false);
      expect(calls[1]!.text).toBe("select 1; -- a -- b");
      calls[1]!.finish();
      await vi.advanceTimersByTimeAsync(0);
      expect(await second).toBe(true);
      expect(dirty()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the failure toast to one per file when a queued write fails too", async () => {
    vi.useFakeTimers();
    try {
      const { calls, write } = deferredWrites();
      const id = useTabsStore.getState().newQuery(info.connectionId);
      seedEditor(id, "select 1;");
      updateQueryTab(id, { filePath: "/tmp/fails-twice.sql" });
      type(id, " -- a");
      const done = autoSaveFile(id);
      type(id, " -- b");
      void autoSaveFile(id);
      calls[0]!.finish(new Error("disk full"));
      await vi.advanceTimersByTimeAsync(0);
      expect(write).toHaveBeenCalledTimes(2);
      calls[1]!.finish(new Error("disk full"));
      expect(await done).toBe(false);
      expect(toast.error).toHaveBeenCalledOnce();
      expect(dirty()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});
