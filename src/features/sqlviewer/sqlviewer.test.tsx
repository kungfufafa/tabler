import { QueryClient } from "@tanstack/react-query";
import { act, fireEvent, render as rtlRender, screen, waitFor } from "@testing-library/react";
import type { ReactElement } from "react";
import { toast } from "sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { answerLargeFile, useLargeFileStore } from "@/features/editor/largeFile";
import { actions } from "@/features/palette/actions";
import { TabBody } from "@/features/tabs/TabContent";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import "@/lib/ipc/mock";
import { mockFileText, resetMockQueryState } from "@/lib/ipc/mock/query";
import { mockRecentItems, resetMockRecent } from "@/lib/ipc/mock/recent";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests } from "@/stores/prefs";
import { currentSession, resetSessionForTests, setRemembered } from "@/stores/sessionPersist";
import { installConnectRestore, resetSessionRestoreForTests } from "@/stores/sessionRestore";
import { hasUnsaved, unsavedCount, useTabsStore, type ToolTab } from "@/stores/tabs";
import { handleQuitRequest, subscribeUnsaved } from "@/hooks/useUnsavedGuard";
import { useUiStore } from "@/stores/ui";
import { getViewerView, viewerText } from "./buffers";
import { closeStandalone, dropClosedViewers } from "./lifecycle";
import {
  defaultDialect,
  keepsViewerTab,
  openPathFromViewerTab,
  openSqlViewer,
  restoreViewerParams,
  SQL_VIEWER_TOOL,
} from "./open";
import { SqlViewerHost } from "./SqlViewerHost";
import { STANDALONE, getDoc, resetSqlViewerStore, useSqlViewerStore } from "./store";

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
});

const SAMPLE = "/Users/me/queries/sample.sql";

const render = (ui: ReactElement) => rtlRender(ui, { wrapper: TooltipProvider });

const info = (id: string): ConnectionInfo => ({
  connectionId: id,
  profileId: `p-${id}`,
  name: id,
  color: null,
  driver: "mysql",
  serverVersion: "9.6",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
});

const viewerTabs = () =>
  useTabsStore.getState().tabs.filter((t): t is ToolTab => t.kind === "tool");

/** Types into the viewer's editor as the user would (one transaction). */
function type(key: string, text: string) {
  const view = getViewerView(key)!;
  act(() => view.dispatch({ changes: { from: view.state.doc.length, insert: text } }));
}

beforeEach(() => {
  resetSqlViewerStore();
  resetMockQueryState();
  resetMockRecent();
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useUiStore.setState({ view: "launcher" });
});
afterEach(() => {
  dropClosedViewers(new Set());
  closeStandalone();
  vi.restoreAllMocks();
});

describe("params and defaults", () => {
  it("restores only viewers of a file, and highlights like the connection", () => {
    expect(restoreViewerParams({ path: "/a.sql" })).toEqual({ path: "/a.sql" });
    expect(restoreViewerParams({ path: null })).toBeNull();
    expect(restoreViewerParams({ path: " " })).toBeNull();
    expect(restoreViewerParams(null)).toBeNull();
    expect(defaultDialect("mysql")).toBe("mysql");
    expect(defaultDialect("sqlite")).toBe("sqlite");
    expect(defaultDialect(undefined)).toBe("postgres");
  });
});

describe("params and defaults: what the session keeps", () => {
  it("keeps only viewers of a file, the same test the restore makes", () => {
    expect(keepsViewerTab({ path: "/a.sql" })).toBe(true);
    expect(keepsViewerTab({ path: null })).toBe(false);
    expect(keepsViewerTab({ path: "  " })).toBe(false);
    expect(keepsViewerTab(null)).toBe(false);
    for (const params of [{ path: "/a.sql" }, { path: null }, { path: "" }, null])
      expect(keepsViewerTab(params), JSON.stringify(params)).toBe(
        restoreViewerParams(params) !== null,
      );
  });
});

