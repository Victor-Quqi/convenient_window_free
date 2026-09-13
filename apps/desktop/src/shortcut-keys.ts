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

/** 单个 ASCII 字母或数字才算 helper 能执行的键（parse_ascii_key 的要求）。 */
function isAsciiAlphaNumeric(value: string): boolean {
  return /^[A-Za-z0-9]$/.test(value);
}

/**
 * 解析主键名；helper 不支持的键返回 null（表示不录制）。
 *
 * 优先用 e.code 取物理键位：e.key 在非 QWERTY 布局（如 AZERTY）下会返回布局字符，
 * 会让录到的快捷键和实际按下的键对不上。
 * 但某些嵌入环境（webview / Electron）里 e.code 可能缺失或不带 "Key"/"Digit" 前缀，
 * 此时退回 e.key —— 只接受单个 ASCII 字母或数字，仍然不会放行 helper 不支持的键。
 */
export function resolveKeyName(event: KeyboardEvent): string | null {
  if (NAMED_KEYS[event.key]) return NAMED_KEYS[event.key];
  if (isModifierKey(event) || event.key === "AltGraph") return null;
  // 只认 F1..F12：helper 的 parse_key 只映射到 VK_F12，F13..F24 会报 unsupported shortcut key。
  if (/^F([1-9]|1[0-2])$/.test(event.key)) return event.key.toUpperCase();
  const code = typeof event.code === "string" ? event.code : "";
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  const key = typeof event.key === "string" ? event.key : "";
  if (isAsciiAlphaNumeric(key)) return key.toUpperCase();
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
