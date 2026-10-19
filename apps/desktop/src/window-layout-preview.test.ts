import { describe, expect, it } from "vitest";
import { previewBounds, previewWindows } from "./window-layout-preview";
import type { WindowLayoutWindow } from "./window-layout-store";

const DESKTOP = { left: 0, top: 0, right: 2560, bottom: 1400 };

function windowAt(left: number, top: number, right: number, bottom: number, extra: Partial<WindowLayoutWindow> = {}): WindowLayoutWindow {
  return {
    title: `window-${left}-${top}`,
    processName: "app.exe",
    className: "Win32Window",
    rect: { left, top, right, bottom },
    minimized: false,
    maximized: false,
    topmost: false,
    ...extra
  };
}

describe("window layout preview", () => {
  it("only paints windows that are visible on the desktop", () => {
    const visible = windowAt(0, 0, 800, 600);
    const minimized = windowAt(100, 100, 900, 700, { minimized: true });
    const sliver = windowAt(0, 0, 4, 600);
    const offscreen = windowAt(3000, 2000, 3200, 2400);
    const touchingEdge = windowAt(2560, 0, 3000, 600);

    expect(previewWindows([visible, minimized, sliver, offscreen, touchingEdge], DESKTOP)).toEqual([visible]);
  });

  it("keeps windows that are only partially inside the preview area", () => {
    const partial = windowAt(-200, -100, 400, 300);
    expect(previewWindows([partial], DESKTOP)).toEqual([partial]);
  });

  it("paints bottom to top so topmost windows cover the ones behind", () => {
    const top = windowAt(0, 0, 800, 600, { title: "top", zIndex: 0 });
    const middle = windowAt(0, 0, 800, 600, { title: "middle", zIndex: 1 });
    const bottom = windowAt(0, 0, 800, 600, { title: "bottom", zIndex: 2 });
    const input = [top, middle, bottom];

    expect(previewWindows(input, DESKTOP)).toEqual([bottom, middle, top]);
    expect(input).toEqual([top, middle, bottom]);
  });

  it("keeps the captured order for legacy snapshots without zIndex", () => {
    const first = windowAt(0, 0, 400, 300);
    const second = windowAt(100, 100, 500, 400);
    const third = windowAt(200, 200, 600, 500);

    expect(previewWindows([first, second, third], DESKTOP)).toEqual([first, second, third]);
  });

  it("draws at most 24 windows and keeps the topmost ones", () => {
    const windows = Array.from({ length: 30 }, (_, index) => windowAt(
      (index % 6) * 400, Math.floor(index / 6) * 280, (index % 6) * 400 + 380, Math.floor(index / 6) * 280 + 260, { zIndex: index }
    ));

    const preview = previewWindows(windows, DESKTOP);
    expect(preview).toHaveLength(24);
    expect(preview.map((item) => item.zIndex)).toEqual(Array.from({ length: 24 }, (_, index) => 23 - index));
    expect(preview.at(-1)?.zIndex).toBe(0);
  });

  it("derives bounds from the visible rects only", () => {
    const first = windowAt(100, 50, 900, 650);
    const second = windowAt(1200, 100, 2000, 700);
    const minimized = windowAt(-32000, -32000, -31900, -31900, { minimized: true });

    expect(previewBounds([first, minimized, second])).toEqual({ left: 100, top: 50, right: 2000, bottom: 700 });
    expect(previewBounds([minimized])).toEqual({ left: 0, top: 0, right: 1, bottom: 1 });
    expect(previewBounds([])).toEqual({ left: 0, top: 0, right: 1, bottom: 1 });
  });

  it("ignores placementRect: the preview shows the rect currently on the desktop", () => {
    const snapped = windowAt(100, 100, 900, 700, { placementRect: { left: -30000, top: -30000, right: -29900, bottom: -29900 } });

    expect(previewBounds([snapped])).toEqual({ left: 100, top: 100, right: 900, bottom: 700 });
    expect(previewWindows([snapped], DESKTOP)).toEqual([snapped]);
  });

  it("falls back to derived bounds when the supplied bounds are unusable", () => {
    const item = windowAt(0, 0, 800, 600);
    expect(previewWindows([item], { left: 0, top: 0, right: 0, bottom: 0 })).toEqual([item]);
  });
});