describe("the standalone viewer (no connection open)", () => {
  it("opens a file, saves it and asks before closing with unsaved changes", async () => {
    render(<SqlViewerHost />);
    act(() => openSqlViewer(SAMPLE));
    expect(await screen.findByRole("dialog", { name: "SQL Viewer" })).toBeInTheDocument();
    await waitFor(() => expect(getDoc(STANDALONE)).toMatchObject({ path: SAMPLE, loading: false }));
    expect(viewerText(STANDALONE)).toBe(mockFileText(SAMPLE));
    expect(screen.getByText("sample.sql")).toBeInTheDocument();
    await waitFor(() => expect(mockRecentItems().files).toEqual([SAMPLE]));

    type(STANDALONE, "-- more\n");
    expect(getDoc(STANDALONE)?.dirty).toBe(true);
    expect(screen.getByLabelText("Edited")).toBeInTheDocument();

    // ⌘W asks first; Cancel keeps it open.
    await act(() => actions.run("tab.close"));
    expect(screen.getByRole("alert")).toHaveTextContent("Save the changes to sample.sql");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    // ⌘S writes the file (the browser mock's file system).
    await act(() => actions.run("query.save"));
    expect(mockFileText(SAMPLE)?.endsWith("-- more\n")).toBe(true);
    expect(getDoc(STANDALONE)?.dirty).toBe(false);
    await act(() => actions.run("tab.close"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(getDoc(STANDALONE)).toBeUndefined();
  });

  it("says above the text that the first 5 MB of a big file is a copy, not the file", async () => {
    const BIG = "/Users/me/dumps/huge-dump.sql";
    const MB = 1024 * 1024;
    vi.spyOn(ipc.sqlFiles, "info").mockResolvedValue({ bytes: 50 * MB, compressed: false });
    vi.spyOn(ipc.sqlFiles, "readHead").mockResolvedValue({
      text: "INSERT INTO t VALUES (1);\n",
      totalBytes: 50 * MB,
      truncated: true,
      compressed: false,
      encoding: null,
    });
    render(<SqlViewerHost />);
    act(() => openSqlViewer(BIG));
    await screen.findByRole("dialog", { name: "SQL Viewer" });
    await waitFor(() => expect(useLargeFileStore.getState().request).not.toBeNull());
    // Nothing is shown before the answer, and a file that opens whole never shows the notice.
    expect(screen.queryByTestId("head-notice")).toBeNull();
    answerLargeFile("head");

    const notice = await screen.findByTestId("head-notice");
    expect(notice).toHaveAttribute("role", "status");
    expect(notice).toHaveTextContent(
      "Showing the first 5 MB of huge-dump.sql; saving here will not overwrite the file",
    );
    expect(getDoc(STANDALONE)).toMatchObject({ path: null, headOf: BIG });
    expect(viewerText(STANDALONE)).toBe("INSERT INTO t VALUES (1);\n");
  });

  it("shows no notice for a file that opens whole", async () => {
    render(<SqlViewerHost />);
    act(() => openSqlViewer(SAMPLE));
    await waitFor(() => expect(getDoc(STANDALONE)).toMatchObject({ path: SAMPLE, loading: false }));
    expect(screen.queryByTestId("head-notice")).toBeNull();
  });

  it("keeps unsaved text until the user discards it for another file", async () => {
    render(<SqlViewerHost />);
    act(() => openSqlViewer());
    await screen.findByRole("dialog", { name: "SQL Viewer" });
    expect(getDoc(STANDALONE)).toMatchObject({ path: null, loading: false });
    type(STANDALONE, "select 1;");
    act(() => openSqlViewer(SAMPLE));
    expect(screen.getByRole("alert")).toHaveTextContent("Discard the unsaved changes to Untitled");
    expect(viewerText(STANDALONE)).toBe("select 1;");
    fireEvent.click(screen.getByRole("button", { name: "Discard and Open" }));
    await waitFor(() => expect(getDoc(STANDALONE)?.path).toBe(SAMPLE));
    expect(getDoc(STANDALONE)?.dirty).toBe(false);
  });

  it("saves an untitled buffer under the name the user picks", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/queries/new");
    render(<SqlViewerHost />);
    act(() => openSqlViewer());
    await screen.findByRole("dialog", { name: "SQL Viewer" });
    type(STANDALONE, "select 2;");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(getDoc(STANDALONE)?.path).toBe("/Users/me/queries/new.sql"));
    expect(mockFileText("/Users/me/queries/new.sql")).toBe("select 2;");
  });

  it("opens File ▸ Open SQL File… in the viewer while no connection is open", async () => {
    vi.spyOn(window, "prompt").mockReturnValue(SAMPLE);
    render(<SqlViewerHost />);
    expect(actions.has("query.open")).toBe(true);
    await act(() => actions.run("query.open"));
    await waitFor(() => expect(getDoc(STANDALONE)?.path).toBe(SAMPLE));
    expect(useSqlViewerStore.getState().standaloneOpen).toBe(true);
  });
});

