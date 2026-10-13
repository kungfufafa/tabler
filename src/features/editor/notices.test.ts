import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useConsoleStore } from "@/features/console/store";
import { ipc, type QueryEvent, type StatementResult } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { EMPTY_QUERY_TAB, useQueryTabsStore } from "@/stores/queryTabs";
import { ResultStore } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import {
  installNoticeNotifications,
  MAX_NOTICE_TOASTS,
  newCursor,
  noticeText,
  takeNewNotices,
  toastNotices,
} from "./notices";
import { applyEvent, newRun, type RunNotice, type RunState } from "./runModel";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

const sink = new ResultStore();

const notice = (
  index: number | null,
  message: string,
  severity = "NOTICE",
  extra: Partial<RunNotice> = {},
): QueryEvent => ({
  event: "notice",
  data: { index, severity, message, detail: null, hint: null, context: null, ...extra },
});

function finished(index: number): QueryEvent {
  const result: StatementResult = {
    index,
    sql: "do $$ … $$",
    span: { start: 0, end: 10, line: 1 },
    kind: "empty",
    columns: [],
    resultSets: 0,
    rowCount: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: "2026-01-01T00:00:00.000Z",
    durationMs: 3,
    error: null,
    notices: [],
  };
  return { event: "finished", data: { index, result } };
}

const started = (index: number): QueryEvent => ({
  event: "started",
  data: { index, sql: "do $$ … $$", span: { start: 0, end: 10, line: 1 } },
});

function fold(events: QueryEvent[], run: RunState = newRun("r1", "do $$ … $$", 0)): RunState {
  return events.reduce((r, e) => applyEvent(r, e, sink), run);
}

beforeEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
  resetPrefsForTests();
});

describe("notices in the run model", () => {
  it("lists a statement's notices in order and keeps them when it finishes", () => {
    const run = fold([
      started(0),
      notice(0, "one"),
      notice(0, "two", "WARNING", { hint: "h" }),
      finished(0),
    ]);
    const st = run.statements[0]!;
    expect(st.status).toBe("ok");
    expect(st.notices?.map(noticeText)).toEqual(["NOTICE: one", "WARNING: two"]);
    expect(st.notices?.[1]?.hint).toBe("h");
  });

  it("keeps notices raised outside the statements apart", () => {
    const run = fold([started(0), notice(null, "from bootstrap"), finished(0)]);
    expect(run.outsideNotices?.map((n) => n.message)).toEqual(["from bootstrap"]);
    expect(run.statements[0]!.notices).toBeUndefined();
  });

  it("reports only the notices that arrived since the last look", () => {
    const cursor = newCursor();
    let run = fold([started(0), notice(0, "a")]);
    expect(takeNewNotices(run, cursor).statement.map((n) => n.message)).toEqual(["a"]);
    run = fold([notice(0, "b"), notice(null, "c")], run);
    const next = takeNewNotices(run, cursor);
    expect(next.statement.map((n) => n.message)).toEqual(["b"]);
    expect(next.outside.map((n) => n.message)).toEqual(["c"]);
    expect(takeNewNotices(run, cursor)).toEqual({ statement: [], outside: [] });
  });
});

describe("notice toasts", () => {
  const n = (message: string, severity = "NOTICE"): RunNotice => ({
    severity,
    message,
    detail: null,
    hint: null,
    context: null,
  });

  it("shows NOTICE / WARNING / INFO, not LOG or DEBUG", () => {
    const cursor = newCursor();
    toastNotices("r", [n("hi"), n("careful", "WARNING"), n("x", "LOG")], cursor);
    expect(toast.message).toHaveBeenCalledWith("NOTICE: hi", {});
    expect(toast.warning).toHaveBeenCalledWith("WARNING: careful", {});
    expect(toast.message).toHaveBeenCalledTimes(1);
  });

  it("caps the toasts of a run and sums up the rest", () => {
    const cursor = newCursor();
    const many = Array.from({ length: MAX_NOTICE_TOASTS + 3 }, (_, i) => n(`m${i}`));
    toastNotices("r", many, cursor);
    expect(toast.message).toHaveBeenCalledTimes(MAX_NOTICE_TOASTS + 1);
    expect(toast.message).toHaveBeenLastCalledWith("3 more notices in Messages", {
      id: "notices-r",
    });
  });

  it("puts DETAIL and HINT in the description", () => {
    toastNotices("r", [{ ...n("m"), detail: "d", hint: "h" }], newCursor());
    expect(toast.message).toHaveBeenCalledWith("NOTICE: m", { description: "DETAIL: d\nHINT: h" });
  });
});

