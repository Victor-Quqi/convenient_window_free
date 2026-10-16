import type { UiKey } from "./i18n";
import type { GestureTemplate } from "./types";

export const builtinGestureKeys = {
  "gesture-up": "gestureUp",
  "gesture-down": "gestureDown",
  "gesture-l": "gestureL",
  "gesture-circle": "gestureCircle",
  "gesture-rectangle": "gestureRectangle"
} satisfies Record<string, UiKey>;

// Only these historical defaults are removed during migration. User overrides survive.
const legacyNames: Record<string, readonly string[]> = {
  "gesture-up": ["向上 · 复制", "Up · Copy"],
  "gesture-down": ["向下 · 粘贴", "Down · Paste"],
  "gesture-l": ["L 型 · 关闭窗口", "L · Close"],
  "gesture-circle": ["圆圈 · 切换窗口置顶", "圆圈 · 显示桌面", "Circle · Topmost"],
  "gesture-rectangle": ["矩形截图", "Region capture"]
};

export function isBuiltinGesture(id: string): id is keyof typeof builtinGestureKeys {
  return Object.hasOwn(builtinGestureKeys, id);
}

export function migrateGestureName(id: string, name: string | undefined, schema: number): string | undefined {
  return schema < 8 && Object.hasOwn(legacyNames, id) && legacyNames[id].includes(name ?? "") ? undefined : name;
}

export function gestureDisplayName(gesture: Pick<GestureTemplate, "id" | "name">, ui: (key: UiKey) => string): string {
  return gesture.name || (isBuiltinGesture(gesture.id) ? ui(builtinGestureKeys[gesture.id]) : ui("customGesture"));
}

export function gestureLabels(gestures: GestureTemplate[], ui: (key: UiKey) => string): Record<string, string> {
  return Object.fromEntries(gestures.map(gesture => [gesture.id, gestureOverlayName(gesture, ui)]));
}

export function gestureOverlayName(gesture: GestureTemplate, ui: (key: UiKey) => string): string {
  if (gesture.name) return gesture.name;
  if (gesture.mode === "region-screenshot") return ui("regionScreenshot");
  const action = gesture.action;
  if (action.kind === "shortcut") {
    const names: Record<string, UiKey> = {
      "Ctrl+C": "gestureActionCopy", "Ctrl+V": "gestureActionPaste", "Alt+F4": "gestureActionClose",
      "Win+Down": "minimizeWindow", "Win+Up": "maximizeWindow"
    };
    return Object.hasOwn(names, action.value ?? "") ? ui(names[action.value!]) : (action.value ?? "");
  }
  const names: Record<string, UiKey> = {
    "show-desktop": "gestureActionDesktop", "toggle-window-topmost": "gestureActionTopmost",
    "lock-screen": "gestureActionLock", "open-command": "gestureActionCommand",
    "volume-adjust": Number(action.value) < 0 ? "gestureActionVolumeDown" : "gestureActionVolumeUp",
    "brightness-adjust": Number(action.value) < 0 ? "gestureActionBrightnessDown" : "gestureActionBrightnessUp"
  };
  return Object.hasOwn(names, action.kind) ? ui(names[action.kind]) : "";
}