describe("the standalone viewer and the window's quit guard", () => {
  it("counts unsaved text as unsaved work: the window close and ⌘Q ask first", async () => {
    render(<SqlViewerHost />);
    act(() => openSqlViewer(SAMPLE));
    await waitFor(() => expect(getDoc(STANDALONE)).toMatchObject({ path: SAMPLE, loading: false }));
    // Nothing edited: nothing to ask about.
    expect(hasUnsaved()).toBe(false);
    expect(useTabsStore.getState().requestWindowClose()).toBe(true);

    // The guard's reporter hears the change (it tells the backend, so a Dock quit asks too).
    const heard = vi.fn();
    const stop = subscribeUnsaved(heard);
    type(STANDALONE, "-- more\n");
    expect(heard).toHaveBeenCalled();
    stop();
    expect(hasUnsaved()).toBe(true);

    const quit = vi.fn(() => Promise.resolve());
    handleQuitRequest(undefined, quit, () => Promise.resolve());
    expect(quit).not.toHaveBeenCalled();
    const request = useTabsStore.getState().closeRequest;
    expect(request).toMatchObject({ kind: "window", total: 1 });
    expect(request?.dirty).toEqual([
      { id: "window:sql-viewer", kind: "query", title: "sample.sql", count: 1 },
    ]);
    useTabsStore.getState().cancelClose();

    // Saved: the window may close again.
    await act(() => actions.run("query.save"));
    expect(hasUnsaved()).toBe(false);
    handleQuitRequest(undefined, quit, () => Promise.resolve());
    expect(quit).toHaveBeenCalledOnce();
  });

  it("stops counting once the viewer is closed, and with the host gone", async () => {
    const { unmount } = render(<SqlViewerHost />);
    act(() => openSqlViewer());
    await screen.findByRole("dialog", { name: "SQL Viewer" });
    type(STANDALONE, "select 1;");
    expect(hasUnsaved()).toBe(true);
    act(() => closeStandalone());
    expect(hasUnsaved()).toBe(false);
    act(() => openSqlViewer());
    await screen.findByRole("dialog", { name: "SQL Viewer" });
    type(STANDALONE, "select 2;");
    expect(hasUnsaved()).toBe(true);
    unmount();
    expect(hasUnsaved()).toBe(false);
  });
});

