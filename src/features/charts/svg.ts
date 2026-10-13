import type { Paint, Scene, SceneNode } from "./scene";

/**
 * Chart colors. On screen every role is a CSS custom property (`charts.css` defines the series
 * colors per theme; the chrome uses the app's tokens), so the chart follows light / dark. A file
 * (Copy as SVG, Export PNG) cannot see the app's CSS, so it gets the same colors as fixed values
 * for the theme in use.
 */

export type ChartTheme = "light" | "dark";

/** The CSS value of a role on screen. */
export function paintVar(p: Paint): string {
  switch (p) {
    case "fg":
      return "var(--foreground)";
    case "muted":
      return "var(--muted-foreground)";
    case "grid":
      return "var(--gridline)";
    case "axis":
      return "var(--border-strong)";
    case "surface":
      return "var(--surface)";
    case "hover":
      return "var(--foreground)";
    case "other":
      return "var(--chart-other)";
    default:
      return `var(--chart-${p})`;
  }
}

/**
 * Fixed colors per theme: the app's surface / text / line tokens converted to sRGB, and the
 * categorical palette of `charts.css` (validated for color-vision deficiencies on both surfaces).
 */
export const FILE_COLORS: Record<ChartTheme, Record<Paint, string>> = {
  light: {
    fg: "#181b1f",
    muted: "#6e7278",
    grid: "#e8e8e6",
    axis: "#cbcac8",
    surface: "#ffffff",
    hover: "#181b1f",
    other: "#b4b3ad",
    "series-1": "#2a78d6",
    "series-2": "#eb6834",
    "series-3": "#1baf7a",
    "series-4": "#eda100",
    "series-5": "#e87ba4",
    "series-6": "#008300",
    "series-7": "#4a3aa7",
    "series-8": "#e34948",
  },
  dark: {
    fg: "#e6e4e1",
    muted: "#94928d",
    grid: "#292a2c",
    axis: "#46484a",
    surface: "#1f2022",
    hover: "#e6e4e1",
    other: "#6b6a66",
    "series-1": "#3987e5",
    "series-2": "#d95926",
    "series-3": "#199e70",
    "series-4": "#c98500",
    "series-5": "#d55181",
    "series-6": "#008300",
    "series-7": "#9085e9",
    "series-8": "#e66767",
  },
};

const FONT_FAMILY = "-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif";

/**
 * What XML 1.0 cannot hold at all, not even escaped: C0 controls other than tab / LF / CR, lone
 * surrogates and U+FFFE / U+FFFF. One of them in a label makes the whole SVG unreadable.
 */
const XML_INVALID = /[^\t\n\r\u0020-\uD7FF\uE000-\uFFFD\u{10000}-\u{10FFFF}]/gu;

/**
 * XML-escapes text and attribute values (labels come from the database), dropping the characters
 * XML does not allow.
 */
export function xmlEscape(s: string): string {
  return s
    .replace(XML_INVALID, "")
    .replace(/[&<>"']/g, (c) =>
      c === "&"
        ? "&amp;"
        : c === "<"
          ? "&lt;"
          : c === ">"
            ? "&gt;"
            : c === '"'
              ? "&quot;"
              : "&apos;",
    );
}

function nodeSvg(n: SceneNode, color: (p: Paint) => string): string {
  switch (n.t) {
    case "rect":
      return `<rect x="${n.x}" y="${n.y}" width="${n.w}" height="${n.h}"${n.rx ? ` rx="${n.rx}"` : ""} fill="${color(n.fill)}"/>`;
    case "path":
      return `<path d="${n.d}" fill="${n.fill === "none" ? "none" : color(n.fill)}"${
        n.stroke
          ? ` stroke="${color(n.stroke)}" stroke-width="${n.strokeWidth ?? 1}" stroke-linejoin="round" stroke-linecap="round"`
          : ""
      }/>`;
    case "line":
      return `<line x1="${n.x1}" y1="${n.y1}" x2="${n.x2}" y2="${n.y2}" stroke="${color(n.stroke)}" stroke-width="1" shape-rendering="crispEdges"/>`;
    case "circle":
      return `<circle cx="${n.cx}" cy="${n.cy}" r="${n.r}" fill="${color(n.fill)}"${
        n.stroke ? ` stroke="${color(n.stroke)}" stroke-width="${n.strokeWidth ?? 1}"` : ""
      }/>`;
    case "text":
      return `<text x="${n.x}" y="${n.y}" fill="${color(n.fill)}" font-size="${n.size}"${
        n.weight ? ` font-weight="${n.weight}"` : ""
      }${n.anchor === "start" ? "" : ` text-anchor="${n.anchor}"`}>${xmlEscape(n.text)}</text>`;
  }
}

/** The scene as a standalone SVG document in `theme`'s colors, on its surface. */
export function sceneToSvg(scene: Scene, theme: ChartTheme): string {
  const colors = FILE_COLORS[theme];
  const color = (p: Paint) => colors[p];
  const { width, height } = scene;
  const body = scene.nodes.map((n) => nodeSvg(n, color)).join("\n  ");
  return [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" font-family="${xmlEscape(FONT_FAMILY)}" role="img" aria-label="${xmlEscape(scene.title)}">`,
    `  <title>${xmlEscape(scene.title)}</title>`,
    `  <rect width="${width}" height="${height}" fill="${colors.surface}"/>`,
    `  ${body}`,
    `</svg>`,
  ].join("\n");
}
