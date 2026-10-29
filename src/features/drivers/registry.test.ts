/// <reference types="node" />
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DEFAULT_FORM } from "@/features/connections/schema";
import type { AppInfo, DatabaseKind } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useAppInfoStore } from "@/stores/appInfo";
import { fakeLane } from "./__fixtures__/fakeLane";
import { CORE_KINDS } from "./catalog";
import { LANE_IDS, LANES, laneOfDialect, laneOfDriver, laneOfKind, type LaneId } from "./lanes";
import {
  awaitDialectUi,
  awaitDriverUi,
  awaitKindUi,
  importLane,
  loadDialectUi,
  loadDriverUi,
  loadKindUi,
  LANES_WITH_MODULE,
  registerLaneOverride,
  useDriverUi,
  visibleKinds,
} from "./registry";

const ROOT = path.resolve(import.meta.dirname, "../../..");

interface LaneEntry {
  kinds: string[];
  drivers: string[];
  dialects: string[];
  ts: string[];
}
const LANES_JSON = JSON.parse(readFileSync(path.join(ROOT, "dev/lanes.json"), "utf8")) as {
  lanes: Record<string, LaneEntry>;
};

/** A build with the core databases and `kinds`. */
const built = (...kinds: DatabaseKind[]): AppInfo => ({
  version: "0.1.0",
  platform: "test",
  debug: true,
  kinds: [...CORE_KINDS, ...kinds],
});

afterEach(() => {
  useAppInfoStore.setState({ info: null });
  for (const lane of LANE_IDS) registerLaneOverride(lane, null);
  vi.restoreAllMocks();
});

describe("the lane table", () => {
  it("is dev/lanes.json's", () => {
    const fromJson = Object.fromEntries(
      Object.entries(LANES_JSON.lanes).map(([lane, l]) => [
        lane,
        { kinds: l.kinds, drivers: l.drivers, dialects: l.dialects },
      ]),
    );
    expect(LANES).toEqual(fromJson);
    for (const [lane, l] of Object.entries(LANES_JSON.lanes))
      expect(l.ts).toEqual([
        `src/features/drivers/${lane}/**`,
        `src/lib/ipc/mock/drivers/${lane}/**`,
      ]);
  });

  it("finds the lane of each database, driver and dialect", () => {
    expect(laneOfKind("cockroachdb")).toBe("pg-flavors");
    expect(laneOfKind("d1")).toBe("sqlite-remote");
    expect(laneOfDriver("libsql")).toBe("sqlite-remote");
    expect(laneOfDialect("tsql")).toBe("sqlserver");
    expect(laneOfDialect("cql")).toBe("cassandra");
    expect(laneOfKind("mariadb")).toBeNull();
    expect(laneOfDriver("postgres")).toBeNull();
    expect(laneOfDialect("postgres")).toBeNull();
  });
});

// These tests never import a lane's own module: what a lane holds (and whether its work in
// progress parses) is the lane's to test. Overrides stand in for them.
describe("the lane modules", () => {
  it("are one per lane, found by the registry's lazy glob", () => {
    expect(LANES_WITH_MODULE).toEqual(LANE_IDS);
  });
});

describe("the registry", () => {
  it("answers null at once for the core drivers and the databases this build lacks", () => {
    expect(visibleKinds(null)).toEqual(CORE_KINDS);
    expect(loadKindUi("postgres")).toBeNull();
    expect(loadKindUi("mariadb")).toBeNull();
    expect(loadDriverUi("mysql")).toBeNull();
    expect(loadDialectUi("sqlite")).toBeNull();
    // Before the backend answers (and in tests) only the core databases are built.
    expect(loadKindUi("mongodb")).toBeNull();
    expect(loadDriverUi("mongodb")).toBeNull();
    expect(loadDialectUi("tsql")).toBeNull();
  });

  it("loads a built lane lazily: undefined while it loads, then its part", async () => {
    useAppInfoStore.setState({ info: built("mongodb") });
    registerLaneOverride("mongodb", fakeLane("mongodb", "mongodb"));
    expect(loadKindUi("mongodb")).toBeUndefined();
    const form = await awaitKindUi("mongodb");
    expect(form?.rows).toEqual(["hostPort", "user", "secret", "database"]);
    expect(loadKindUi("mongodb")).toBe(form);
    expect(loadDriverUi("mongodb")?.console?.classify("find orders")).toBe("read");
    // A lane that leaves a part out answers null for it.
    expect(await awaitDialectUi("tsql")).toBeNull();
  });

  it("answers from an override, and starts over when another is registered", async () => {
    useAppInfoStore.setState({ info: built("redis", "mongodb") });
    registerLaneOverride("redis", fakeLane("redis", "redis"));
    expect((await awaitDriverUi("redis"))?.TableView).toBeDefined();
    registerLaneOverride("redis", {});
    expect(await awaitDriverUi("redis")).toBeNull();
    // An override may also be a loader, imported lazily like a lane's own.
    registerLaneOverride("mongodb", () =>
      Promise.resolve({ default: fakeLane("mongodb", "mongodb") }),
    );
    expect(loadDriverUi("mongodb")).toBeUndefined();
    expect((await awaitDriverUi("mongodb"))?.objectMenu).toBeDefined();
  });

  it("renders the driver's views once its lane has loaded (useDriverUi)", async () => {
    useAppInfoStore.setState({ info: built("mongodb") });
    registerLaneOverride("mongodb", () =>
      Promise.resolve({ default: fakeLane("mongodb", "mongodb") }),
    );
    expect(renderHook(() => useDriverUi("postgres")).result.current).toBeNull();
    expect(renderHook(() => useDriverUi(null)).result.current).toBeNull();
    const { result } = renderHook(() => useDriverUi("mongodb"));
    expect(result.current).toBeUndefined();
    await waitFor(() => expect(result.current?.Inspector).toBeDefined());
    // A build without the lane answers null again.
    act(() => useAppInfoStore.setState({ info: built() }));
    expect(result.current).toBeNull();
  });
});

