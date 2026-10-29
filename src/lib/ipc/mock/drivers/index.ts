import { LANE_IDS, laneOfDriver, type LaneId } from "@/features/drivers/lanes";
import type { ConnectionProfile } from "../../bindings/ConnectionProfile";
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
  // Imported here: the handlers will dispatch to this module, which must not import them back.
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
