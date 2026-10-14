import { create } from "zustand";
import { ipc, toIpcError, type MetricsBoard, type MetricsResult, type Value } from "@/lib/ipc";
import { normalizeObjects, type BoardObject } from "./model";

/** A board as the Metrics Board tab shows it: the saved board plus the edits not saved yet. */
export interface BoardDraft {
  id: string;
  /** The connection profile it belongs to; null = every connection. */
  profileId: string | null;
  name: string;
  objects: BoardObject[];
  createdAt: string;
  updatedAt: string;
  /** Edits wait for the (debounced) save. */
  dirty: boolean;
  /** The last save failed with this message. */
  saveError: string | null;
}

/** The last run of one object. */
export interface ObjectRun {
  status: "running" | "ok" | "error";
  result: MetricsResult | null;
  error: string | null;
  /** Scoreboard: its number before the latest refresh (the change is shown against it). */
  previous: number | null;
  /** When the last run finished (ms since the epoch). */
  at: number | null;
}

interface MetricsState {
  boards: Record<string, BoardDraft>;
  /** Board ids in list order. */
  order: string[];
  /** Board lists loaded per profile id: "ok", "loading" or the error. */
  loaded: Record<string, string>;
  /** Object runs by `runKey(board, object)`. */
  runs: Record<string, ObjectRun>;
  /** Variables set by inputs and dropdowns, per board. */
  vars: Record<string, Record<string, Value>>;
  /** What each input field shows, by `runKey` (kept while the tab is away). */
  texts: Record<string, string>;
}

export const useMetricsStore = create<MetricsState>()(() => ({
  boards: {},
  order: [],
  loaded: {},
  runs: {},
  vars: {},
  texts: {},
}));

export const runKey = (boardId: string, objectId: string): string => `${boardId}:${objectId}`;

/** Debounce of the automatic save after an edit (ms). */
export const SAVE_DELAY = 600;

function draftOf(board: MetricsBoard): BoardDraft {
  return {
    id: board.id,
    profileId: board.profileId,
    name: board.name,
    objects: normalizeObjects(board.objects),
    createdAt: board.createdAt,
    updatedAt: board.updatedAt,
    dirty: false,
    saveError: null,
  };
}

/** Takes boards from the backend; drafts with unsaved edits keep them. */
function absorb(list: readonly MetricsBoard[]): void {
  useMetricsStore.setState((s) => {
    const boards = { ...s.boards };
    for (const b of list) {
      const current = boards[b.id];
      if (current?.dirty) continue;
      boards[b.id] = draftOf(b);
    }
    const order = [...s.order];
    for (const b of list) if (!order.includes(b.id)) order.push(b.id);
    return { boards, order };
  });
}

/** The boards a connection's tab lists: its profile's and those of every connection. */
export function boardsFor(
  state: Pick<MetricsState, "boards" | "order">,
  profileId: string,
): BoardDraft[] {
  return state.order.flatMap((id) => {
    const b = state.boards[id];
    return b && (b.profileId === null || b.profileId === profileId) ? [b] : [];
  });
}

/** Loads (or reloads) the board list of a profile. */
export async function loadBoards(profileId: string): Promise<void> {
  useMetricsStore.setState((s) => ({ loaded: { ...s.loaded, [profileId]: "loading" } }));
  try {
    absorb(await ipc.metrics.listBoards(profileId));
    useMetricsStore.setState((s) => ({ loaded: { ...s.loaded, [profileId]: "ok" } }));
  } catch (err) {
    const message = toIpcError(err).message;
    useMetricsStore.setState((s) => ({ loaded: { ...s.loaded, [profileId]: message } }));
  }
}

/** Creates a board (saved at once) and returns it. */
export async function createBoard(
  profileId: string | null,
  name: string,
  objects: readonly BoardObject[] = [],
): Promise<BoardDraft> {
  const saved = await ipc.metrics.saveBoard({ id: null, profileId, name, objects });
  absorb([saved]);
  return useMetricsStore.getState().boards[saved.id] ?? draftOf(saved);
}

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** Saves in flight, so a later save waits for an earlier one (they never cross). */
const saving = new Map<string, Promise<void>>();

