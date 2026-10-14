import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { openSqlFile } from "@/features/deeplink/open";
import { closeImport, useImportStore } from "@/features/import/store";
import { loadViewerFile } from "@/features/sqlviewer/files";
import { viewerText } from "@/features/sqlviewer/buffers";
import { viewerTitle } from "@/features/sqlviewer/labels";
import { ensureDoc, getDoc, newDoc, resetSqlViewerStore } from "@/features/sqlviewer/store";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { getQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { saveQuery, openQueryFile } from "./controller";
import { getEditorState, seedEditor } from "./editorStates";
import { baseName, headTitle, readSqlFileGuarded } from "./files";
import {
  answerLargeFile,
  HEAD_BYTES,
  LARGE_FILE_BYTES,
  LARGE_GZ_BYTES,
  sizeLabel,
  useLargeFileStore,
} from "./largeFile";
import { LargeFileDialog } from "./LargeFileDialog";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const MB = 1024 * 1024;
const BIG = "/Users/me/dumps/huge-dump.sql";
const SMALL = "/Users/me/queries/small.sql";

let info: ConnectionInfo;

beforeAll(async () => {
  info = await ipc.connections.connect("p-local-pg");
});

afterAll(async () => {
  await ipc.connections.disconnect(info.connectionId);
});

beforeEach(() => {
  resetMockQueryState({ emptyHistory: true });
  closeImport();
  resetSqlViewerStore();
  useLargeFileStore.setState({ request: null });
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
});

afterEach(() => {
  answerLargeFile("cancel");
  Object.values(toast).forEach((fn) => fn.mockReset());
  vi.restoreAllMocks();
});

/** A query tab with some text (so Open goes to a new tab, not into this one). */
function busyTab(): string {
  const id = useTabsStore.getState().newQuery(info.connectionId);
  seedEditor(id, "select 1;", { dialect: "postgres" });
  return id;
}

/** `ipc.sqlFiles.info` for BIG: the size on disk. */
function bigFile(bytes: number, compressed = false) {
  return vi.spyOn(ipc.sqlFiles, "info").mockResolvedValue({ bytes, compressed });
}

const HEAD_TEXT = "INSERT INTO t VALUES (1);\nINSERT INTO t VALUES (2);\n";
function headRead(extra: Partial<Awaited<ReturnType<typeof ipc.sqlFiles.readHead>>> = {}) {
  return vi.spyOn(ipc.sqlFiles, "readHead").mockResolvedValue({
    text: HEAD_TEXT,
    totalBytes: 50 * MB,
    truncated: true,
    compressed: false,
    encoding: null,
    ...extra,
  });
}

const queryTabs = () => useTabsStore.getState().tabs.filter((t) => t.kind === "query");

/** Starts ⌘O with `path` picked and waits for the large-file question. */
async function openAndWaitForQuestion(tabId: string, path = BIG) {
  vi.spyOn(window, "prompt").mockReturnValue(path);
  const opened = openQueryFile(tabId);
  await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
  // Wrapped: an async function would otherwise wait for the promise it returns.
  return { opened };
}

describe("opening an SQL file in the editor", () => {
  it("opens a small file without asking anything, as it always did", async () => {
    await ipc.sqlFiles.write(SMALL, "select 42;");
    const info_ = vi.spyOn(ipc.sqlFiles, "info");
    const head = vi.spyOn(ipc.sqlFiles, "readHead");
    vi.spyOn(window, "prompt").mockReturnValue(SMALL);
    const before = queryTabs().length;
    await openQueryFile(busyTab());
    expect(useLargeFileStore.getState().request).toBeNull();
    expect(info_).toHaveBeenCalledOnce();
    expect(head).toHaveBeenCalledExactlyOnceWith(SMALL, null);
    const tab = queryTabs().at(-1)!;
    expect(queryTabs()).toHaveLength(before + 2);
    expect(getEditorState(tab.id)?.doc.toString()).toBe("select 42;");
    expect(getQueryTab(tab.id)).toMatchObject({ filePath: SMALL, headNotice: null });
    expect(tab.title).toBe("small.sql");
    expect(toast.message).not.toHaveBeenCalled();
  });

  it("asks above 8 MB (2 MB when gzipped), and 'Import as SQL dump…' hands the file to the wizard", async () => {
    bigFile(50 * MB);
    const read = vi.spyOn(ipc.sqlFiles, "read");
    const head = vi.spyOn(ipc.sqlFiles, "readHead");
    const tabId = busyTab();
    const { opened } = await openAndWaitForQuestion(tabId);
    expect(useLargeFileStore.getState().request).toMatchObject({
      name: "huge-dump.sql",
      bytes: 50 * MB,
      compressed: false,
      canImport: true,
    });
    answerLargeFile("import");
    await opened;
    // The SQL import wizard on this connection, on the file; nothing was loaded into an editor.
    expect(useImportStore.getState().request).toEqual({
      connectionId: info.connectionId,
      kind: "sql",
      table: null,
    });
    expect(useImportStore.getState().path).toBe(BIG);
    expect(read).not.toHaveBeenCalled();
    expect(head).not.toHaveBeenCalled();
    expect(queryTabs()).toHaveLength(1);
  });

  it("threshold: 8 MB plain opens at once, a bit more asks; a 3 MB gzip file asks, 1 MB does not", async () => {
    for (const [bytes, compressed, asks] of [
      [LARGE_FILE_BYTES, false, false],
      [LARGE_FILE_BYTES + 1, false, true],
      [3 * MB, false, false],
      [LARGE_GZ_BYTES, true, false],
      [LARGE_GZ_BYTES + 1, true, true],
      [1 * MB, true, false],
    ] as const) {
      vi.restoreAllMocks();
      bigFile(bytes, compressed);
      headRead({ truncated: false, totalBytes: bytes });
      // Its own path each time: a file that is open already is only brought to the front.
      vi.spyOn(window, "prompt").mockReturnValue(`${BIG}.${bytes}.${compressed}`);
      const opened = openQueryFile(busyTab());
      if (asks) {
        await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
        expect(useLargeFileStore.getState().request?.compressed).toBe(compressed);
        answerLargeFile("cancel");
      } else {
        expect(useLargeFileStore.getState().request).toBeNull();
      }
      await opened;
    }
  });

  it("'Open the first 5 MB' opens a tab that is NOT linked to the file, with a notice", async () => {
    bigFile(50 * MB);
    const head = headRead();
    const tabId = busyTab();
    const { opened } = await openAndWaitForQuestion(tabId);
    answerLargeFile("head");
    await opened;
    expect(head).toHaveBeenCalledExactlyOnceWith(BIG, HEAD_BYTES);
    const tab = queryTabs().at(-1)!;
    expect(tab.id).not.toBe(tabId);
    expect(getEditorState(tab.id)?.doc.toString()).toBe(HEAD_TEXT);
    const slice = getQueryTab(tab.id);
    // No file behind the tab: saving can never truncate the original.
    expect(slice.filePath).toBeNull();
    expect(slice.headNotice).toBe(
      "Showing the first 5 MB of huge-dump.sql; saving here will not overwrite the file",
    );
    expect(tab.title).toBe("huge-dump-head.sql");
    expect(tab.title).toBe(headTitle(BIG));
  });

  it("saving the head asks for a new file: the original is never written", async () => {
    bigFile(50 * MB);
    headRead();
    const { opened } = await openAndWaitForQuestion(busyTab());
    answerLargeFile("head");
    await opened;
    const tab = queryTabs().at(-1)!;
    const write = vi.spyOn(ipc.sqlFiles, "write");
    const prompt = vi.mocked(window.prompt);
    prompt.mockClear();
    prompt.mockReturnValue("/Users/me/queries/first-part.sql");
    expect(await saveQuery(tab.id)).toBe(true);
    // It asked where to save (the tab has no file), and wrote only the path it was given.
    expect(prompt).toHaveBeenCalledOnce();
    expect(write).toHaveBeenCalledExactlyOnceWith("/Users/me/queries/first-part.sql", HEAD_TEXT);
    expect(write).not.toHaveBeenCalledWith(BIG, expect.anything());
    // Saved under its own name now: linked to that file, and the notice is gone.
    expect(getQueryTab(tab.id)).toMatchObject({
      filePath: "/Users/me/queries/first-part.sql",
      headNotice: null,
    });
  });

  it("'Open anyway' reads the whole file and links the tab to it", async () => {
    bigFile(50 * MB);
    const head = headRead({ truncated: false, text: "select 7;" });
    const { opened } = await openAndWaitForQuestion(busyTab());
    answerLargeFile("anyway");
    await opened;
    expect(head).toHaveBeenCalledExactlyOnceWith(BIG, null);
    const tab = queryTabs().at(-1)!;
    expect(getQueryTab(tab.id)).toMatchObject({ filePath: BIG, headNotice: null });
    expect(tab.title).toBe("huge-dump.sql");
  });

  it("Cancel opens nothing and reads nothing", async () => {
    bigFile(50 * MB);
    const head = headRead();
    const { opened } = await openAndWaitForQuestion(busyTab());
    answerLargeFile("cancel");
    await opened;
    expect(head).not.toHaveBeenCalled();
    expect(queryTabs()).toHaveLength(1);
  });

  it("reuses the empty tab for a head too, and still does not link it", async () => {
    bigFile(50 * MB);
    headRead();
    const id = useTabsStore.getState().newQuery(info.connectionId);
    seedEditor(id, "", { dialect: "postgres" });
    const { opened } = await openAndWaitForQuestion(id);
    answerLargeFile("head");
    await opened;
    expect(queryTabs()).toHaveLength(1);
    expect(getEditorState(id)?.doc.toString()).toBe(HEAD_TEXT);
    expect(getQueryTab(id).filePath).toBeNull();
    expect(getQueryTab(id).headNotice).toContain("first 5 MB of huge-dump.sql");
  });

  it("a file that is not UTF-8 opens, and a toast says how it was read", async () => {
    const real = vi.spyOn(ipc.sqlFiles, "readHead");
    real.mockResolvedValue({
      text: "select 'café';",
      totalBytes: 14,
      truncated: false,
      compressed: false,
      encoding: "Windows-1252",
    });
    vi.spyOn(ipc.sqlFiles, "info").mockResolvedValue({ bytes: 14, compressed: false });
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/queries/latin1.sql");
    await openQueryFile(busyTab());
    expect(getEditorState(queryTabs().at(-1)!.id)?.doc.toString()).toBe("select 'café';");
    expect(toast.message).toHaveBeenCalledWith("Opened as Windows-1252", expect.anything());
  });

  it("says why a file could not be opened", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/queries/missing.sql");
    await openQueryFile(busyTab());
    expect(toast.error).toHaveBeenCalledWith("Could not open the file", expect.anything());
    expect(queryTabs()).toHaveLength(1);
  });
});

