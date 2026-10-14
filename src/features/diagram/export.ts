import { createElement } from "react";
import { ipc, isMock, type DiagramRelation, type DiagramTable } from "@/lib/ipc";
import { DiagramScene } from "./DiagramScene";
import type { LineStyle } from "./geometry";
import { boundsOf, type Positions, type Rect } from "./layout";
import { FONT_SANS, LIVE_PALETTE, sceneGeometry, type ScenePalette } from "./scene";

/** Margin around the exported cards. */
const EXPORT_PADDING = 32;
/** A PNG never gets more pixels than this (WebKit's canvas limit is around 16k × 16k). */
const MAX_PNG_PIXELS = 120_000_000;

export interface ExportInput {
  tables: readonly DiagramTable[];
  relations: readonly DiagramRelation[];
  positions: Positions;
  lineStyle: LineStyle;
}

/** Bounding box of an SVG path's points (control points included: a superset of the curve). */
export function pathBounds(d: string): Rect | null {
  const nums = d.match(/-?\d+(?:\.\d+)?(?:e-?\d+)?/gi)?.map(Number) ?? [];
  if (nums.length < 2) return null;
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (let i = 0; i + 1 < nums.length; i += 2) {
    const x = nums[i]!;
    const y = nums[i + 1]!;
    x0 = Math.min(x0, x);
    x1 = Math.max(x1, x);
    y0 = Math.min(y0, y);
    y1 = Math.max(y1, y);
  }
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/** The area an export covers: every card and line, plus a margin. */
export function exportBounds(input: ExportInput): Rect {
  const cards = boundsOf(input.tables, input.positions);
  let x0 = cards.x;
  let y0 = cards.y;
  let x1 = cards.x + cards.w;
  let y1 = cards.y + cards.h;
  const { lines } = sceneGeometry(input.tables, input.relations, input.positions, input.lineStyle);
  for (const line of lines) {
    const b = pathBounds(line.geometry.path);
    if (!b) continue;
    x0 = Math.min(x0, b.x);
    y0 = Math.min(y0, b.y);
    x1 = Math.max(x1, b.x + b.w);
    y1 = Math.max(y1, b.y + b.h);
  }
  return {
    x: Math.floor(x0 - EXPORT_PADDING),
    y: Math.floor(y0 - EXPORT_PADDING),
    w: Math.ceil(x1 - x0 + EXPORT_PADDING * 2),
    h: Math.ceil(y1 - y0 + EXPORT_PADDING * 2),
  };
}

/**
 * The diagram as a standalone SVG document: every colour is a plain value (no CSS variables or
 * stylesheet), so it opens the same anywhere.
 */
export async function buildSvg(
  input: ExportInput,
  palette: ScenePalette,
): Promise<{ svg: string; width: number; height: number }> {
  const { renderToStaticMarkup } = await import("react-dom/server");
  const b = exportBounds(input);
  const markup = renderToStaticMarkup(
    createElement(
      "svg",
      {
        xmlns: "http://www.w3.org/2000/svg",
        width: b.w,
        height: b.h,
        viewBox: `${b.x} ${b.y} ${b.w} ${b.h}`,
        fontFamily: FONT_SANS,
      },
      createElement("rect", { x: b.x, y: b.y, width: b.w, height: b.h, fill: palette.canvas }),
      createElement(DiagramScene, {
        tables: input.tables,
        relations: input.relations,
        positions: input.positions,
        lineStyle: input.lineStyle,
        palette,
      }),
    ),
  );
  return {
    svg: `<?xml version="1.0" encoding="UTF-8"?>\n${markup}\n`,
    width: b.w,
    height: b.h,
  };
}

const SENTINEL = "#010203";

const isJsdom = () => typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent);

/**
 * The live palette with its CSS variables resolved to sRGB colours (`#rrggbb` or `rgba()`), read
 * from the current theme: a probe element resolves the variable, a 1×1 canvas converts whatever
 * colour space it uses (oklch, color-mix…) to sRGB.
 */
