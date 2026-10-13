import { describe, expect, it } from "vitest";
import { mockSchemaRelations } from "@/lib/ipc/mock/diagram";
import { ipc } from "@/lib/ipc";
import { buildSvg, exportBounds, pathBounds, resolvePalette } from "./export";
import { shopSchema } from "./fixtures.test-utils";
import { autoLayout, boundsOf } from "./layout";
import { LIVE_PALETTE, type ScenePalette } from "./scene";

const PALETTE: ScenePalette = {
  canvas: "#ffffff",
  card: "#ffffff",
  header: "#f5f5f4",
  border: "#d4d4d4",
  divider: "#e5e5e5",
  text: "#1c1c22",
  muted: "#71717a",
  edge: "#71717a",
  accent: "#2563eb",
  accentSoft: "rgba(37, 99, 235, 0.12)",
  pk: "#d97706",
  fk: "#2563eb",
  viewIcon: "#71717a",
  labelBg: "#ffffff",
};

describe("SVG export", () => {
  const d = shopSchema();
  const positions = autoLayout(d.tables, d.relations, "public");
  const input = {
    tables: d.tables,
    relations: d.relations,
    positions,
    lineStyle: "curved" as const,
  };

  it("is a standalone document with plain colours", async () => {
    const { svg, width, height } = await buildSvg(input, PALETTE);
    expect(svg.startsWith('<?xml version="1.0" encoding="UTF-8"?>\n<svg')).toBe(true);
    expect(svg).toContain('xmlns="http://www.w3.org/2000/svg"');
    expect(svg).toContain(`width="${width}"`);
    expect(svg).toContain(`height="${height}"`);
    expect(svg).not.toContain("var(--");
    for (const t of d.tables) expect(svg).toContain(`>${t.ref.name}<`);
    // Interactive bits (hit areas, hover labels, tooltips) are left out.
    expect(svg).not.toContain('stroke="transparent"');
    expect(svg).not.toContain("<title>");
    const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
    expect(doc.querySelector("parsererror")).toBeNull();
    expect(doc.querySelectorAll("[data-table]")).toHaveLength(d.tables.length);
    // The cross-schema key has no card to attach to.
    expect(doc.querySelectorAll("[data-relation]")).toHaveLength(d.relations.length - 1);
  });

  it("covers every card and line with a margin", () => {
    const b = exportBounds(input);
    const cards = boundsOf(d.tables, positions);
    expect(b.x).toBeLessThan(cards.x);
    expect(b.y).toBeLessThan(cards.y);
    expect(b.x + b.w).toBeGreaterThan(cards.x + cards.w);
    expect(b.y + b.h).toBeGreaterThan(cards.y + cards.h);
    expect(pathBounds("M 1 2 C 10 -5 3 4 7 8")).toEqual({ x: 1, y: -5, w: 9, h: 13 });
    expect(pathBounds("")).toBeNull();
  });

  it("resolves every colour of the palette", () => {
    expect(Object.keys(resolvePalette()).sort()).toEqual(Object.keys(LIVE_PALETTE).sort());
    // jsdom computes literal colours only (no var()); the browser resolves the theme's.
    const palette = resolvePalette(document.body, { ...LIVE_PALETTE, card: "rgb(1, 2, 3)" });
    expect(palette.card).toBe("rgb(1, 2, 3)");
  });
});

describe("diagram mock", () => {
  it("builds the schema from the table list and metadata", async () => {
    const info = await ipc.connections.connect("p-local-pg");
    const d = await mockSchemaRelations(info.connectionId, "public");
    expect(d.schema).toBe("public");
    const names = d.tables.map((t) => t.ref.name);
    expect(names).toContain("users");
    expect(names).toEqual([...names].sort());
    const users = d.tables.find((t) => t.ref.name === "users")!;
    expect(users.columns[0]).toMatchObject({ name: "id", isPk: true, nullable: false });
    expect(d.relations.map((r) => `${r.from.name}->${r.to.name}`).sort()).toEqual([
      "post_tags->posts",
      "post_tags->tags",
      "posts->users",
    ]);
    expect(await ipc.diagram.schemaRelations(info.connectionId, "public")).toEqual(d);
    await expect(
      ipc.diagram.writeBinaryFile("/tmp/d.png", "iVBORw0KGgo="),
    ).resolves.toBeUndefined();
    await expect(ipc.diagram.writeBinaryFile("/missing/d.png", "AA==")).rejects.toMatchObject({
      code: "IO",
    });
    await ipc.connections.disconnect(info.connectionId);
  });
});
