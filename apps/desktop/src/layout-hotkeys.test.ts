// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HelperClient } from "./helper-client";
import type { LayoutHotkeyExecutor } from "./layout-hotkeys";
import type { HelperMessage, HelperStatus } from "./types";
import type { WindowLayoutSnapshot } from "./window-layout-store";

// 两套宿主现在共用同一份 helper 热键客户端：全局快捷键由 helper 常驻键盘钩子匹配，
// 前端只负责全量同步列表、在 shortcut.hotkey.triggered 时执行复位、在连接恢复时重同步。
// 单测用假 helper 替代 websocket，覆盖同步内容、失败回报、触发执行与监听清理。

interface SyncResult {
  accepted: number;
  rejected: Array<{ id: string; reason: string }>;
}

interface HotkeyEntry {
  id: string;
  accelerator: string;
}

type SyncHotkeys = (hotkeys: HotkeyEntry[]) => Promise<SyncResult>;

const LAYOUTS_KEY = "convenient-window-layouts";

function storeLayouts(layouts: Array<{ id: string; shortcut?: string }>): void {
  localStorage.setItem(LAYOUTS_KEY, JSON.stringify(layouts.map((layout, index) => ({
    id: layout.id,
    name: layout.id,
    capturedAt: index,
    ...(layout.shortcut ? { shortcut: layout.shortcut } : {}),
    windows: [{ title: "Editor", processName: "code.exe", className: "Code", rect: { left: 0, top: 0, right: 800, bottom: 600 }, minimized: false, maximized: false, topmost: false }]
  }))));
}

function createHelper(sync?: SyncHotkeys) {
  const messageHandlers = new Set<(message: HelperMessage) => void>();
  const statusHandlers = new Set<(status: HelperStatus) => void>();
  const syncShortcutHotkeys = vi.fn<SyncHotkeys>(
    sync ?? (async (hotkeys) => ({ accepted: hotkeys.length, rejected: [] }))
  );
  const fake = {
    syncShortcutHotkeys,
    onMessage(handler: (message: HelperMessage) => void): () => void {
      messageHandlers.add(handler);
      return () => { messageHandlers.delete(handler); };
    },
    onStatus(handler: (status: HelperStatus) => void): () => void {
      statusHandlers.add(handler);
      return () => { statusHandlers.delete(handler); };
    }
  };
  return {
    helper: fake as unknown as HelperClient,
    syncShortcutHotkeys,
    emit(type: string, data?: unknown): void {
      for (const handler of [...messageHandlers]) handler({ id: "server", type, time: 0, data });
    },
    emitStatus(status: HelperStatus): void {
      for (const handler of [...statusHandlers]) handler(status);
    },
    messageHandlerCount: (): number => messageHandlers.size,
    statusHandlerCount: (): number => statusHandlers.size
  };
}

/** 让串行 sync 的微任务链跑完。 */
async function flush(): Promise<void> {
  for (let index = 0; index < 8; index += 1) await Promise.resolve();
}

type HotkeysModule = typeof import("./layout-hotkeys");
let hotkeys: HotkeysModule;

beforeEach(async () => {
  vi.resetModules();
  localStorage.clear();
  hotkeys = await import("./layout-hotkeys");
});