export function resolvePalette(
  root: HTMLElement = document.body,
  source: ScenePalette = LIVE_PALETTE,
): ScenePalette {
  const probe = document.createElement("span");
  probe.style.display = "none";
  root.appendChild(probe);
  const canvas = isJsdom() ? null : document.createElement("canvas");
  if (canvas) canvas.width = canvas.height = 1;
  const ctx = canvas?.getContext("2d", { willReadFrequently: true }) ?? null;
  const resolve = (value: string): string => {
    probe.style.color = "";
    probe.style.color = value;
    const computed = getComputedStyle(probe).color || value;
    if (!ctx) return computed;
    ctx.clearRect(0, 0, 1, 1);
    // A colour the canvas cannot parse leaves the sentinel: keep the CSS text then.
    ctx.fillStyle = SENTINEL;
    ctx.fillStyle = computed;
    if (ctx.fillStyle === SENTINEL) return computed;
    ctx.fillRect(0, 0, 1, 1);
    const [r = 0, g = 0, b = 0, a = 255] = ctx.getImageData(0, 0, 1, 1).data;
    if (a === 255) return `#${[r, g, b].map((n) => n.toString(16).padStart(2, "0")).join("")}`;
    return `rgba(${r}, ${g}, ${b}, ${Math.round((a / 255) * 1000) / 1000})`;
  };
  try {
    const out = {} as ScenePalette;
    for (const key of Object.keys(source) as Array<keyof ScenePalette>)
      out[key] = resolve(source[key]);
    return out;
  } finally {
    probe.remove();
  }
}

/** Rasterises an SVG document at `scale` (2× by default, less when it would get too large). */
export async function svgToPngBase64(
  svg: string,
  width: number,
  height: number,
  scale = 2,
): Promise<string> {
  const image = new Image();
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error("The diagram could not be rendered as an image"));
    image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  });
  const s = Math.max(0.1, Math.min(scale, Math.sqrt(MAX_PNG_PIXELS / Math.max(1, width * height))));
  const canvas = document.createElement("canvas");
  canvas.width = Math.ceil(width * s);
  canvas.height = Math.ceil(height * s);
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("The diagram could not be rendered as an image");
  ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
  const url = canvas.toDataURL("image/png");
  return url.slice(url.indexOf(",") + 1);
}

/** Folder of the previous export, to start the next save panel in. */
let lastDir: string | null = null;

/** Asks where to save: the native save panel, or `window.prompt` in the browser mock. */
export async function pickDiagramPath(
  fileName: string,
  ext: "svg" | "png",
): Promise<string | null> {
  let picked: string | null;
  if (isMock) {
    picked = window.prompt("Export diagram as", `${lastDir ?? "/Users/me/Downloads"}/${fileName}`);
  } else {
    const { save } = await import("@tauri-apps/plugin-dialog");
    picked = await save({
      title: "Export Diagram",
      defaultPath: lastDir ? `${lastDir}/${fileName}` : fileName,
      filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
      canCreateDirectories: true,
    });
  }
  const path = picked?.trim();
  if (!path) return null;
  const slash = path.lastIndexOf("/");
  if (slash > 0) lastDir = path.slice(0, slash);
  return path;
}

/** Export ▸ SVG… / PNG…: asks for a file and writes the diagram. Resolves the path, or null. */
export async function exportDiagram(
  input: ExportInput,
  format: "svg" | "png",
  baseName: string,
): Promise<string | null> {
  const path = await pickDiagramPath(`${baseName}.${format}`, format);
  if (!path) return null;
  const { svg, width, height } = await buildSvg(input, resolvePalette());
  if (format === "svg") await ipc.files.writeText(path, svg);
  else await ipc.diagram.writeBinaryFile(path, await svgToPngBase64(svg, width, height));
  return path;
}

/** Copy as SVG. */
export async function copyDiagramSvg(input: ExportInput): Promise<void> {
  const { svg } = await buildSvg(input, resolvePalette());
  await navigator.clipboard.writeText(svg);
}
