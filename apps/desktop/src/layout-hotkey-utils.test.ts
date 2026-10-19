import { describe, expect, it } from "vitest";
import {
  layoutShortcutFromKeyboardEvent,
  normalizeLayoutShortcut,
  toHostAccelerator
} from "./layout-hotkey-utils";

// 纯函数只读 KeyboardEvent 的 key/code 与四个修饰键布尔值，用普通对象即可覆盖，
// 不依赖具体 DOM 实现（两套前端运行同一个测试文件）。
function keyEvent(init: {
  key: string;
  code?: string;
  ctrl?: boolean;
  alt?: boolean;
  shift?: boolean;
  meta?: boolean;
}): KeyboardEvent {
  return {
    key: init.key,
    code: init.code ?? "",
    ctrlKey: Boolean(init.ctrl),
    altKey: Boolean(init.alt),
    shiftKey: Boolean(init.shift),
    metaKey: Boolean(init.meta)
  } as KeyboardEvent;
}

describe("normalizeLayoutShortcut", () => {
  it("normalizes modifier order and key case", () => {
    expect(normalizeLayoutShortcut("alt+ctrl+1")).toBe("Ctrl+Alt+1");
    expect(normalizeLayoutShortcut("win+shift+a")).toBe("Shift+Win+A");
    expect(normalizeLayoutShortcut("Ctrl+Alt+Shift+Win+f12")).toBe("Ctrl+Alt+Shift+Win+F12");
    expect(normalizeLayoutShortcut("  Ctrl + Alt + 0 ")).toBe("Ctrl+Alt+0");
  });

  it("rejects shortcuts without a modifier or with an unknown main key", () => {
    expect(normalizeLayoutShortcut("A")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+Alt")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+Space")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+Enter")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+ArrowUp")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+F13")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+;")).toBeNull();
  });

  it("rejects duplicate or conflicting parts", () => {
    expect(normalizeLayoutShortcut("Ctrl+Ctrl+1")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+Alt+A+B")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl++1")).toBeNull();
    expect(normalizeLayoutShortcut("Ctrl+Alt+")).toBeNull();
  });

  it("rejects non-string and empty input", () => {
    expect(normalizeLayoutShortcut(null)).toBeNull();
    expect(normalizeLayoutShortcut(42)).toBeNull();
    expect(normalizeLayoutShortcut("")).toBeNull();
    expect(normalizeLayoutShortcut("   ")).toBeNull();
  });
});

describe("toHostAccelerator", () => {
  it("renders Win as Super for both hosts", () => {
    expect(toHostAccelerator("Ctrl+Alt+1")).toBe("Ctrl+Alt+1");
    expect(toHostAccelerator("Ctrl+Alt+Win+1")).toBe("Ctrl+Alt+Super+1");
    expect(toHostAccelerator("win+shift+f2")).toBe("Shift+Super+F2");
  });

  it("returns null for shortcuts that do not normalize", () => {
    expect(toHostAccelerator("Alt")).toBeNull();
    expect(toHostAccelerator("Ctrl+Space")).toBeNull();
  });
});

describe("layoutShortcutFromKeyboardEvent", () => {
  it("records modifier combinations from e.code", () => {
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "1", code: "Digit1", ctrl: true }))).toBe("Ctrl+1");
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "a", code: "KeyA", ctrl: true, alt: true }))).toBe("Ctrl+Alt+A");
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "F4", code: "F4", alt: true }))).toBe("Alt+F4");
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "z", code: "KeyZ", ctrl: true, shift: true, meta: true }))).toBe("Ctrl+Shift+Win+Z");
  });

  it("falls back to e.key when e.code is missing or prefixed differently", () => {
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "g", ctrl: true }))).toBe("Ctrl+G");
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "7", alt: true }))).toBe("Alt+7");
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "f12", ctrl: true }))).toBe("Ctrl+F12");
  });

  it("ignores modifier-only keys and unsupported keys", () => {
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "Control", code: "ControlLeft", ctrl: true }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "Shift", code: "ShiftLeft", ctrl: true, shift: true }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "Escape", code: "Escape", ctrl: true }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: ";", code: "Semicolon", ctrl: true }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "F13", code: "F13", ctrl: true }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "Enter", code: "Enter", ctrl: true }))).toBeNull();
  });

  it("requires at least one modifier", () => {
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "a", code: "KeyA" }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "1", code: "Digit1" }))).toBeNull();
    expect(layoutShortcutFromKeyboardEvent(keyEvent({ key: "F2", code: "F2" }))).toBeNull();
  });
});