describe("helper layout hotkeys client", () => {
  it("syncs stored layouts with Win rendered as Super and skips layouts without a shortcut", async () => {
    storeLayouts([
      { id: "layout-1", shortcut: "ctrl+alt+1" },
      { id: "layout-2" },
      { id: "layout-3", shortcut: "win+f2" }
    ]);
    const fake = createHelper();

    expect(await hotkeys.initLayoutHotkeys(fake.helper, vi.fn())).toEqual([]);
    expect(fake.syncShortcutHotkeys).toHaveBeenLastCalledWith([
      { id: "layout-1", accelerator: "Ctrl+Alt+1" },
      { id: "layout-3", accelerator: "Super+F2" }
    ]);
  });

  it("sends the full list on every sync and clears the helper table with an empty list", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }]);
    const fake = createHelper();
    await hotkeys.initLayoutHotkeys(fake.helper, vi.fn());

    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+2" }]);
    expect(await hotkeys.syncLayoutHotkeys()).toEqual([]);
    expect(fake.syncShortcutHotkeys).toHaveBeenLastCalledWith([{ id: "layout-1", accelerator: "Ctrl+Alt+2" }]);

    storeLayouts([]);
    expect(await hotkeys.syncLayoutHotkeys()).toEqual([]);
    expect(fake.syncShortcutHotkeys).toHaveBeenLastCalledWith([]);
  });

  it("executes the matching layout on shortcut.hotkey.triggered and re-reads the latest snapshot", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }]);
    const fake = createHelper();
    const executor = vi.fn<LayoutHotkeyExecutor>();
    await hotkeys.initLayoutHotkeys(fake.helper, executor);

    // 触发前布局名称变化（例如用户重命名）应被读到最新值。
    const stored = JSON.parse(localStorage.getItem(LAYOUTS_KEY)!) as WindowLayoutSnapshot[];
    stored[0].name = "Renamed";
    localStorage.setItem(LAYOUTS_KEY, JSON.stringify(stored));

    fake.emit("shortcut.hotkey.triggered", { hotkeyId: "layout-1", accelerator: "Ctrl+Alt+1" });
    await flush();
    expect(executor).toHaveBeenCalledTimes(1);
    expect(executor.mock.calls[0][0]).toMatchObject({ id: "layout-1", name: "Renamed" });
  });

  it("ignores unknown ids, malformed payloads and executor failures", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }]);
    const fake = createHelper();
    const executor = vi.fn<LayoutHotkeyExecutor>(() => { throw new Error("restore failed"); });
    await hotkeys.initLayoutHotkeys(fake.helper, executor);

    fake.emit("shortcut.hotkey.triggered", { hotkeyId: "layout-unknown" });
    fake.emit("shortcut.hotkey.triggered", { hotkeyId: 42 });
    fake.emit("shortcut.hotkey.triggered", null);
    fake.emit("shortcut.hotkey.triggered", { hotkeyId: "layout-1" });
    await flush();
    expect(executor).toHaveBeenCalledTimes(1);
    expect(executor.mock.calls[0][0]).toMatchObject({ id: "layout-1" });
  });

  it("returns the layout ids the helper rejected so the UI can warn", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }, { id: "layout-2", shortcut: "Ctrl+Alt+2" }]);
    const fake = createHelper(async () => ({ accepted: 1, rejected: [{ id: "layout-2", reason: "duplicate accelerator" }] }));

    expect(await hotkeys.initLayoutHotkeys(fake.helper, vi.fn())).toEqual(["layout-2"]);
    expect(await hotkeys.syncLayoutHotkeys()).toEqual(["layout-2"]);
  });

  it("reports every configured layout id when the helper request fails", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }, { id: "layout-2", shortcut: "Ctrl+Alt+2" }]);
    const fake = createHelper(async () => { throw new Error("Helper is not ready"); });

    expect(await hotkeys.initLayoutHotkeys(fake.helper, vi.fn())).toEqual(["layout-1", "layout-2"]);
  });

  it("re-syncs when the connection is restored and the handshake completes", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }]);
    const fake = createHelper();
    await hotkeys.initLayoutHotkeys(fake.helper, vi.fn());
    const afterInit = fake.syncShortcutHotkeys.mock.calls.length;

    fake.emitStatus("disconnected");
    await flush();
    expect(fake.syncShortcutHotkeys).toHaveBeenCalledTimes(afterInit);

    fake.emitStatus("connected");
    await flush();
    expect(fake.syncShortcutHotkeys).toHaveBeenCalledTimes(afterInit + 1);

    // helper.ready 之后协议才可用，重连后必须在握手完成时把热键重新登记。
    fake.emit("helper.ready", { protocolVersion: 7, schemaVersion: 9 });
    await flush();
    expect(fake.syncShortcutHotkeys).toHaveBeenCalledTimes(afterInit + 2);
  });

  it("unsubscribes and syncs an empty list on dispose", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }]);
    const fake = createHelper();
    const executor = vi.fn<LayoutHotkeyExecutor>();
    await hotkeys.initLayoutHotkeys(fake.helper, executor);

    hotkeys.disposeLayoutHotkeys();
    expect(fake.syncShortcutHotkeys).toHaveBeenLastCalledWith([]);
    expect(fake.messageHandlerCount()).toBe(0);
    expect(fake.statusHandlerCount()).toBe(0);

    fake.emit("shortcut.hotkey.triggered", { hotkeyId: "layout-1" });
    await flush();
    expect(executor).not.toHaveBeenCalled();
    expect(await hotkeys.syncLayoutHotkeys()).toEqual([]);
  });

  it("serializes syncs so a slow acknowledgement cannot interleave with the next list", async () => {
    storeLayouts([{ id: "layout-1", shortcut: "Ctrl+Alt+1" }]);
    let acknowledge: (result: SyncResult) => void = () => undefined;
    const fake = createHelper();
    fake.syncShortcutHotkeys.mockImplementationOnce(() => new Promise<SyncResult>((resolve) => { acknowledge = resolve; }));

    const initialised = hotkeys.initLayoutHotkeys(fake.helper, vi.fn());
    await flush();
    expect(fake.syncShortcutHotkeys).toHaveBeenCalledTimes(1);

    // 第一次同步还没收到回执时，第二次同步必须排队，不能并发改写 helper 的热键表。
    const next = hotkeys.syncLayoutHotkeys();
    await flush();
    expect(fake.syncShortcutHotkeys).toHaveBeenCalledTimes(1);

    acknowledge({ accepted: 1, rejected: [] });
    await initialised;
    await flush();
    expect(fake.syncShortcutHotkeys).toHaveBeenCalledTimes(2);
    expect(await next).toEqual([]);
  });
});
