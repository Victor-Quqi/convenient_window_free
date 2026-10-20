// 窗口布局的排除应用名单（两套宿主共用，必须逐字节一致）。
//
// 名单只保存在宿主 UI 存储（localStorage / uTools dbStorage）里，和窗口布局快照同级；
// 它不进入 AppSettings，也不下发给 helper —— 过滤在宿主侧完成，记录、复位与缩略图
// 都只看到过滤后的窗口。
//
// 两套宿主（uTools 插件与独立桌面）的 src/window-layout-exclusions.ts 必须逐字节一致。

import { loadHostValue, saveHostValue } from "./host-storage";

/** 排除名单的宿主存储键；与 `convenient-window-layouts` 同级。 */
export const WINDOW_LAYOUT_EXCLUSIONS_KEY = "convenient-window-layout-exclusions";

/** 名单上限：与 helper 的窗口列表上限保持一致，避免 UI 里出现无法管理的条目。 */
export const MAX_LAYOUT_EXCLUSIONS = 64;

/** 只描述匹配需要的窗口身份字段，布局快照里的窗口对象天然满足。 */
export interface LayoutWindowIdentity {
  processName?: string;
  title?: string;
  className?: string;
}

/**
 * 规范化排除名单：
 * - 只接受字符串，非字符串条目丢弃；
 * - 条目 trim，空条目忽略；
 * - 大小写不敏感去重，保留第一次出现的原大小写；
 * - 最多保留 64 条。
 */
export function normalizeLayoutExclusions(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const result: string[] = [];
  const seen = new Set<string>();
  for (const item of value) {
    if (typeof item !== "string") continue;
    const entry = item.trim();
    if (!entry) continue;
    const key = entry.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(entry);
    if (result.length >= MAX_LAYOUT_EXCLUSIONS) break;
  }
  return result;
}

/**
 * 与 helper 的 `is_paused_window` 保持一致：进程名、窗口标题、窗口类名任一
 * 小写包含排除条目即视为排除；空名单、空条目永远不命中。
 */
export function isLayoutWindowExcluded(
  window: LayoutWindowIdentity | null | undefined,
  exclusions: readonly string[]
): boolean {
  if (!window || !exclusions.length) return false;
  const candidates = [window.processName, window.title, window.className]
    .filter((value): value is string => typeof value === "string" && value.length > 0)
    .map((value) => value.toLowerCase());
  if (!candidates.length) return false;
  return exclusions.some((entry) => {
    const needle = entry.trim().toLowerCase();
    return Boolean(needle) && candidates.some((candidate) => candidate.includes(needle));
  });
}

/** 过滤掉命中排除名单的窗口；返回值是新数组，不修改入参。 */
export function filterLayoutWindows<T extends LayoutWindowIdentity>(
  windows: readonly T[],
  exclusions: readonly string[]
): T[] {
  return windows.filter((window) => !isLayoutWindowExcluded(window, exclusions));
}

/** 读取宿主存储里的排除名单；缺失或损坏时返回空名单。 */
export function loadLayoutExclusions(): string[] {
  return normalizeLayoutExclusions(loadHostValue<unknown>(WINDOW_LAYOUT_EXCLUSIONS_KEY));
}

/** 规范化后写回宿主存储。 */
export function saveLayoutExclusions(value: string[]): void {
  saveHostValue(WINDOW_LAYOUT_EXCLUSIONS_KEY, normalizeLayoutExclusions(value));
}
