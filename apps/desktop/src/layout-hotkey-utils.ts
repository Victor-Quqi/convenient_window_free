// 布局全局快捷键的纯函数工具：解析、规范化与宿主 accelerator 转换。
//
// 两套宿主（uTools 插件与独立桌面）的 src/layout-hotkey-utils.ts 必须逐字节一致。
//
// 存储格式固定为 "Ctrl+Alt+1" 这类字符串：修饰键固定顺序 Ctrl、Alt、Shift、Win，
// 主键只允许 A-Z / 0-9 / F1..F12，且至少包含一个修饰键。真正注册到宿主时再经
// toHostAccelerator 转成 Electron 与 Tauri（global-hotkey）都可用的 accelerator，
// 两者都用 "Super" 表示 Win 键。
//
// 这里刻意只允许有限的主键集合：helper 的键盘解析（parse_key / parse_ascii_key）
// 也只认这些键，放开标点、方向键之类只会让快捷键录进去却执行不了。

/** 规范修饰键顺序；与 shortcut-keys.ts 的 MODIFIER_ORDER 一致。 */
const MODIFIER_ORDER = ["Ctrl", "Alt", "Shift", "Win"] as const;

type LayoutModifier = (typeof MODIFIER_ORDER)[number];

/** 修饰键大小写不敏感；Win 不接受 Super/Meta 别名，避免出现第二个存储写法。 */
const MODIFIER_ALIASES: Record<string, LayoutModifier> = {
  ctrl: "Ctrl",
  alt: "Alt",
  shift: "Shift",
  win: "Win"
};

/** 主键本身（不含修饰键）的规范化：A-Z、0-9、F1-F12。 */
function normalizeMainKey(value: string): string | null {
  if (/^[A-Za-z]$/.test(value)) return value.toUpperCase();
  if (/^[0-9]$/.test(value)) return value;
  const functionKey = /^[Ff]([1-9]|1[0-2])$/.exec(value);
  if (functionKey) return `F${functionKey[1]}`;
  return null;
}

/**
 * 把未知输入规范成存储/展示用的快捷键字符串；非法输入返回 null。
 *
 * - 修饰键顺序固定为 Ctrl、Alt、Shift、Win；
 * - 至少一个修饰键，且只能有一个主键；
 * - 拒绝未知修饰键、未知主键、重复修饰键、空片段。
 */
export function normalizeLayoutShortcut(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const parts = value.split("+").map((part) => part.trim());
  if (parts.length < 2) return null;
  const modifiers: LayoutModifier[] = [];
  let mainKey: string | null = null;
  for (const part of parts) {
    if (!part) return null;
    const modifier = MODIFIER_ALIASES[part.toLowerCase()];
    if (modifier) {
      if (modifiers.includes(modifier)) return null;
      modifiers.push(modifier);
      continue;
    }
    if (mainKey !== null) return null;
    const normalized = normalizeMainKey(part);
    if (!normalized) return null;
    mainKey = normalized;
  }
  if (mainKey === null) return null;
  const ordered = MODIFIER_ORDER.filter((modifier) => modifiers.includes(modifier));
  return [...ordered, mainKey].join("+");
}

/**
 * 把规范化后的布局快捷键转成宿主 accelerator：Win -> Super。
 * 例如 "Ctrl+Alt+Win+1" -> "Ctrl+Alt+Super+1"。
 * 非法输入返回 null（调用方据此跳过注册）。
 */
export function toHostAccelerator(shortcut: string): string | null {
  const normalized = normalizeLayoutShortcut(shortcut);
  if (!normalized) return null;
  return normalized
    .split("+")
    .map((part) => (part === "Win" ? "Super" : part))
    .join("+");
}

const MODIFIER_KEYS: ReadonlySet<string> = new Set(["Control", "Alt", "Shift", "Meta"]);

/** 从键盘事件里取出按住的修饰键，顺序固定。 */
function eventModifiers(event: KeyboardEvent): LayoutModifier[] {
  return MODIFIER_ORDER.filter((modifier) =>
    (modifier === "Ctrl" && event.ctrlKey) ||
    (modifier === "Alt" && event.altKey) ||
    (modifier === "Shift" && event.shiftKey) ||
    (modifier === "Win" && event.metaKey)
  );
}

/** 从键盘事件里解析主键；修饰键本身与不支持的键返回 null。 */
function eventMainKey(event: KeyboardEvent): string | null {
  if (event.key === "Escape" || event.key === "AltGraph" || event.key === "Unidentified") return null;
  if (MODIFIER_KEYS.has(event.key)) return null;
  // 优先用 e.code 取物理键位：e.key 在非 QWERTY 布局（如 AZERTY）下会返回布局字符，
  // 会让录到的快捷键和实际按下的键对不上。
  const code = typeof event.code === "string" ? event.code : "";
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  const functionKey = /^F([1-9]|1[0-2])$/.exec(code);
  if (functionKey) return `F${functionKey[1]}`;
  // 某些嵌入环境（webview / Electron）里 e.code 可能缺失或不带前缀，退回 e.key；
  // 仍然只接受同一组主键，不会放行 helper 不支持的键。
  return normalizeMainKey(typeof event.key === "string" ? event.key : "");
}

/**
 * 把一次按键事件转成可直接保存的布局快捷键。
 *
 * 至少一个修饰键 + A-Z / 0-9 / F1..F12 主键才算有效；修饰键本身、无修饰键、
 * Escape 或其它不支持的键返回 null（录制 UI 据此继续等待或提示）。
 */
export function layoutShortcutFromKeyboardEvent(event: KeyboardEvent): string | null {
  if (!event || event.key === "Escape") return null;
  const mainKey = eventMainKey(event);
  if (!mainKey) return null;
  const modifiers = eventModifiers(event);
  if (!modifiers.length) return null;
  return normalizeLayoutShortcut([...modifiers, mainKey].join("+"));
}