describe("readSqlFileGuarded", () => {
  it("does not offer the import wizard without a connection to import into", async () => {
    bigFile(50 * MB);
    const read = readSqlFileGuarded(BIG, {});
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    expect(useLargeFileStore.getState().request?.canImport).toBe(false);
    answerLargeFile("import");
    // Nothing to import into: treated as no answer (and nothing opened).
    await expect(read).resolves.toBeNull();
    expect(useImportStore.getState().request).toBeNull();
  });

  it("reports a head only when the file really went on past it", async () => {
    bigFile(9 * MB);
    headRead({ truncated: false, totalBytes: 9 * MB, text: "select 1;" });
    const read = readSqlFileGuarded(BIG, { connectionId: info.connectionId });
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    answerLargeFile("head");
    // The 9 MB file was shorter than the head asked for (a compressed size misleads): all of it.
    await expect(read).resolves.toMatchObject({
      text: "select 1;",
      readOnlyHead: false,
      headNotice: null,
    });
  });

  it("sizes read as '5 MB', '12.4 MB' and '1.20 GB'", () => {
    expect(sizeLabel(HEAD_BYTES)).toBe("5 MB");
    expect(sizeLabel(Math.round(12.4 * MB))).toBe("12.4 MB");
    expect(sizeLabel(1.2 * 1024 * MB)).toBe("1.20 GB");
    expect(baseName(BIG)).toBe("huge-dump.sql");
  });
});

