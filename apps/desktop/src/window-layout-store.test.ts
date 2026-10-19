// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { loadWindowLayouts, saveWindowLayouts, type WindowLayoutSnapshot } from "./window-layout-store";

describe("window layout store", () => {
  beforeEach(() => localStorage.clear());

  it("round-trips snapshots and removes malformed window entries", () => {
    const snapshot: WindowLayoutSnapshot = {
      id: "layout-1", name: "Work", capturedAt: 123,
      windows: [{ title: "Editor", processName: "code.exe", className: "Chrome_WidgetWin_1", occurrence: 1, rect: { left: 0, top: 0, right: 800, bottom: 600 }, minimized: false, maximized: false, topmost: true }]
    };
    saveWindowLayouts([snapshot, { ...snapshot, id: "bad", windows: [{ title: "bad", processName: "", className: "", rect: null as never, minimized: false, maximized: false, topmost: false }] }]);
    expect(loadWindowLayouts()).toEqual([snapshot, { ...snapshot, id: "bad", windows: [] }]);
  });

  it("caps persisted snapshots and windows", () => {
    const window = { title: "Editor", processName: "code.exe", className: "Chrome_WidgetWin_1", rect: { left: 0, top: 0, right: 800, bottom: 600 }, minimized: false, maximized: false, topmost: false };
    const layouts = Array.from({ length: 26 }, (_, index) => ({ id: `layout-${index}`, name: `Layout ${index}`, capturedAt: index, windows: Array.from({ length: 70 }, () => window) }));
    saveWindowLayouts(layouts);
    const loaded = loadWindowLayouts();
    expect(loaded).toHaveLength(24);
    expect(loaded[0].windows).toHaveLength(64);
  });

  it("preserves minimized state and native placement while defaulting old snapshots to normal", () => {
    const stored: WindowLayoutSnapshot[] = [{
      id: "layout-1", name: "Layout", capturedAt: 1,
      windows: [{ title: "Editor", processName: "code.exe", className: "Code", rect: { left: 10, top: 20, right: 810, bottom: 620 }, placementRect: { left: 8, top: 10, right: 808, bottom: 610 }, minimized: true, maximized: false, topmost: false }]
    }, {
      id: "layout-2", name: "Legacy", capturedAt: 2,
      windows: [{ title: "Browser", processName: "msedge.exe", className: "Chrome_WidgetWin_1", rect: { left: 0, top: 0, right: 800, bottom: 600 }, minimized: false, maximized: false, topmost: false }]
    }];
    localStorage.setItem("convenient-window-layouts", JSON.stringify(stored));

    const loaded = loadWindowLayouts();
    expect(loaded[0].windows[0]).toMatchObject({ minimized: true, placementRect: stored[0].windows[0].placementRect });
    expect(loaded[1].windows[0]).toMatchObject({ minimized: false });
  });

  it("round-trips shortcut and zIndex while dropping invalid values", () => {
    const window = { title: "Editor", processName: "code.exe", className: "Code", rect: { left: 0, top: 0, right: 800, bottom: 600 }, minimized: false, maximized: false, topmost: false };
    saveWindowLayouts([{
      id: "layout-1", name: "Layout", capturedAt: 1, shortcut: "alt+ctrl+f5",
      windows: [{ ...window, zIndex: 0 }, { ...window, zIndex: 3 }, { ...window, zIndex: -1 as never }, { ...window, zIndex: 1.5 as never }]
    }]);
    const loaded = loadWindowLayouts();
    expect(loaded[0].shortcut).toBe("Ctrl+Alt+F5");
    expect(loaded[0].windows.map((item) => item.zIndex)).toEqual([0, 3, undefined, undefined]);
  });

  it("drops malformed shortcuts both when saving and when loading", () => {
    const window = { title: "Editor", processName: "code.exe", className: "Code", rect: { left: 0, top: 0, right: 800, bottom: 600 }, minimized: false, maximized: false, topmost: false };
    saveWindowLayouts([
      { id: "layout-1", name: "Layout", capturedAt: 1, shortcut: "Ctrl+Space", windows: [window] },
      { id: "layout-2", name: "Layout", capturedAt: 2, shortcut: "win+shift+2", windows: [window] }
    ]);
    const saved = loadWindowLayouts();
    expect("shortcut" in saved[0]).toBe(false);
    expect(saved[1].shortcut).toBe("Shift+Win+2");

    localStorage.setItem("convenient-window-layouts", JSON.stringify([
      { id: "raw-1", name: "Raw", capturedAt: 4, shortcut: "ctrl+alt+9", windows: [] },
      { id: "raw-2", name: "Raw", capturedAt: 5, shortcut: "Ctrl+1+2", windows: [] }
    ]));
    const loaded = loadWindowLayouts();
    expect(loaded[0].shortcut).toBe("Ctrl+Alt+9");
    expect("shortcut" in loaded[1]).toBe(false);
  });
});
