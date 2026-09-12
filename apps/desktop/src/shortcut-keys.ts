import type { ModifierKey } from "./types";

// 快捷键键名表：必须与 helper 的 parse_key 严格对齐。
// 权威实现见 open-source/helper/src/platform/windows/keyboard.rs：
//   - 命名键只认 ctrl/shift/alt/win、enter/return、esc/escape、tab、space、
//     left/right/up/down、volumeup/volumedown/volumemute、mediaplaypause、f1..f12
//   - 其余交给 parse_ascii_key，那里要求「长度恰好为 1 且是 ASCII 字母或数字」
// 因此标点（/ ; [ 等）、Backspace/Delete/Home/PageUp、小键盘数字都不受支持，
// 录进去也只会在执行时报 unsupported shortcut key，所以这里直接不接收。
export const MODIFIER_ORDER: ModifierKey[] = ["ctrl", "alt", "shift", "win"];

export const MODIFIER_NAMES: Record<ModifierKey, string> = {
  ctrl: "Ctrl",
  alt: "Alt",
  shift: "Shift",
  win: "Win"
};

export const NAMED_KEYS: Record<string, string> = {
  " ": "Space",
  Enter: "Enter",
  Tab: "Tab",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right"
};

/** 当前按住的修饰键，顺序固定为 Ctrl、Alt、Shift、Win。 */
export function heldModifiers(event: KeyboardEvent): ModifierKey[] {
  return MODIFIER_ORDER.filter((key) =>
    (key === "ctrl" && event.ctrlKey) ||
    (key === "alt" && event.altKey) ||
    (key === "shift" && event.shiftKey) ||
    (key === "win" && event.metaKey)
  );
}

/** 该事件是否是修饰键本身（此时只更新提示，不结束录制）。 */
export function isModifierKey(event: KeyboardEvent): boolean {
  return ["Control", "Alt", "Shift", "Meta"].includes(event.key);
}

/**
 * 解析主键名；helper 不支持的键返回 null（表示不录制）。
 * 用 e.code 取物理键位：e.key 在非 QWERTY 布局（如 AZERTY）下会返回布局字符，
 * 会让录到的快捷键和实际按下的键对不上。
 */
export function resolveKeyName(event: KeyboardEvent): string | null {
  if (NAMED_KEYS[event.key]) return NAMED_KEYS[event.key];
  if (isModifierKey(event) || event.key === "AltGraph") return null;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.key)) return event.key.toUpperCase();
  if (/^Key[A-Z]$/.test(event.code)) return event.code.slice(3);
  if (/^Digit[0-9]$/.test(event.code)) return event.code.slice(5);
  return null;
}

/** 组装显示与存储用的快捷键字符串，修饰键在前。 */
export function formatShortcut(modifiers: ModifierKey[], key: string): string {
  return [...modifiers.map((item) => MODIFIER_NAMES[item]), key].join("+");
}

/** 录制过程中按键处理的三种结果。 */
export type KeyStep =
  /** 仍在等待：只按住了修饰键，或按到了 helper 不支持的键。 */
  | { action: "wait"; hint: string }
  /** 用户按 Escape 放弃录制。 */
  | { action: "cancel" }
  /** 录制完成，value 为可直接保存的快捷键字符串。 */
  | { action: "commit"; value: string };

/**
 * 录制状态下处理一次按键。纯函数，便于直接测试完整按键序列。
 * 不在这里判断"是否正在录制"——那是调用方的职责。
 */
export function nextRecordingStep(event: KeyboardEvent): KeyStep {
  if (event.key === "Escape") return { action: "cancel" };
  const modifiers = heldModifiers(event);
  if (isModifierKey(event)) {
    return { action: "wait", hint: modifiers.map((key) => MODIFIER_NAMES[key]).join("+") };
  }
  const key = resolveKeyName(event);
  if (!key) return { action: "wait", hint: "" };
  return { action: "commit", value: formatShortcut(modifiers, key) };
}