describe("LargeFileDialog", () => {
  it("offers the four choices and answers the question", async () => {
    render(<LargeFileDialog />);
    bigFile(50 * MB);
    const read = readSqlFileGuarded(BIG, { connectionId: info.connectionId });
    expect(
      await screen.findByRole("button", { name: "Open the first 5 MB (read-only)" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toHaveTextContent("huge-dump.sql is 50 MB");
    for (const name of ["Import as SQL dump…", "Open anyway", "Cancel"])
      expect(screen.getByRole("button", { name })).toBeInTheDocument();

    headRead();
    fireEvent.click(screen.getByRole("button", { name: "Open the first 5 MB (read-only)" }));
    await expect(read).resolves.toMatchObject({ readOnlyHead: true });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("Escape cancels, and 'Import as SQL dump…' is left out without a connection", async () => {
    render(<LargeFileDialog />);
    bigFile(50 * MB);
    const read = readSqlFileGuarded(BIG);
    await screen.findByRole("alertdialog");
    expect(screen.queryByRole("button", { name: "Import as SQL dump…" })).toBeNull();
    fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    await expect(read).resolves.toBeNull();
  });
});

describe("the SQL Viewer", () => {
  it("opens the head of a big file unlinked, with a notice and a title of its own", async () => {
    bigFile(50 * MB);
    headRead();
    ensureDoc("standalone", () => newDoc(null));
    const loaded = loadViewerFile("standalone", BIG);
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    // The standalone viewer has no connection: no import wizard.
    expect(useLargeFileStore.getState().request?.canImport).toBe(false);
    answerLargeFile("head");
    await expect(loaded).resolves.toBe(true);
    const doc = getDoc("standalone")!;
    expect(doc).toMatchObject({ path: null, headOf: BIG, loading: false, dirty: false });
    expect(doc.headNotice).toContain("first 5 MB of huge-dump.sql");
    expect(viewerTitle(doc)).toBe("huge-dump-head.sql");
    expect(viewerText("standalone")).toBe(HEAD_TEXT);
  });

  it("a viewer tab can send a big file to the import wizard", async () => {
    bigFile(50 * MB);
    const tabId = useTabsStore.getState().newQuery(info.connectionId);
    // A tool tab of the same workspace stands for the viewer tab.
    useTabsStore.setState((s) => ({
      tabs: s.tabs.map((t) => (t.id === tabId ? { ...t } : t)),
    }));
    ensureDoc(tabId, () => newDoc(null));
    const loaded = loadViewerFile(tabId, BIG);
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    expect(useLargeFileStore.getState().request?.canImport).toBe(true);
    answerLargeFile("import");
    await expect(loaded).resolves.toBe(false);
    expect(useImportStore.getState().request).toMatchObject({ kind: "sql" });
    expect(useImportStore.getState().path).toBe(BIG);
    // The viewer is not left "loading".
    expect(getDoc(tabId)?.loading).toBe(false);
  });

  it("opens a small file as before", async () => {
    await ipc.sqlFiles.write(SMALL, "select 5;");
    ensureDoc("standalone", () => newDoc(null));
    await expect(loadViewerFile("standalone", SMALL)).resolves.toBe(true);
    expect(getDoc("standalone")).toMatchObject({ path: SMALL, headOf: null, headNotice: null });
    expect(viewerText("standalone")).toBe("select 5;");
  });
});

describe("opening a file from a link or the Recent menu", () => {
  it("opens the head of a big file unlinked in a new query tab of the connection", async () => {
    bigFile(50 * MB);
    headRead();
    const opened = openSqlFile(info.connectionId, BIG);
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    answerLargeFile("head");
    await opened;
    const tab = queryTabs().at(-1)!;
    expect(getEditorState(tab.id)?.doc.toString()).toBe(HEAD_TEXT);
    expect(getQueryTab(tab.id)).toMatchObject({ filePath: null });
    expect(getQueryTab(tab.id).headNotice).toContain("first 5 MB");
  });

  it("opens a small file as before and a cancelled big one not at all", async () => {
    await ipc.sqlFiles.write(SMALL, "select 6;");
    await openSqlFile(info.connectionId, SMALL);
    expect(getQueryTab(queryTabs().at(-1)!.id).filePath).toBe(SMALL);

    bigFile(50 * MB);
    const before = queryTabs().length;
    const opened = openSqlFile(info.connectionId, BIG);
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    act(() => answerLargeFile("cancel"));
    await opened;
    expect(queryTabs()).toHaveLength(before);
  });
});
