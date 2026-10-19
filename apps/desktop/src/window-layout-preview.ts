import type { WindowLayoutRect, WindowLayoutWindow } from "./window-layout-store";

/** Area (virtual-desktop coordinates) a layout thumbnail is drawn into. */
export interface WindowLayoutPreviewBounds {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Slivers below this size on either axis are not meaningful as a window preview. */
const MIN_VISIBLE_SIZE = 4;
/** A 64px-wide card cannot show more distinct windows without turning into noise. */
const MAX_PREVIEW_WINDOWS = 24;

/**
 * Area covered by the windows the user can actually see: minimized, sliver-sized
 * and malformed windows are ignored. Falls back to a unit rectangle for layouts
 * that have nothing visible, so callers never divide by zero.
 */
export function previewBounds(windows: WindowLayoutWindow[]): WindowLayoutPreviewBounds {
  const visible = windows.filter(isDrawableWindow);
  if (!visible.length) return { left: 0, top: 0, right: 1, bottom: 1 };
  const first = visible[0].rect;
  return visible.reduce<WindowLayoutPreviewBounds>((bounds, item) => ({
    left: Math.min(bounds.left, item.rect.left),
    top: Math.min(bounds.top, item.rect.top),
    right: Math.max(bounds.right, item.rect.right),
    bottom: Math.max(bounds.bottom, item.rect.bottom)
  }), { left: first.left, top: first.top, right: first.right, bottom: first.bottom });
}

/**
 * Windows to paint on a layout thumbnail, in paint order (bottom first, top last).
 *
 * - minimized windows are skipped: they are not on the desktop;
 * - sliver-sized and malformed windows are skipped;
 * - windows that do not overlap the preview area at all are skipped;
 * - the helper reports `zIndex` with 0 = topmost, so painting sorts by
 *   `zIndex` descending: the topmost windows are painted last and their opaque
 *   fill naturally hides the ones behind them;
 * - snapshots captured before `zIndex` existed sort last (same as -1) and keep
 *   the captured array order thanks to the stable sort;
 * - at most 24 windows are painted, keeping the topmost ones when the desktop
 *   has more, because those are the windows the user actually sees.
 */
export function previewWindows(windows: WindowLayoutWindow[], bounds: WindowLayoutPreviewBounds): WindowLayoutWindow[] {
  const area = isSaneBounds(bounds) ? bounds : previewBounds(windows);
  const visible = windows.filter((item) => isDrawableWindow(item) && intersects(area, item.rect));
  const ordered = visible.slice().sort((left, right) => zIndexOf(right) - zIndexOf(left));
  return ordered.slice(-MAX_PREVIEW_WINDOWS);
}

function zIndexOf(item: WindowLayoutWindow): number {
  const value = item.zIndex;
  return typeof value === "number" && Number.isFinite(value) ? value : -1;
}

function isDrawableWindow(item: WindowLayoutWindow): boolean {
  if (!item || item.minimized === true) return false;
  return isSaneRect(item.rect) && item.rect.right - item.rect.left > MIN_VISIBLE_SIZE && item.rect.bottom - item.rect.top > MIN_VISIBLE_SIZE;
}

function isSaneRect(rect: WindowLayoutRect | WindowLayoutPreviewBounds | undefined): rect is WindowLayoutRect {
  if (!rect) return false;
  const values = [rect.left, rect.top, rect.right, rect.bottom];
  return values.every((value) => typeof value === "number" && Number.isFinite(value))
    && rect.right > rect.left && rect.bottom > rect.top;
}

function isSaneBounds(bounds: WindowLayoutPreviewBounds | undefined): bounds is WindowLayoutPreviewBounds {
  return isSaneRect(bounds);
}

function intersects(area: WindowLayoutPreviewBounds, rect: WindowLayoutRect): boolean {
  return rect.right > area.left && rect.left < area.right && rect.bottom > area.top && rect.top < area.bottom;
}