describe("notices while a run is in progress", () => {
  installNoticeNotifications();
  let tabId = "";
  beforeEach(() => {
    useQueryTabsStore.setState({ slices: {} });
    useConsoleStore.getState().clear();
    tabId = useTabsStore.getState().newQuery("c-notices");
  });

  const show = (run: RunState) =>
    useQueryTabsStore.setState((s) => ({
      slices: { ...s.slices, [tabId]: { ...EMPTY_QUERY_TAB, run } },
    }));

  it("toasts each notice as it arrives, and outside notices go to the console", () => {
    let run = fold([started(0)]);
    show(run);
    run = fold([notice(0, "first")], run);
    show(run);
    expect(toast.message).toHaveBeenCalledWith("NOTICE: first", {});
    run = fold([notice(null, "bootstrap"), notice(0, "second"), finished(0)], run);
    run = fold([{ event: "done", data: { totalMs: 5 } }], run);
    show(run);
    expect(toast.message).toHaveBeenCalledWith("NOTICE: second", {});
    expect(toast.message).toHaveBeenCalledTimes(2);
    expect(useConsoleStore.getState().entries).toEqual([
      expect.objectContaining({
        connectionId: "c-notices",
        sql: "-- NOTICE: bootstrap",
        meta: true,
        label: "NOTICE",
      }),
    ]);
  });

  it("stays quiet when Raise Notice notifications are off, or for a finished run", () => {
    setPref("editorRaiseNotices", false);
    const run = fold([started(0)]);
    show(run);
    show(fold([notice(0, "quiet")], run));
    expect(toast.message).not.toHaveBeenCalled();
    setPref("editorRaiseNotices", true);
    const done = fold(
      [started(0), notice(0, "old"), finished(0), { event: "done", data: { totalMs: 1 } }],
      newRun("r2", "x", 0),
    );
    show(done);
    expect(toast.message).not.toHaveBeenCalled();
  });
});

describe("mock RAISE", () => {
  let connectionId = "";
  beforeAll(async () => {
    connectionId = (await ipc.connections.connect("p-local-pg")).connectionId;
  });
  afterAll(async () => {
    await ipc.connections.disconnect(connectionId);
  });
  beforeEach(() => resetMockQueryState({ emptyHistory: true }));

  async function run(sql: string) {
    const events: QueryEvent[] = [];
    const results = await ipc.query.run(connectionId, crypto.randomUUID(), sql, {}, (e) =>
      events.push(e),
    );
    return { events, results };
  }

  it("streams a notice for RAISE NOTICE / WARNING", async () => {
    const { events } = await run(
      "DO $$ BEGIN RAISE NOTICE 'hi %', 7; RAISE WARNING 'careful' USING HINT = 'look'; END $$;",
    );
    expect(events.map((e) => e.event)).toEqual(["started", "notice", "notice", "finished", "done"]);
    expect(events[1]).toMatchObject({
      event: "notice",
      data: { index: 0, severity: "NOTICE", message: "hi 7", hint: null },
    });
    expect(events[2]).toMatchObject({
      data: { severity: "WARNING", message: "careful", hint: "look" },
    });
  });

  it("fails on RAISE EXCEPTION after the notices before it", async () => {
    const { events } = await run(
      "DO $$ BEGIN RAISE NOTICE 'before'; RAISE EXCEPTION 'boom'; END $$",
    );
    expect(events.map((e) => e.event)).toEqual(["started", "notice", "failed", "done"]);
    expect(events[2]).toMatchObject({ data: { error: { message: "boom", sqlstate: "P0001" } } });
  });
});