describe("a lane with a syntax error", () => {
  let dir: string;
  let broken: string;

  beforeAll(() => {
    // Written where no check reads (dist is ignored by git, tsc, eslint and vitest's globs) but
    // inside the project, which is all Vite imports from; removed again after the test.
    mkdirSync(path.join(ROOT, "dist"), { recursive: true });
    dir = mkdtempSync(path.join(ROOT, "dist", ".broken-lane-"));
    broken = path.join(dir, "index.ts");
    writeFileSync(broken, 'import type { LaneModule } from "x";\nconst lane: LaneModule = {;\n');
  });
  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  it("fails only its own import", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    useAppInfoStore.setState({ info: built("redis", "mongodb", "duckdb") });
    // Every lane loads lazily from a module of its own; redis's does not parse.
    for (const lane of LANE_IDS) registerLaneOverride(lane, () => Promise.resolve({ default: {} }));
    registerLaneOverride("mongodb", () =>
      Promise.resolve({ default: fakeLane("mongodb", "mongodb") }),
    );
    registerLaneOverride("redis", () => import(/* @vite-ignore */ pathToFileURL(broken).href));
    const settled = await Promise.allSettled(LANE_IDS.map((lane) => importLane(lane)));
    const failed = LANE_IDS.filter((_, i) => settled[i]!.status === "rejected");
    expect(failed).toEqual<LaneId[]>(["redis"]);
    const reason = (settled[LANE_IDS.indexOf("redis")] as PromiseRejectedResult).reason;
    expect(String(reason)).toMatch(/PARSE_ERROR|Unexpected token/);
    expect(error).toHaveBeenCalledWith("The redis driver lane failed to load", expect.anything());
    // Its lookups answer null (the UI goes on without it); awaiting one rejects.
    expect(loadDriverUi("redis")).toBeNull();
    await expect(awaitKindUi("redis")).rejects.toBeDefined();
    // The other lanes answer as before.
    expect((await awaitDriverUi("mongodb"))?.console).toBeDefined();
    expect(await awaitDriverUi("duckdb")).toBeNull();
    expect(loadDriverUi("duckdb")).toBeNull();
  });
});

describe("the fake lane", () => {
  const lane = fakeLane("mongodb", "mongodb");
  const form = lane.kinds?.mongodb;
  const views = lane.drivers?.mongodb;

  it("saves its database row as a driver param and drops the user", () => {
    const base = SEED_PROFILES[0]!;
    const saved = form?.toProfile?.({ ...DEFAULT_FORM, database: "admin2" }, base);
    expect(saved?.options.driverParams).toEqual({ authSource: "admin2" });
    expect(saved?.user).toBeNull();
    expect(saved && form?.fromProfile?.(saved)).toEqual({ database: "admin2" });
    expect(form?.url?.parse("fake://db.local:27018/admin")?.values).toEqual({
      host: "db.local",
      port: "27018",
      database: "admin",
    });
    expect(form?.validate?.({ ...DEFAULT_FORM, host: "" })).toEqual([
      { path: "host", message: "Host is required" },
    ]);
  });

  it("splits and classifies console lines, and calls the session from its menu", async () => {
    const console = views?.console;
    expect(console?.split("find orders\n\n  count users")).toEqual([
      { start: 0, end: 11, line: 1 },
      { start: 15, end: 26, line: 3 },
    ]);
    expect(console?.classify("insert orders {}")).toBe("write");
    const call = vi.fn().mockResolvedValue({ ok: 1 });
    const refresh = vi.fn();
    const items = views?.objectMenu?.(
      { table: { schema: null, name: "orders" }, kind: "table", database: "app" },
      { connectionId: "c1", info: {} as never, caps: {} as never, call, refresh },
    );
    expect(items?.map((i) => i.label)).toEqual(["Collection Stats", "Compact…"]);
    await items?.[1]?.run();
    expect(call).toHaveBeenCalledWith("compact", { collection: "orders" });
    expect(refresh).toHaveBeenCalled();
  });
});