/** Saves a board's edits now (no-op without any). */
export function flushSave(boardId: string): Promise<void> {
  const timer = timers.get(boardId);
  if (timer) clearTimeout(timer);
  timers.delete(boardId);
  const previous = saving.get(boardId) ?? Promise.resolve();
  const next = previous.then(async () => {
    const draft = useMetricsStore.getState().boards[boardId];
    if (!draft?.dirty) return;
    const sent = draft;
    try {
      const saved = await ipc.metrics.saveBoard({
        id: sent.id,
        profileId: sent.profileId,
        name: sent.name,
        objects: sent.objects,
      });
      useMetricsStore.setState((s) => {
        const now = s.boards[boardId];
        if (!now) return s;
        // Edits made while saving stay dirty (and get saved next).
        const same = now.objects === sent.objects && now.name === sent.name;
        return {
          boards: {
            ...s.boards,
            [boardId]: { ...now, updatedAt: saved.updatedAt, dirty: !same, saveError: null },
          },
        };
      });
    } catch (err) {
      const message = toIpcError(err).message;
      useMetricsStore.setState((s) => {
        const now = s.boards[boardId];
        return now ? { boards: { ...s.boards, [boardId]: { ...now, saveError: message } } } : s;
      });
    }
  });
  saving.set(boardId, next);
  void next.finally(() => {
    if (saving.get(boardId) === next) saving.delete(boardId);
  });
  return next;
}

function scheduleSave(boardId: string): void {
  const timer = timers.get(boardId);
  if (timer) clearTimeout(timer);
  timers.set(
    boardId,
    setTimeout(() => void flushSave(boardId), SAVE_DELAY),
  );
}

function patchBoard(boardId: string, patch: (b: BoardDraft) => Partial<BoardDraft>): void {
  useMetricsStore.setState((s) => {
    const b = s.boards[boardId];
    if (!b) return s;
    return { boards: { ...s.boards, [boardId]: { ...b, ...patch(b), dirty: true } } };
  });
  scheduleSave(boardId);
}

/** Edits a board's objects (saved shortly after). */
export function updateObjects(
  boardId: string,
  fn: (objects: BoardObject[]) => BoardObject[],
): void {
  patchBoard(boardId, (b) => ({ objects: fn(b.objects) }));
}

/** Edits one object. */
export function updateObject(boardId: string, objectId: string, patch: Partial<BoardObject>): void {
  updateObjects(boardId, (objects) =>
    objects.map((o) => (o.id === objectId ? { ...o, ...patch } : o)),
  );
}

export async function renameBoard(boardId: string, name: string): Promise<void> {
  patchBoard(boardId, () => ({ name }));
  await flushSave(boardId);
}

/** Show on every connection (profile null) or only on this profile's. */
export async function setBoardProfile(boardId: string, profileId: string | null): Promise<void> {
  patchBoard(boardId, () => ({ profileId }));
  await flushSave(boardId);
}

export async function deleteBoard(boardId: string): Promise<void> {
  const timer = timers.get(boardId);
  if (timer) clearTimeout(timer);
  timers.delete(boardId);
  await ipc.metrics.deleteBoard(boardId);
  useMetricsStore.setState((s) => {
    const boards = { ...s.boards };
    delete boards[boardId];
    const prefix = `${boardId}:`;
    const runs = Object.fromEntries(Object.entries(s.runs).filter(([k]) => !k.startsWith(prefix)));
    const vars = { ...s.vars };
    delete vars[boardId];
    return { boards, order: s.order.filter((id) => id !== boardId), runs, vars };
  });
}

// ---- runtime: variables and runs --------------------------------------------------------------

export function setVariable(boardId: string, name: string, value: Value): void {
  useMetricsStore.setState((s) => ({
    vars: { ...s.vars, [boardId]: { ...s.vars[boardId], [name]: value } },
  }));
}

/** Sets the variables that are not set yet (the board's inputs' defaults). */
export function seedVariables(boardId: string, defaults: Readonly<Record<string, Value>>): void {
  const current = useMetricsStore.getState().vars[boardId] ?? {};
  const missing = Object.entries(defaults).filter(([name]) => !(name in current));
  if (missing.length === 0) return;
  useMetricsStore.setState((s) => ({
    vars: { ...s.vars, [boardId]: { ...Object.fromEntries(missing), ...s.vars[boardId] } },
  }));
}

export function setInputText(boardId: string, objectId: string, text: string): void {
  const key = runKey(boardId, objectId);
  useMetricsStore.setState((s) => ({ texts: { ...s.texts, [key]: text } }));
}

export function patchRun(boardId: string, objectId: string, patch: Partial<ObjectRun>): void {
  const key = runKey(boardId, objectId);
  useMetricsStore.setState((s) => {
    const prev: ObjectRun = s.runs[key] ?? {
      status: "running",
      result: null,
      error: null,
      previous: null,
      at: null,
    };
    return { runs: { ...s.runs, [key]: { ...prev, ...patch } } };
  });
}

/** Test-only: forget everything (and pending saves). */
export function resetMetricsStoreForTests(): void {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
  saving.clear();
  useMetricsStore.setState({ boards: {}, order: [], loaded: {}, runs: {}, vars: {}, texts: {} });
}
