import { DESCRIPTORS, isCoreDriver, isCoreKind, KINDS } from "@/features/drivers/catalog";
import { LANE_IDS, LANES, laneOfDriver, type LaneId } from "@/features/drivers/lanes";
import type { ConnectionProfile } from "../../bindings/ConnectionProfile";
import type { DatabaseKind } from "../../bindings/DatabaseKind";
import type { Driver } from "../../bindings/Driver";
import type { MockDriver, MockLane } from "./types";

export type { MockDriver, MockLane, MockSession } from "./types";

/**
 * The mock's drivers beyond the core three: each lane's `mock/drivers/<lane>/index.ts`, imported
 * lazily (no `eager`), so a lane's error breaks only that lane's mock. `fake/` is the
 * foundation's test driver: tests register it themselves (`registerMockDriver`).
 */
const LOADERS: Partial<Record<LaneId, () => Promise<{ default: MockLane }>>> = Object.fromEntries(
  Object.entries(
    import.meta.glob<{ default: MockLane }>(["./*/index.ts", "!./fake/index.ts"]),
  ).flatMap(([file, load]) => {
    const lane = LANE_IDS.find((id) => file === `./${id}/index.ts`);
    return lane ? [[lane, load]] : [];
  }),
);

/** The lanes that have a mock module. */
export const MOCK_LANES: readonly LaneId[] = LANE_IDS.filter((lane) => LOADERS[lane]);

const lanes = new Map<LaneId, Promise<MockLane>>();
const overrides = new Map<Driver, MockDriver>();

/** A lane's mock module, imported once. */
export function importMockLane(lane: LaneId): Promise<MockLane> {
  let loading = lanes.get(lane);
  if (!loading) {
    const load = LOADERS[lane];
    loading = load ? load().then((m) => m.default) : Promise.resolve({ drivers: [] });
    lanes.set(lane, loading);
  }
  return loading;
}

/** Tests only: `driver` is served by `mock` (null: by its lane's mock again). */
export function registerMockDriver(driver: Driver, mock: MockDriver | null): void {
  if (mock) overrides.set(driver, mock);
  else overrides.delete(driver);
}

/** The mock of `driver`; null for the core drivers and a lane whose mock has none. */
export async function loadMockDriver(driver: Driver): Promise<MockDriver | null> {
  const override = overrides.get(driver);
  if (override) return override;
  const lane = laneOfDriver(driver);
  if (lane === null) return null;
  const { drivers } = await importMockLane(lane);
  return drivers.find((d) => d.driver === driver) ?? null;
}

/**
 * Adds the seed profiles of the mock drivers `ids` to the mock's profiles (`?mockDrivers=`, and
 * tests); the core seed (`SEED_PROFILES`) stays as it is. Returns the profiles added.
 */
export async function seedMockDriverProfiles(ids: readonly Driver[]): Promise<ConnectionProfile[]> {
  // Imported here: the handlers dispatch to this module, which must not import them back.
  const { mockProfiles } = await import("../handlers");
  const added: ConnectionProfile[] = [];
  for (const id of ids) {
    const mock = await loadMockDriver(id);
    for (const p of mock?.seedProfiles() ?? []) {
      mockProfiles.set(p.id, p);
      added.push(p);
    }
  }
  return added;
}

// --- the mock's build --------------------------------------------------------------------------

/**
 * The databases beyond the core ones the mock is a build of: none by default (the mock is a build
 * of the core drivers, so the picker shows the 4 core tiles), the ones `?mockDrivers=` names in
 * `vite --mode mock`, or a test's.
 */
const builtKinds = new Set<DatabaseKind>();

/** The databases the mock connects to, in the picker's order (`AppInfo.kinds`). */
export function mockBuiltKinds(): DatabaseKind[] {
  return KINDS.filter((k) => isCoreKind(k.kind) || builtKinds.has(k.kind)).map((k) => k.kind);
}

/** The drivers the mock has (`AppInfo.drivers`), in the catalog's order. */
export function mockBuiltDrivers(): Driver[] {
  const kinds = mockBuiltKinds();
  return Object.values(DESCRIPTORS)
    .map((d) => d.driver)
    .filter((d) => KINDS.some((k) => k.driver === d && kinds.includes(k.kind)));
}

/** Whether the mock connects to `kind` (`registry::ensure_built`). */
export function isMockBuilt(kind: DatabaseKind): boolean {
  return isCoreKind(kind) || builtKinds.has(kind);
}

/** The databases `id` names: a database, a lane driver's, a lane's (`pg-flavors`), or `all`. */
function kindsNamed(id: string): DatabaseKind[] {
  const lane = LANE_IDS.find((l) => l === id);
  return KINDS.filter(
    (k) =>
      id === "all" ||
      k.kind === id ||
      (k.driver === id && !isCoreDriver(k.driver)) ||
      (lane !== undefined && LANES[lane].kinds.some((kind) => kind === k.kind)),
  ).map((k) => k.kind);
}

/**
 * Makes the mock a build of the databases `ids` names: a database id (`cockroachdb`), a lane
 * driver's id (`redis`), a lane's (`pg-flavors`), or `all`. Unknown ids are left out. Returns the
 * databases added.
 */
export function buildMockDrivers(ids: readonly string[]): DatabaseKind[] {
  const added: DatabaseKind[] = [];
  for (const raw of ids) {
    const id = raw.trim().toLowerCase();
    const kinds = kindsNamed(id);
    if (kinds.length === 0 && id !== "") console.warn(`[mock] no driver or database “${raw}”`);
    for (const kind of kinds)
      if (!isCoreKind(kind) && !builtKinds.has(kind)) {
        builtKinds.add(kind);
        added.push(kind);
      }
  }
  return added;
}

/** Tests only: back to a build of the core drivers. */
export function resetMockBuild(): void {
  builtKinds.clear();
}

let seeding: Promise<unknown> = Promise.resolve();

/** Resolves once the seed profiles of `enableMockDrivers` are in (`list_profiles` waits). */
export const mockDriversSeeded = (): Promise<unknown> => seeding;

/**
 * `?mockDrivers=redis,mongodb`: builds them into the mock (`buildMockDrivers`) and adds their mock
 * drivers' seed profiles to the launcher. Returns the profiles added.
 */
export function enableMockDrivers(ids: readonly string[]): Promise<ConnectionProfile[]> {
  const kinds = buildMockDrivers(ids);
  const drivers = KINDS.filter((k) => kinds.includes(k.kind) && !isCoreDriver(k.driver)).map(
    (k) => k.driver,
  );
  const seeded = seedMockDriverProfiles([...new Set(drivers)]);
  seeding = seeded.catch((err: unknown) => console.error("[mock] seeding the drivers failed", err));
  return seeded;
}

if (typeof window !== "undefined") {
  const ids = new URLSearchParams(window.location.search).get("mockDrivers");
  if (ids) void enableMockDrivers(ids.split(","));
}
