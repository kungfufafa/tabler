import { afterEach, describe, expect, it } from "vitest";
import { shopSchema } from "./fixtures.test-utils";
import {
  describeRelation,
  foreignKeyColumns,
  isLocal,
  isOptional,
  relationKey,
  relationsAmong,
  relationsOf,
  visibleTables,
  withNeighbours,
} from "./graph";
import {
  centerOn,
  clampZoom,
  fitView,
  forgetPositions,
  loadLineStyle,
  loadPositions,
  MAX_ZOOM,
  MIN_ZOOM,
  positionsKey,
  saveLineStyle,
  savePositions,
  toDiagram,
  zoomAt,
} from "./viewport";

const names = (tables: Array<{ ref: { name: string } }>) => tables.map((t) => t.ref.name).sort();

describe("graph", () => {
  const d = shopSchema();

  it("finds a table's neighbours in both directions, within the schema", () => {
    expect([...withNeighbours(d, new Set(["orders"]))].sort()).toEqual([
      "coupons",
      "order_items",
      "orders",
      "users",
    ]);
    // The cross-schema key (billing.accounts) is not a neighbour.
    expect(withNeighbours(d, new Set(["orders"])).has("accounts")).toBe(false);
  });

  it("shows every table, or the selection and its neighbours", () => {
    expect(visibleTables(d, new Set(["products"]), false)).toHaveLength(d.tables.length);
    expect(visibleTables(d, new Set(), true)).toHaveLength(d.tables.length);
    expect(names(visibleTables(d, new Set(["products"]), true))).toEqual([
      "categories",
      "order_items",
      "products",
    ]);
  });

  it("keeps the relations between drawn tables", () => {
    const drawn = relationsAmong(d, new Set(["orders", "users", "order_items"]));
    expect(drawn.map((r) => r.name).sort()).toEqual([
      "order_items_order_id_fkey",
      "orders_user_id_fkey",
      "users_manager_id_fkey",
    ]);
    expect(
      isLocal(
        d.relations.find((r) => r.name === "orders_account_fkey")!,
        "public",
      ),
    ).toBe(false);
  });

  it("collects FK columns, optional keys and relation keys", () => {
    const fk = foreignKeyColumns(d.relations);
    expect([...fk.get("orders")!].sort()).toEqual(["coupon_id", "user_id"]);
    const orders = d.tables.find((t) => t.ref.name === "orders");
    const coupon = d.relations.find((r) => r.name === "orders_coupon_id_fkey")!;
    const user = d.relations.find((r) => r.name === "orders_user_id_fkey")!;
    expect(isOptional(coupon, orders)).toBe(true);
    expect(isOptional(user, orders)).toBe(false);
    expect(isOptional(user, undefined)).toBe(false);
    expect(relationsOf(d.relations, new Set(["coupons"]))).toEqual(new Set([relationKey(coupon)]));
    expect(describeRelation(user)).toBe("orders(user_id) → users(id)");
  });
});

describe("viewport", () => {
  afterEach(() => localStorage.clear());

  it("zooms around the anchor and clamps", () => {
    const v = zoomAt({ x: 10, y: 20, k: 1 }, 2, { x: 110, y: 120 });
    expect(v.k).toBe(2);
    // The diagram point under the anchor stays put.
    expect(toDiagram(v, { x: 110, y: 120 })).toEqual(
      toDiagram({ x: 10, y: 20, k: 1 }, { x: 110, y: 120 }),
    );
    expect(zoomAt(v, 100, { x: 0, y: 0 }).k).toBe(MAX_ZOOM);
    expect(clampZoom(0)).toBe(MIN_ZOOM);
  });

  it("fits a box into the viewport, never above 100 %", () => {
    const v = fitView({ x: 100, y: 100, w: 2000, h: 1000 }, { w: 1064, h: 564 }, 32);
    expect(v.k).toBe(0.5);
    // Centred: the box's middle lands in the viewport's middle.
    expect(v.x + (100 + 1000) * v.k).toBeCloseTo(532);
    expect(v.y + (100 + 500) * v.k).toBeCloseTo(282);
    expect(fitView({ x: 0, y: 0, w: 100, h: 100 }, { w: 1000, h: 1000 }).k).toBe(1);
    expect(fitView({ x: 50, y: 60, w: 0, h: 0 }, { w: 0, h: 0 }, 10)).toEqual({
      x: -40,
      y: -50,
      k: 1,
    });
    const c = centerOn({ x: 0, y: 0, k: 2 }, { x: 10, y: 10, w: 20, h: 20 }, { w: 200, h: 100 });
    expect(c).toEqual({ x: 60, y: 10, k: 2 });
  });

  it("remembers positions per profile, database and schema", () => {
    const a = positionsKey("p1", "shop", "public");
    expect(a).not.toBe(positionsKey("p1", "shop", "audit"));
    expect(a).not.toBe(positionsKey("p2", "shop", "public"));
    expect(positionsKey("p1", null, "main")).toBe(positionsKey("p1", null, "main"));
    expect(loadPositions(a)).toBeNull();
    savePositions(a, { users: { x: 1, y: 2 } });
    expect(loadPositions(a)).toEqual({ users: { x: 1, y: 2 } });
    forgetPositions(a);
    expect(loadPositions(a)).toBeNull();
    // Damaged entries are skipped, not fatal.
    localStorage.setItem(a, JSON.stringify({ ok: { x: 3, y: 4 }, bad: { x: "1" }, nul: null }));
    expect(loadPositions(a)).toEqual({ ok: { x: 3, y: 4 } });
    localStorage.setItem(a, "{not json");
    expect(loadPositions(a)).toBeNull();
  });

  it("remembers the line style", () => {
    expect(loadLineStyle()).toBe("curved");
    saveLineStyle("orthogonal");
    expect(loadLineStyle()).toBe("orthogonal");
  });
});
