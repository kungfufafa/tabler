import { describe, expect, it } from "vitest";
import type { DiagramTable } from "@/lib/ipc";
import { relation, shopSchema, table } from "./fixtures.test-utils";
import {
  autoLayout,
  boundsOf,
  CARD,
  cardHeight,
  cardSize,
  intersects,
  LAYOUT,
  placeTables,
  rectsOf,
  type Positions,
} from "./layout";

function expectNoOverlap(tables: readonly DiagramTable[], positions: Positions, gap = 8) {
  const rects = [...rectsOf(tables, positions).entries()];
  expect(rects).toHaveLength(tables.length);
  for (let i = 0; i < rects.length; i++)
    for (let j = i + 1; j < rects.length; j++) {
      const [a, ra] = rects[i]!;
      const [b, rb] = rects[j]!;
      expect(intersects(ra, rb, gap), `${a} overlaps ${b}`).toBe(false);
    }
}

/** A deterministic shuffle (the layout must not depend on the input order). */
function shuffled<T>(items: readonly T[], seed: number): T[] {
  const out = [...items];
  let s = seed;
  for (let i = out.length - 1; i > 0; i--) {
    s = (s * 1103515245 + 12345) % 2147483648;
    const j = s % (i + 1);
    [out[i], out[j]] = [out[j]!, out[i]!];
  }
  return out;
}

describe("cardSize", () => {
  it("grows with the text and stays within bounds", () => {
    const small = cardSize(table("t", ["*id:int"]));
    expect(small.w).toBe(CARD.minWidth);
    expect(small.h).toBe(CARD.header + CARD.row + CARD.padBottom);
    const wide = cardSize(table("t", ["a_rather_long_column_name:character varying(255)"]));
    expect(wide.w).toBeGreaterThan(CARD.minWidth);
    expect(wide.w).toBeLessThanOrEqual(CARD.maxWidth);
    const huge = cardSize(table("t", [`${"x".repeat(200)}:text`]));
    expect(huge.w).toBe(CARD.maxWidth);
    // A table without columns keeps one (empty) row.
    expect(cardHeight(0)).toBe(cardHeight(1));
  });
});

describe("autoLayout", () => {
  it("is deterministic whatever the input order", () => {
    const { tables, relations } = shopSchema();
    const a = autoLayout(tables, relations, "public");
    for (const seed of [1, 7, 42]) {
      const b = autoLayout(shuffled(tables, seed), shuffled(relations, seed + 1), "public");
      expect(b).toEqual(a);
    }
  });

  it("never overlaps cards", () => {
    const { tables, relations } = shopSchema();
    expectNoOverlap(tables, autoLayout(tables, relations, "public"));
  });

  it("puts referenced tables left of the tables pointing at them", () => {
    const { tables, relations } = shopSchema();
    const p = autoLayout(tables, relations, "public");
    expect(p.users!.x).toBeLessThan(p.orders!.x);
    expect(p.orders!.x).toBeLessThan(p.order_items!.x);
    expect(p.categories!.x).toBeLessThan(p.products!.x);
    expect(p.products!.x).toBeLessThan(p.order_items!.x);
  });

  it("puts unrelated tables underneath the related ones", () => {
    const { tables, relations } = shopSchema();
    const p = autoLayout(tables, relations, "public");
    const related = boundsOf(
      tables.filter((t) => !["settings", "audit_log", "active_users"].includes(t.ref.name)),
      p,
    );
    for (const name of ["settings", "audit_log", "active_users"])
      expect(p[name]!.y).toBeGreaterThanOrEqual(related.y + related.h);
  });

  it("ignores relations to other schemas and to unknown tables", () => {
    const tables = [table("a", ["*id:int"]), table("b", ["*id:int", "a_id:int"])];
    const withExternal = autoLayout(
      tables,
      [relation("b", "a_id", "a"), relation("b", "a_id", "x", "id", "ext", "other")],
      "public",
    );
    expect(withExternal).toEqual(autoLayout(tables, [relation("b", "a_id", "a")], "public"));
  });

  it("wraps big schemas and keeps them apart", () => {
    // A star (one parent, 60 children) and 80 unrelated tables.
    const tables = [
      table("hub", ["*id:int"]),
      ...Array.from({ length: 60 }, (_, i) =>
        table(`child_${String(i).padStart(2, "0")}`, ["*id:int", "hub_id:int", "x:text", "y:text"]),
      ),
      ...Array.from({ length: 80 }, (_, i) => table(`lone_${i}`, ["*id:int", "v:text"])),
    ];
    const relations = tables
      .filter((t) => t.ref.name.startsWith("child_"))
      .map((t) => relation(t.ref.name, "hub_id", "hub"));
    const p = autoLayout(tables, relations, "public");
    expectNoOverlap(tables, p);
    const children = tables.filter((t) => t.ref.name.startsWith("child_"));
    // The children's layer wraps into several columns instead of one very tall one.
    expect(new Set(children.map((t) => p[t.ref.name]!.x)).size).toBeGreaterThan(1);
    for (const t of children) expect(p[t.ref.name]!.y).toBeLessThan(LAYOUT.maxColumnHeight * 2);
    // Unrelated tables wrap into rows.
    const lone = tables.filter((t) => t.ref.name.startsWith("lone_"));
    expect(new Set(lone.map((t) => p[t.ref.name]!.y)).size).toBeGreaterThan(1);
  });

  it("handles an empty schema", () => {
    expect(autoLayout([], [], "public")).toEqual({});
    expect(boundsOf([], {})).toEqual({ x: 0, y: 0, w: 0, h: 0 });
  });
});

describe("placeTables", () => {
  it("keeps remembered positions and places new tables underneath", () => {
    const { tables, relations } = shopSchema();
    const saved: Positions = { users: { x: 500, y: 40 }, orders: { x: 900, y: 40 } };
    const p = placeTables(tables, relations, "public", saved);
    expect(p.users).toEqual({ x: 500, y: 40 });
    expect(p.orders).toEqual({ x: 900, y: 40 });
    expectNoOverlap(tables, p);
    const known = boundsOf(
      tables.filter((t) => saved[t.ref.name]),
      p,
    );
    for (const t of tables)
      if (!saved[t.ref.name]) expect(p[t.ref.name]!.y).toBeGreaterThan(known.y + known.h);
  });

  it("drops positions of tables that are gone, and lays out from scratch without any", () => {
    const { tables, relations } = shopSchema();
    const p = placeTables(tables, relations, "public", { dropped: { x: 1, y: 2 } });
    expect(p.dropped).toBeUndefined();
    expect(p).toEqual(autoLayout(tables, relations, "public"));
    expect(placeTables(tables, relations, "public", null)).toEqual(p);
  });
});
