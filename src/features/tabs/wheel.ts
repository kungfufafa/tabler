const LINE_HEIGHT_PX = 16;

/**
 * Horizontal scroll delta for a wheel event over the tab strip: a vertical mouse wheel scrolls the
 * tabs sideways. Returns 0 when the gesture is already horizontal (trackpads scroll natively).
 */
export function wheelToScrollLeft(
  e: Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode">,
  pageWidth: number,
): number {
  if (Math.abs(e.deltaX) >= Math.abs(e.deltaY)) return 0;
  const unit = e.deltaMode === 1 ? LINE_HEIGHT_PX : e.deltaMode === 2 ? pageWidth : 1;
  return e.deltaY * unit;
}
