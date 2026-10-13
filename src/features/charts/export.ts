import { toast } from "sonner";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { ipc, isMock, toIpcError } from "@/lib/ipc";
import type { Scene } from "./scene";
import { sceneToSvg, type ChartTheme } from "./svg";

/** The theme the app shows (exports use its colors). */
export function currentTheme(): ChartTheme {
  return typeof document !== "undefined" && document.documentElement.classList.contains("dark")
    ? "dark"
    : "light";
}

/**
 * A file name from the chart's title ("revenue by month.png"): no path or control characters, at
 * most 80 characters (code points, so an emoji is never cut in half).
 */
export function chartFileName(scene: Scene, ext: "png" | "svg"): string {
  const cleaned = scene.title.replace(/[\\/:*?"<>|\p{Cc}\p{Cs}]+/gu, " ").trim();
  const base = Array.from(cleaned).slice(0, 80).join("").trim() || "chart";
  return `${base}.${ext}`;
}

/** Copy as SVG: the chart as a standalone SVG document, in the current theme's colors. */
export async function copyChartSvg(scene: Scene): Promise<void> {
  try {
    await writeClipboard(sceneToSvg(scene, currentTheme()));
    toast.success("Copied the chart as SVG");
  } catch (e) {
    toast.error("Could not copy the chart", { description: toIpcError(e).message });
  }
}

/** Renders the SVG onto a canvas at `scale`× and returns the PNG, base64-encoded. */
export async function svgToPngBase64(
  svg: string,
  width: number,
  height: number,
  scale = 2,
): Promise<string> {
  const img = new Image();
  img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  await img.decode();
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(height * scale);
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("The chart could not be rendered as an image");
  ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
  const url = canvas.toDataURL("image/png");
  return url.slice(url.indexOf(",") + 1);
}

/** Folder of the previous export, to start the next save panel in. */
let lastDir: string | null = null;

/** The native save panel for a PNG; null when cancelled. */
async function pickPngPath(fileName: string): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({
    title: "Export Chart",
    defaultPath: lastDir ? `${lastDir}/${fileName}` : fileName,
    filters: [{ name: "PNG", extensions: ["png"] }],
    canCreateDirectories: true,
  });
  const path = picked?.trim();
  if (!path) return null;
  const slash = path.lastIndexOf("/");
  if (slash > 0) lastDir = path.slice(0, slash);
  return /\.png$/i.test(path) ? path : `${path}.png`;
}

/**
 * Export PNG…: a 2× PNG of the chart in the current theme. The desktop app asks where to save it
 * (written by `write_binary_file`); the browser downloads it.
 */
export async function exportChartPng(scene: Scene): Promise<void> {
  const name = chartFileName(scene, "png");
  try {
    if (isMock) {
      const base64 = await svgToPngBase64(
        sceneToSvg(scene, currentTheme()),
        scene.width,
        scene.height,
      );
      const a = document.createElement("a");
      a.href = `data:image/png;base64,${base64}`;
      a.download = name;
      a.click();
      return;
    }
    const path = await pickPngPath(name);
    if (!path) return;
    const base64 = await svgToPngBase64(
      sceneToSvg(scene, currentTheme()),
      scene.width,
      scene.height,
    );
    await ipc.diagram.writeBinaryFile(path, base64);
    toast.success(`Exported ${path.split("/").pop() ?? name}`);
  } catch (e) {
    toast.error("Could not export the chart", { description: toIpcError(e).message });
  }
}