describe("viewer tabs (a connection is open)", () => {
  beforeEach(() => useConnectionsStore.getState().add(info("c1")));

  it("opens a tab per ⇧⌘O and shows the tab of a file that is open already", () => {
    openSqlViewer();
    openSqlViewer();
    openSqlViewer(SAMPLE);
    openSqlViewer(SAMPLE);
    expect(viewerTabs().map((t) => [t.tool, t.title, t.params])).toEqual([
      [SQL_VIEWER_TOOL, "SQL Viewer", { path: null }],
      [SQL_VIEWER_TOOL, "SQL Viewer", { path: null }],
      [SQL_VIEWER_TOOL, "sample.sql", { path: SAMPLE }],
    ]);
    expect(useTabsStore.getState().activeId).toBe(viewerTabs()[2]!.id);
  });

  it("reads its file, follows Save As in its title and counts as unsaved work", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/queries/copy.sql");
    render(<SqlViewerHost />);
    openSqlViewer(SAMPLE);
    const tab = viewerTabs()[0]!;
    const { rerender } = render(<TabBody tab={tab} />);
    await waitFor(() => expect(viewerText(tab.id)).toBe(mockFileText(SAMPLE)));
    // The connection is MySQL: the viewer highlights MySQL.
    expect(getDoc(tab.id)?.dialect).toBe("mysql");
    expect(unsavedCount(tab)).toBe(0);
    type(tab.id, "-- edited\n");
    expect(unsavedCount(tab)).toBe(1);

    await act(() => actions.run("query.save-as"));
    await waitFor(() =>
      expect(viewerTabs()[0]).toMatchObject({
        title: "copy.sql",
        params: { path: "/Users/me/queries/copy.sql" },
      }),
    );
    rerender(<TabBody tab={viewerTabs()[0]!} />);
    expect(unsavedCount(viewerTabs()[0]!)).toBe(0);
    expect(mockFileText("/Users/me/queries/copy.sql")?.endsWith("-- edited\n")).toBe(true);
  });

  it("opens a picked file in place, in a new tab over unsaved changes, or shows its tab", async () => {
    openSqlViewer();
    const untitled = viewerTabs()[0]!;
    render(<TabBody tab={untitled} />);
    await waitFor(() => expect(getViewerView(untitled.id)).not.toBeNull());
    type(untitled.id, "select 3;");
    openPathFromViewerTab(untitled.id, SAMPLE);
    expect(viewerTabs()).toHaveLength(2);
    expect(viewerText(untitled.id)).toBe("select 3;");
    const fileTab = viewerTabs()[1]!;
    // Picking it again from the first viewer shows its tab.
    useTabsStore.getState().activate(untitled.id);
    openPathFromViewerTab(untitled.id, SAMPLE);
    expect(useTabsStore.getState().activeId).toBe(fileTab.id);
  });

  it("saves a viewer of a file in the session, not an untitled one that could never come back", async () => {
    resetPrefsForTests();
    resetSessionForTests();
    resetSessionRestoreForTests();
    const warning = vi.spyOn(toast, "warning");
    try {
      openSqlViewer();
      openSqlViewer(SAMPLE);
      expect(viewerTabs()).toHaveLength(2);
      const saved = currentSession().workspaces.find((w) => w.profileId === "p-c1");
      expect(saved?.tabs).toEqual([
        { kind: "tool", tool: SQL_VIEWER_TOOL, title: "sample.sql", params: { path: SAMPLE } },
      ]);

      // The next start: the connection is back, its remembered tabs reopen, and nothing is
      // reported as "No longer there".
      setRemembered([{ ...saved!, open: false }]);
      installConnectRestore(new QueryClient());
      useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
      useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
      useConnectionsStore.getState().add(info("c1"));
      await waitFor(() => expect(viewerTabs()).toHaveLength(1));
      expect(viewerTabs()[0]).toMatchObject({
        tool: SQL_VIEWER_TOOL,
        title: "sample.sql",
        params: { path: SAMPLE },
      });
      // Let the restore finish before looking for the warning.
      await new Promise((r) => setTimeout(r, 20));
      expect(warning).not.toHaveBeenCalled();
    } finally {
      resetSessionRestoreForTests();
      resetSessionForTests();
    }
  });

  it("forgets the buffers of closed viewer tabs", async () => {
    openSqlViewer();
    const tab = viewerTabs()[0]!;
    render(<SqlViewerHost />);
    render(<TabBody tab={tab} />);
    await waitFor(() => expect(getDoc(tab.id)).toBeDefined());
    act(() => useTabsStore.getState().close(tab.id));
    expect(getDoc(tab.id)).toBeUndefined();
  });
});
