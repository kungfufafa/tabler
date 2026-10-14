import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runQuery } from "@/features/editor/controller";
import { dropEditor, seedEditor } from "@/features/editor/editorStates";
import type { RunState } from "@/features/editor/runModel";
import { setCell } from "@/features/staging/model";
import { ipc, type ConnectionInfo, type ResultColumn, type Value } from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests } from "@/stores/prefs";
import { getQueryTab, updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { dropResults, resultData } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { bufferKey } from "./changes";
import { RerunConfirmDialog } from "./RerunConfirmDialog";
import { answerRerunConfirm, confirmRerunDiscard, useRerunConfirmStore } from "./rerunConfirm";
import {
  installResultsLifecycle,
  pendingForRun,
  pendingForTab,
  pinResult,
  setResultStaging,
  useResultsStore,
} from "./store";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

const TAB = "q1";
const CONN = "c1";
const RESULT = "run1:0:0";
const text = (v: string): Value => ({ t: "text", v });
const columns: ResultColumn[] = [
  { name: "email", dataType: "text", kind: "text", nullable: null, table: null, origin: null },
];

function runState(): RunState {
  const sql = "select email from users";
  return {
    id: "run1",
    sql,
    baseOffset: 0,
    startedAt: 0,
    finishedAt: 1,
    totalMs: 1,
    status: "done",
    statements: [],
    results: [
      {
        id: RESULT,
        statementIndex: 0,
        set: 0,
        columns,
        done: true,
        truncated: false,
        durationMs: 1,
      },
    ],
    error: null,
    cancelling: false,
  };
}

const stageEdit = () =>
  setResultStaging(TAB, RESULT, (s) =>
    setCell(s, bufferKey(0), 0, text("new@x"), resultData.get(RESULT)!.rows[0]),
  );

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  installResultsLifecycle();
  resetPrefsForTests();
  useRerunConfirmStore.setState({ request: null });
  useResultsStore.setState({ edits: {}, editability: {}, pins: {} });
  useConnectionsStore.setState({
    byId: { [CONN]: { info: { connectionId: CONN, driver: "postgres" } as ConnectionInfo } },
  } as never);
  useTabsStore.setState({
    tabs: [{ id: TAB, kind: "query", connectionId: CONN, title: "Query 1" }],
    activeId: TAB,
  });
  useQueryTabsStore.setState({ slices: {} });
  seedEditor(TAB, "select email from users");
  updateQueryTab(TAB, { run: runState(), pane: RESULT });
  dropResults(() => true);
  resultData.setColumns(RESULT, columns);
  resultData.append(RESULT, [[text("old@x")]]);
  stageEdit();
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
  dropEditor(TAB);
});

describe("re-running a query whose results hold uncommitted edits", () => {
  it("counts the edits of the run, not the pinned ones", () => {
    const st = useResultsStore.getState();
    expect(pendingForRun(st, TAB, "run1")).toBe(1);
    expect(pendingForRun(st, TAB, "run0")).toBe(0);
    pinResult(TAB, runState().results[0]!, "Result 1", null);
    expect(pendingForRun(useResultsStore.getState(), TAB, "run1")).toBe(0);
    expect(pendingForTab(useResultsStore.getState(), TAB)).toBe(1);
  });

  it("asks first; Cancel leaves the edits and runs nothing", async () => {
    const run = vi.spyOn(ipc.query, "run").mockResolvedValue([]);
    const done = runQuery(TAB, "auto");
    await flush();
    expect(useRerunConfirmStore.getState().request).toMatchObject({ tabId: TAB, count: 1 });
    expect(run).not.toHaveBeenCalled();
    answerRerunConfirm(false);
    await done;
    expect(run).not.toHaveBeenCalled();
    expect(getQueryTab(TAB).run?.id).toBe("run1");
    expect(pendingForTab(useResultsStore.getState(), TAB)).toBe(1);
  });

  it("Discard & Run drops the edits quietly and runs", async () => {
    const run = vi.spyOn(ipc.query, "run").mockResolvedValue([]);
    const done = runQuery(TAB, "current");
    await flush();
    answerRerunConfirm(true);
    await done;
    expect(run).toHaveBeenCalledOnce();
    expect(getQueryTab(TAB).run?.id).not.toBe("run1");
    expect(pendingForTab(useResultsStore.getState(), TAB)).toBe(0);
    // Agreed to: no "were discarded" toast on top.
    expect(toast.message).not.toHaveBeenCalledWith(
      expect.stringContaining("discarded"),
      expect.anything(),
    );
  });

  it("does not ask without edits, nor for a run started elsewhere (Re-run), which toasts", async () => {
    const run = vi.spyOn(ipc.query, "run").mockResolvedValue([]);
    await runQuery(TAB, { sql: "select email from users", baseOffset: 0 });
    expect(useRerunConfirmStore.getState().request).toBeNull();
    expect(run).toHaveBeenCalledOnce();
    expect(toast.message).toHaveBeenCalledWith(
      "1 uncommitted result change was discarded",
      expect.anything(),
    );
    // Nothing staged any more: the next run just runs.
    await runQuery(TAB, "auto");
    expect(useRerunConfirmStore.getState().request).toBeNull();
    expect(run).toHaveBeenCalledTimes(2);
  });
});

describe("RerunConfirmDialog", () => {
  const mod = isMac ? { metaKey: true } : { ctrlKey: true };

  it("asks with the count; ⌘↩ confirms", async () => {
    render(<RerunConfirmDialog />);
    let answer: Promise<boolean> = Promise.resolve(false);
    act(() => void (answer = confirmRerunDiscard(TAB, 3)));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Discard 3 uncommitted result changes and run?");
    fireEvent.keyDown(dialog, { key: "Enter", code: "Enter", ...mod });
    await expect(answer).resolves.toBe(true);
    expect(useRerunConfirmStore.getState().request).toBeNull();
  });

  it("Cancel (or a newer question) answers no", async () => {
    render(<RerunConfirmDialog />);
    let first: Promise<boolean> = Promise.resolve(true);
    let second: Promise<boolean> = Promise.resolve(true);
    act(() => void (first = confirmRerunDiscard(TAB, 1)));
    act(() => void (second = confirmRerunDiscard(TAB, 2)));
    await expect(first).resolves.toBe(false);
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await expect(second).resolves.toBe(false);
  });
});
