import { useCallback, useEffect, useMemo, useRef } from "react";
import { toast } from "sonner";
import { ipc, toIpcError, type Dialect } from "@/lib/ipc";
import { runsQuery, type BoardObject } from "./model";
import { defaultVariables, dropdownOptions, initialOption, scoreNumber, valueKey } from "./results";
import { runBoardQuery } from "./runner";
import { RefreshScheduler, type RunTrigger, type Scheduled } from "./scheduler";
import { patchRun, runKey, seedVariables, setVariable, useMetricsStore } from "./store";

export interface BoardRuntime {
  /** An input / dropdown changed or a button was clicked: its wired objects refresh. */
  fire: (sourceId: string) => void;
  /** Runs one object now. */
  refresh: (objectId: string) => void;
  /** Runs every object that has a query (⌘R). */
  refreshAll: () => void;
  /** A button: runs its SQL (if any), then refreshes the objects wired to it. */
  click: (buttonId: string) => Promise<void>;
}

const EMPTY: BoardObject[] = [];
let seq = 0;

/**
 * What the scheduler is told of the board. Buttons are in it too: `runsQuery` is false for them
 * (the scheduler never runs a button or arms a timer for it), but `fire` finds the objects wired
 * to a clicked button only through its entry.
 */
const scheduled = (objects: readonly BoardObject[]): Scheduled[] =>
  objects.map((o) => ({ id: o.id, refresh: o.refresh, runs: runsQuery(o), targets: o.targets }));

function errorText(err: unknown): string {
  return err instanceof Error && err.name === "BoardQueryError"
    ? err.message
    : toIpcError(err).message;
}

/**
 * Runs a board's objects on a connection while its tab is shown (`RefreshScheduler`): results,
 * errors and variables live in the metrics store, so they survive switching tabs. Running
 * queries are cancelled when the tab goes away.
 */
export function useBoardRuntime(
  connectionId: string,
  boardId: string,
  dialect: Dialect,
): BoardRuntime {
  const objects = useMetricsStore((s) => s.boards[boardId]?.objects ?? EMPTY);
  const scheduler = useRef<RefreshScheduler | null>(null);
  const inflight = useRef(new Map<string, string>());

  const objectOf = useCallback(
    (id: string) => useMetricsStore.getState().boards[boardId]?.objects.find((o) => o.id === id),
    [boardId],
  );

  /** A dropdown got its options: keep its value if it is one of them, else take the first. */
  const settleDropdown = useCallback(
    (o: BoardObject) => {
      const state = useMetricsStore.getState();
      const result = state.runs[runKey(boardId, o.id)]?.result ?? null;
      const options = dropdownOptions(o, result);
      const current = state.vars[boardId]?.[o.variable];
      if (current && options.some((opt) => valueKey(opt.value) === valueKey(current))) return;
      setVariable(boardId, o.variable, initialOption(o, options));
      // Nobody chose that option: what it refreshes is an automatic run (a write is not run).
      scheduler.current?.fire(o.id, "open");
    },
    [boardId],
  );

  const run = useCallback(
    async (id: string, trigger: RunTrigger) => {
      const o = objectOf(id);
      if (!o) return;
      const before = useMetricsStore.getState().runs[runKey(boardId, id)];
      patchRun(boardId, id, { status: "running" });
      const queryId = `metrics:${id}:${++seq}`;
      inflight.current.set(id, queryId);
      try {
        const result = await runBoardQuery({
          connectionId,
          dialect,
          sql: o.sql,
          values: useMetricsStore.getState().vars[boardId] ?? {},
          allowWrites: o.allowWrites,
          timeoutSecs: o.timeoutSecs,
          queryId,
          quiet: trigger === "interval" || trigger === "open" || trigger === "event",
          // The board opening, a timer and a wired input are not the user asking for a write.
          automatic: trigger === "open" || trigger === "interval",
        });
        const previous =
          o.kind === "scoreboard" ? scoreNumber(before?.result, o.valueColumn) : null;
        patchRun(boardId, id, { status: "ok", result, error: null, previous, at: Date.now() });
        if (o.kind === "dropdown" && o.variable) settleDropdown(o);
      } catch (err) {
        patchRun(boardId, id, {
          status: "error",
          result: null,
          error: errorText(err),
          at: Date.now(),
        });
      } finally {
        if (inflight.current.get(id) === queryId) inflight.current.delete(id);
      }
    },
    [boardId, connectionId, dialect, objectOf, settleDropdown],
  );

  const objectsRef = useRef(objects);
  objectsRef.current = objects;

  useEffect(() => {
    const s = new RefreshScheduler(run);
    scheduler.current = s;
    const current = objectsRef.current;
    seedVariables(boardId, defaultVariables(current));
    // Objects already showing a result (the tab was away) only refresh on their own schedule.
    const runs = useMetricsStore.getState().runs;
    s.start(scheduled(current), (id) => {
      const last = runs[runKey(boardId, id)];
      const o = current.find((x) => x.id === id);
      return !last || last.status !== "ok" || o?.refresh.mode === "interval";
    });
    const pending = inflight.current;
    return () => {
      s.stop();
      if (scheduler.current === s) scheduler.current = null;
      for (const queryId of pending.values())
        void ipc.query.cancel(connectionId, queryId).catch(() => {});
      pending.clear();
    };
  }, [boardId, connectionId, run]);

  useEffect(() => {
    seedVariables(boardId, defaultVariables(objects));
    scheduler.current?.update(scheduled(objects));
  }, [boardId, objects]);

  const click = useCallback(
    async (buttonId: string) => {
      const o = objectOf(buttonId);
      if (!o) return;
      if (o.sql.trim() !== "") {
        patchRun(boardId, buttonId, { status: "running", error: null });
        // Tracked like a scheduled run: closing the tab cancels it.
        const queryId = `metrics:${buttonId}:${++seq}`;
        inflight.current.set(buttonId, queryId);
        try {
          const result = await runBoardQuery({
            connectionId,
            dialect,
            sql: o.sql,
            values: useMetricsStore.getState().vars[boardId] ?? {},
            allowWrites: o.allowWrites,
            timeoutSecs: o.timeoutSecs,
            queryId,
          });
          patchRun(boardId, buttonId, { status: "ok", result, error: null, at: Date.now() });
          if (result.affected !== null)
            toast.success(
              `${o.title}: ${result.affected} row${result.affected === 1 ? "" : "s"} affected`,
            );
        } catch (err) {
          const message = errorText(err);
          patchRun(boardId, buttonId, { status: "error", error: message, at: Date.now() });
          toast.error(`${o.title} failed`, { description: message });
          return;
        } finally {
          if (inflight.current.get(buttonId) === queryId) inflight.current.delete(buttonId);
        }
      }
      scheduler.current?.fire(buttonId);
    },
    [boardId, connectionId, dialect, objectOf],
  );

  return useMemo(
    () => ({
      fire: (id) => scheduler.current?.fire(id),
      refresh: (id) => scheduler.current?.refresh(id),
      refreshAll: () => scheduler.current?.refreshAll(),
      click,
    }),
    [click],
  );
}
