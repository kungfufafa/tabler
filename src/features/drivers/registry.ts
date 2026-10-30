import { useEffect } from "react";
import { create } from "zustand";
import type { AppInfo, DatabaseKind, Driver, SqlDialectId } from "@/lib/ipc";
import { useAppInfoStore } from "@/stores/appInfo";
import { visibleKinds } from "./catalog";
import { LANE_IDS, LANES, laneOfDialect, laneOfDriver, laneOfKind, type LaneId } from "./lanes";
import type { DialectUi, DriverUi, KindUi, LaneModule } from "./types";

export { visibleKinds } from "./catalog";

/**
 * The renderer's driver registry (plan §2.10, critique A11): each lane's `LaneModule` (its
 * folder's `index.ts`) is imported lazily, the first time a database, driver or dialect of the
 * lane is asked for, so a lane's error breaks only that lane (A24). The core drivers have no lane:
 * for them, and for a lane this build lacks (`AppInfo.kinds`, the core ones before the backend
 * answers and in tests), every lookup answers null at once, so the core path stays synchronous.
 * While a lane loads its lookups answer undefined; once it failed they answer null (and the
 * error is logged), and the awaiting forms reject with the import's error.
 */

type Loader = () => Promise<{ default: LaneModule }>;

// Not eager: a lane module is fetched (and parsed) only when the lane is used.
const LOADERS: Partial<Record<LaneId, Loader>> = Object.fromEntries(
  Object.entries(import.meta.glob<{ default: LaneModule }>("./*/index.ts")).flatMap(
    ([file, load]) => {
      const lane = LANE_IDS.find((id) => file === `./${id}/index.ts`);
      return lane ? [[lane, load]] : [];
    },
  ),
);

/** The lanes whose folder has a module (every lane: F9a gave each a skeleton). */
export const LANES_WITH_MODULE: readonly LaneId[] = LANE_IDS.filter((lane) => LOADERS[lane]);

type LaneState =
  | { status: "loading"; promise: Promise<LaneModule> }
  | { status: "loaded"; module: LaneModule }
  | { status: "failed"; error: unknown };

const useLanes = create<{ lanes: Partial<Record<LaneId, LaneState>> }>()(() => ({ lanes: {} }));

/** Overrides of `registerLaneOverride` (tests): a module, or a loader used in place of the glob's. */
const overrides = new Map<LaneId, LaneModule | Loader>();

const setLane = (lane: LaneId, state: LaneState | undefined) =>
  useLanes.setState((s) => ({ lanes: { ...s.lanes, [lane]: state } }));

/** A lane's module, imported once (a failed import is not retried until an override resets it). */
export function importLane(lane: LaneId): Promise<LaneModule> {
  const state = useLanes.getState().lanes[lane];
  if (state?.status === "loading") return state.promise;
  if (state?.status === "loaded") return Promise.resolve(state.module);
  if (state?.status === "failed") return Promise.reject(state.error);
  const override = overrides.get(lane);
  let promise: Promise<LaneModule>;
  if (override !== undefined && typeof override !== "function") promise = Promise.resolve(override);
  else {
    const load = override ?? LOADERS[lane];
    promise = load ? load().then((m) => m.default ?? {}) : Promise.resolve({});
  }
  const loading: LaneState = { status: "loading", promise };
  setLane(lane, loading);
  // An override registered meanwhile starts over: this load no longer answers for the lane.
  const settle = (next: LaneState) => {
    if (useLanes.getState().lanes[lane] === loading) setLane(lane, next);
  };
  promise.then(
    (module) => settle({ status: "loaded", module }),
    (error: unknown) => {
      console.error(`The ${lane} driver lane failed to load`, error);
      settle({ status: "failed", error });
    },
  );
  return promise;
}

/**
 * Tests only: `lane` answers with `module` (or what `module` loads, imported lazily like a
 * lane's own) instead of its folder's; null goes back to the folder's.
 */
export function registerLaneOverride(lane: LaneId, module: LaneModule | Loader | null): void {
  if (module === null) overrides.delete(lane);
  else overrides.set(lane, module);
  setLane(lane, undefined);
}

/** Whether this build has the lane: one of its databases is in the picker. */
function laneBuilt(lane: LaneId, info: Pick<AppInfo, "kinds"> | null | undefined): boolean {
  const kinds: readonly DatabaseKind[] = visibleKinds(info);
  return LANES[lane].kinds.some((k) => kinds.includes(k));
}

/**
 * Whether this build has the lane that serves `driver` (or speaks `dialect`): false for a core
 * one, which has no lane. A session of a lane the build lacks gets none of the lane's views.
 */
