import { afterEach, describe, expect, it } from "vitest";
import { LANE_IDS } from "@/features/drivers/lanes";
import type { ConnectionProfile } from "../../bindings/ConnectionProfile";
import type { QueryEvent } from "../../bindings/QueryEvent";
import { SEED_PROFILES } from "../fixtures";
import { mockProfiles } from "../handlers";
import { mockInvoke } from "../transport";
import { fakeMockDriver, MISSING } from "./fake";
import { loadMockDriver, MOCK_LANES, registerMockDriver, seedMockDriverProfiles } from ".";

afterEach(() => {
  registerMockDriver("mongodb", null);
  mockProfiles.delete("p-fake-mongodb");
});

// These tests never import a lane's own mock (what it holds is the lane's to test): the fake
// stands in for one.
describe("the mock's driver lanes", () => {
  it("have a module each, found by the lazy glob", () => {
    expect(MOCK_LANES).toEqual(LANE_IDS);
  });

  it("serve no core driver, and a test's driver in place of its lane's", async () => {
    expect(await loadMockDriver("postgres")).toBeNull();
    const fake = fakeMockDriver("mongodb");
    registerMockDriver("mongodb", fake);
    expect(await loadMockDriver("mongodb")).toBe(fake);
  });

  it("seed their profiles next to the core seed", async () => {
    registerMockDriver("mongodb", fakeMockDriver("mongodb"));
    const added = await seedMockDriverProfiles(["mongodb", "postgres"]);
    expect(added.map((p) => p.id)).toEqual(["p-fake-mongodb"]);
    const listed = await mockInvoke<ConnectionProfile[]>("list_profiles", {});
    expect(listed.map((p) => p.id)).toContain("p-fake-mongodb");
    expect(SEED_PROFILES).toHaveLength(6);
  });
});

describe("the fake document store", () => {
  const session = () => fakeMockDriver("mongodb").connect(SEED_PROFILES[0]!);
  const orders = { schema: null, name: "orders" };

  it("lists collections and pages them by cursor, a missing field kept apart from NULL", () => {
    const s = session();
    expect(s.caps.namespaces.labels.tables).toBe("Collections");
    expect(s.listTables("").map((t) => t.name)).toEqual(["orders", "users"]);
    const req = { table: orders, columns: null, filter: null, sort: [], limit: 10, offset: 0 };
    const first = s.browse(req);
    expect(first.rows).toHaveLength(10);
    expect(first.nextCursor).toBe("10");
    const last = s.browse({ ...req, cursor: "20" });
    expect(last.rows).toHaveLength(5);
    expect(last.nextCursor).toBeNull();
    const status = first.columns.findIndex((c) => c.name === "status");
    expect(first.rows[4]![status]).toEqual(MISSING);
    expect(s.count?.(orders, null)).toEqual({ count: 25, exact: true });
  });

  it("applies one change at a time and reports each", () => {
    const s = session();
    const key = (id: string) => ({
      kind: "pk" as const,
      value: { _id: { t: "text" as const, v: id } },
    });
    const result = s.apply!({
      table: orders,
      returning: false,
      changes: [
        { op: "update", rowId: "1", key: key("o1"), changes: { status: { t: "text", v: "paid" } } },
        {
          op: "update",
          rowId: "2",
          key: key("o2"),
          changes: { status: { t: "text", v: "__fail__" } },
        },
        { op: "delete", rowId: "3", key: key("o3") },
      ],
    });
    expect(result.success).toBe(false);
    expect(result.results.map((r) => r.error === null)).toEqual([true, false, true]);
    expect(result.sql[0]).toBe('db.orders.updateOne({_id: "o1"}, {$set: {status: "paid"}})');
    expect(s.fetchCell?.(orders, key("o1"), "status")).toEqual({ t: "text", v: "paid" });
    expect(s.count?.(orders, null).count).toBe(24);
  });

  it("runs console lines and its own calls", async () => {
    const s = session();
    const events: QueryEvent[] = [];
    await s.run("count orders", (e) => events.push(e));
    expect(events.map((e) => e.event)).toEqual(["started", "columns", "rows", "finished"]);
    events.length = 0;
    await s.run('insert users {"_id": "u3", "name": "Grace"}', (e) => events.push(e));
    const finished = events.at(-1);
    expect(finished?.event === "finished" && finished.data.result.affected).toBe(1);
    events.length = 0;
    await s.run("drop users", (e) => events.push(e));
    expect(events.at(-1)?.event).toBe("failed");
    expect(s.call?.("stats", { collection: "users" })).toEqual({ collection: "users", count: 3 });
  });
});
