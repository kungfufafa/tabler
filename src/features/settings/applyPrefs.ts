import { UI_FONT_SIZES, type UiFontSize } from "@/stores/prefs";

/**
 * Applies the interface text size. The type scale is rem-based around a 13px body (text-sm), so the
 * root font-size is scaled by size/13: "large" makes rem-based text 14px and spacing grows with it.
 * `--ui-font-size` and `data-ui-font-size` are exposed for px-based chrome that wants to follow.
 */
export function applyUiFontSize(
  size: UiFontSize,
  root: HTMLElement = document.documentElement,
): void {
  const px = UI_FONT_SIZES[size];
  root.style.setProperty("--ui-font-size", `${px}px`);
  root.style.fontSize = size === "default" ? "" : `${(px / UI_FONT_SIZES.default) * 100}%`;
  root.dataset.uiFontSize = size;
}
