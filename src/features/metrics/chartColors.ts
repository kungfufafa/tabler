import type { Paint, Scene } from "@/features/charts/scene";

/** `paint` with its series slot replaced by the object's colour choice. */
export function remapPaint(paint: Paint, colors: readonly number[]): Paint {
  const m = /^series-(\d)$/.exec(paint);
  if (!m) return paint;
  const slot = colors[Number(m[1]) - 1];
  return slot === undefined ? paint : (`series-${(slot % 8) + 1}` as Paint);
}

/** The scene drawn with the object's colours (also what Copy as SVG / Export PNG take). */
export function recolorScene(scene: Scene, colors: readonly number[]): Scene {
  if (colors.length === 0) return scene;
  const re = (p: Paint) => remapPaint(p, colors);
  return {
    ...scene,
    nodes: scene.nodes.map((n) => {
      switch (n.t) {
        case "rect":
          return { ...n, fill: re(n.fill) };
        case "path":
          return {
            ...n,
            fill: n.fill === "none" ? "none" : re(n.fill),
            ...(n.stroke ? { stroke: re(n.stroke) } : {}),
          };
        case "circle":
          return { ...n, fill: re(n.fill), ...(n.stroke ? { stroke: re(n.stroke) } : {}) };
        default:
          return n;
      }
    }),
    slices: scene.slices.map((s) => ({ ...s, paint: re(s.paint) })),
    tooltip: (i) => {
      const t = scene.tooltip(i);
      return { ...t, rows: t.rows.map((r) => ({ ...r, paint: re(r.paint) })) };
    },
  };
}