export function laneBuiltFor(
  of: { driver: Driver } | { dialect: SqlDialectId },
  info: Pick<AppInfo, "kinds"> | null | undefined = useAppInfoStore.getState().info,
): boolean {
  const lane = "driver" in of ? laneOfDriver(of.driver) : laneOfDialect(of.dialect);
  return lane !== null && laneBuilt(lane, info);
}

/** The lane of an id; null for a core one. */
type LaneFor<K> = (id: K) => LaneId | null;

/** What a lane answers for `id` in its current state, without starting its load. */
function answer<K, T>(
  laneOf: LaneFor<K>,
  pick: (m: LaneModule, id: K) => T | undefined,
  id: K,
  info: Pick<AppInfo, "kinds"> | null | undefined,
  state: LaneState | undefined,
): T | null | undefined {
  const lane = laneOf(id);
  if (lane === null || !laneBuilt(lane, info)) return null;
  if (state?.status === "loaded") return pick(state.module, id) ?? null;
  if (state?.status === "failed") return null;
  return undefined;
}

function lookup<K, T>(
  laneOf: LaneFor<K>,
  pick: (m: LaneModule, id: K) => T | undefined,
  id: K,
  info: Pick<AppInfo, "kinds"> | null | undefined,
): T | null | undefined {
  const lane = laneOf(id);
  const state = lane === null ? undefined : useLanes.getState().lanes[lane];
  const result = answer(laneOf, pick, id, info, state);
  if (result === undefined && lane !== null) importLane(lane).catch(() => {});
  return result;
}

async function awaitLookup<K, T>(
  laneOf: LaneFor<K>,
  pick: (m: LaneModule, id: K) => T | undefined,
  id: K,
): Promise<T | null> {
  const lane = laneOf(id);
  if (lane === null || !laneBuilt(lane, useAppInfoStore.getState().info)) return null;
  return pick(await importLane(lane), id) ?? null;
}

const pickKind = (m: LaneModule, k: DatabaseKind) => m.kinds?.[k];
const pickDriver = (m: LaneModule, d: Driver) => m.drivers?.[d];
const pickDialect = (m: LaneModule, id: SqlDialectId) => m.dialects?.[id];

const appInfo = () => useAppInfoStore.getState().info;

/** The form of a database: null at once for a core or unbuilt one, undefined while it loads. */
export const loadKindUi = (kind: DatabaseKind): KindUi | null | undefined =>
  lookup(laneOfKind, pickKind, kind, appInfo());

/** The views of a driver: null at once for a core or unbuilt one, undefined while it loads. */
export const loadDriverUi = (driver: Driver): DriverUi | null | undefined =>
  lookup(laneOfDriver, pickDriver, driver, appInfo());

/** The editor support of a dialect: null at once for a core one, undefined while it loads. */
export const loadDialectUi = (dialect: SqlDialectId): DialectUi | null | undefined =>
  lookup(laneOfDialect, pickDialect, dialect, appInfo());

/** `loadKindUi` once the lane has loaded (rejects when its import failed). */
export const awaitKindUi = (kind: DatabaseKind): Promise<KindUi | null> =>
  awaitLookup(laneOfKind, pickKind, kind);

/** `loadDriverUi` once the lane has loaded (rejects when its import failed). */
export const awaitDriverUi = (driver: Driver): Promise<DriverUi | null> =>
  awaitLookup(laneOfDriver, pickDriver, driver);

/** `loadDialectUi` once the lane has loaded (rejects when its import failed). */
export const awaitDialectUi = (dialect: SqlDialectId): Promise<DialectUi | null> =>
  awaitLookup(laneOfDialect, pickDialect, dialect);

/** A lookup as a hook: renders again when the lane has loaded (or failed). */
function useLookup<K, T>(
  laneOf: LaneFor<K>,
  pick: (m: LaneModule, id: K) => T | undefined,
  id: K | null,
): T | null | undefined {
  const info = useAppInfoStore((s) => s.info);
  const lane = id === null ? null : laneOf(id);
  // Subscribes to the lane's state, so the answer below changes when it does.
  const state = useLanes((s) => (lane === null ? undefined : s.lanes[lane]));
  const result = id === null ? null : answer(laneOf, pick, id, info, state);
  // The load starts in an effect: rendering only reads.
  const wanted = result === undefined ? lane : null;
  useEffect(() => {
    if (wanted !== null) importLane(wanted).catch(() => {});
  }, [wanted]);
  return result;
}

export const useKindUi = (kind: DatabaseKind | null) => useLookup(laneOfKind, pickKind, kind);
export const useDriverUi = (driver: Driver | null) => useLookup(laneOfDriver, pickDriver, driver);
export const useDialectUi = (dialect: SqlDialectId | null) =>
  useLookup(laneOfDialect, pickDialect, dialect);
