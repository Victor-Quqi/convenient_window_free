// 布局全局快捷键的 helper 客户端适配层（两套宿主共用，必须逐字节一致）。
//
// uTools 插件没有注册任意全局快捷键的 API（window.utools.registerGlobalShortcut
// 并不存在），Tauri 桌面也不再引入 global-shortcut 插件：两套宿主统一把
// 「布局 id + accelerator」列表全量同步给 helper，由 helper 常驻的低级键盘钩子
// 匹配，命中后回推 shortcut.hotkey.triggered 事件，前端再执行 window.layout.restore。
//
// 两套宿主（uTools 插件与独立桌面）的 src/layout-hotkeys.ts 必须逐字节一致。

import type { HelperClient } from "./helper-client";
import { toHostAccelerator } from "./layout-hotkey-utils";
import { loadWindowLayouts, type WindowLayoutSnapshot } from "./window-layout-store";

/** helper 命中快捷键后执行的回调；App.svelte 注入 helper.restoreWindowLayout。 */
export type LayoutHotkeyExecutor = (layout: WindowLayoutSnapshot) => void | Promise<void>;

/** 同步给 helper 的一条布局快捷键；accelerator 由 toHostAccelerator 生成（Win -> Super）。 */
export interface LayoutHotkeyBinding {
  id: string;
  accelerator: string;
}

let client: HelperClient | null = null;
let executor: LayoutHotkeyExecutor | null = null;
let offMessage: (() => void) | null = null;
let offStatus: (() => void) | null = null;
let disposed = false;
/** 串行化 sync：helper 侧是全量替换，并发请求乱序会把旧列表写回去。 */
let serial: Promise<unknown> = Promise.resolve();

/**
 * 保存 helper 与执行器、订阅触发/状态事件，并按当前存储的布局首次全量同步。
 *
 * 触发时重新读取存储里的最新快照（用户可能刚改过名称/窗口），找不到对应布局就丢弃。
 * helper.ready 之后协议才可用，而 onStatus("connected") 只代表 socket 已打开，
 * 所以连接恢复与 helper.ready 两个时机都会重新同步；重复调用会重建监听并再同步。
 */
export async function initLayoutHotkeys(helper: HelperClient, next: LayoutHotkeyExecutor): Promise<string[]> {
  disposeListeners();
  client = helper;
  executor = next;
  disposed = false;
  offMessage = helper.onMessage((message) => {
    if (message.type === "shortcut.hotkey.triggered") {
      const data = message.data && typeof message.data === "object" ? message.data as Record<string, unknown> : {};
      void triggerLayoutHotkey(data.hotkeyId);
    } else if (message.type === "helper.ready") {
      // 重连后 helper 的热键表是空的（按连接归属），握手完成就登记回去。
      void syncLayoutHotkeys();
    }
  });
  offStatus = helper.onStatus((status) => {
    if (status === "connected") void syncLayoutHotkeys();
  });
  return syncLayoutHotkeys();
}

/**
 * 把存储里的布局快捷键全量同步给 helper（helper 按连接替换整张表，空列表即注销全部）。
 *
 * 返回 helper 拒绝的 layout id；helper 未连接、超时或请求抛错时返回所有已配置快捷键
 * 的 id，让布局卡片照常显示告警。连接恢复后会再次同步，失败项因此自动重试。
 */
export function syncLayoutHotkeys(): Promise<string[]> {
  const run = serial.then(runSync, runSync);
  serial = run.then(() => undefined, () => undefined);
  return run;
}

/** 把空列表同步给 helper 并解除监听；App 卸载时调用。 */
export function disposeLayoutHotkeys(): void {
  disposed = true;
  executor = null;
  disposeListeners();
  const previous = client;
  client = null;
  if (previous) {
    // 卸载阶段不能等待网络往返：尽力发出空列表；连接随后断开时 helper 也会清理。
    void previous.syncShortcutHotkeys([]).catch(() => {});
  }
}

function currentBindings(): LayoutHotkeyBinding[] {
  const bindings: LayoutHotkeyBinding[] = [];
  for (const layout of loadWindowLayouts()) {
    const accelerator = layout.shortcut ? toHostAccelerator(layout.shortcut) : null;
    if (accelerator) bindings.push({ id: layout.id, accelerator });
  }
  return bindings;
}

async function runSync(): Promise<string[]> {
  const previous = client;
  if (!previous || disposed) return [];
  const bindings = currentBindings();
  try {
    const result = await previous.syncShortcutHotkeys(bindings);
    return result.rejected.map((item) => item.id).filter((id) => id.length > 0);
  } catch {
    return bindings.map((item) => item.id);
  }
}

async function triggerLayoutHotkey(hotkeyId: unknown): Promise<void> {
  const run = executor;
  if (!run || disposed || typeof hotkeyId !== "string" || !hotkeyId) return;
  const layout = loadWindowLayouts().find((item) => item.id === hotkeyId);
  if (!layout) return;
  try {
    await run(layout);
  } catch {
    // 全局快捷键在后台触发，没有可展示错误的界面；静默失败。
  }
}

function disposeListeners(): void {
  offMessage?.();
  offStatus?.();
  offMessage = null;
  offStatus = null;
}
