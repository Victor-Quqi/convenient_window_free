import { gestureLabels } from "./gesture-names";
import { translator } from "./i18n";
import type { Language } from "./i18n";
import type { AppSettings, HelperMessage, HelperPlatformInfo, HelperStatus } from "./types";
import { settingsForHelper } from "./runtime-settings";
import type { WindowLayoutCapture, WindowLayoutRestoreResult, WindowLayoutWindow } from "./window-layout-store";

type MessageHandler = (message: HelperMessage) => void;
type StatusHandler = (status: HelperStatus) => void;
type PendingRequest = { resolve: (message: HelperMessage) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> };

export const SUPPORTED_HELPER_PROTOCOL = 7;
export const SUPPORTED_HELPER_SCHEMA = 9;

export function isSupportedHelperProtocol(value: unknown): boolean {
  return value === SUPPORTED_HELPER_PROTOCOL;
}

export function isSupportedHelperSchema(value: unknown): boolean {
  return value === SUPPORTED_HELPER_SCHEMA;
}

export class HelperClient {
  private socket: WebSocket | null = null;
  private latestSettings: AppSettings | null = null;
  private latestConfigRevision = 0;
  private latestConfigRequestId = "";
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private shouldReconnect = false;
  private stopRequested = false;
  private protocolReady = false;
  private recordingSupported = false;
  private latestPlatform: HelperPlatformInfo | null = null;
  private generation = 0;
  private readonly messageHandlers = new Set<MessageHandler>();
  private readonly statusHandlers = new Set<StatusHandler>();
  private readonly pendingRequests = new Map<string, PendingRequest>();

  constructor(
    private readonly url = "ws://127.0.0.1:56873",
    private readonly tokenProvider: () => string | null = () => null,
    private readonly languageProvider: () => Language = () => "en-US"
  ) {}

  connect(): void {
    this.openSocket(true);
  }

  private openSocket(shouldReconnect: boolean): void {
    if (shouldReconnect) this.stopRequested = false;
    this.shouldReconnect = shouldReconnect;
    this.protocolReady = false;
    this.recordingSupported = false;
    this.closeSocket();
    this.clearReconnectTimer();

    const gen = ++this.generation;
    this.emitStatus("connecting");
    const token = this.tokenProvider()?.trim();
    const socket = token ? new WebSocket(this.url, token) : new WebSocket(this.url);
    this.socket = socket;

    socket.addEventListener("open", () => {
      if (gen !== this.generation) return;
      this.emitStatus("connected");
      if (this.stopRequested) {
        this.stopRequested = false;
        this.send("helper.stop", {});
        return;
      }
    });
    socket.addEventListener("close", () => {
      if (gen !== this.generation) return;
      this.socket = null;
      this.recordingSupported = false;
      this.stopRequested = false;
      this.emitStatus("disconnected");
      this.rejectPending("Helper disconnected");
      if (this.shouldReconnect) this.scheduleReconnect();
    });
    socket.addEventListener("error", () => {
      if (gen !== this.generation) return;
      this.generation++;
      this.socket = null;
      this.recordingSupported = false;
      this.stopRequested = false;
      this.emitStatus("disconnected");
      this.rejectPending("Helper connection failed");
      if (this.shouldReconnect) this.scheduleReconnect();
      try {
        socket.close();
      } catch {
        // The reconnect timer above is the fallback for incomplete handshakes.
      }
    });
    socket.addEventListener("message", (event) => {
      if (gen !== this.generation) return;
      try {
        const message = JSON.parse(String(event.data)) as HelperMessage;
        this.resolvePending(message);
        if (message.type === "helper.ready") {
          const data = message.data as { protocolVersion?: unknown; schemaVersion?: unknown; platform?: unknown } | null;
          const protocolVersion = data?.protocolVersion;
          this.latestPlatform = isPlatformInfo(data?.platform) ? data.platform : null;
          this.protocolReady = isSupportedHelperProtocol(protocolVersion)
            && isSupportedHelperSchema(data?.schemaVersion);
          this.recordingSupported = this.protocolReady && (message.data as { shortcutRecording?: boolean })?.shortcutRecording === true;
          if (this.protocolReady) this.flushLatestConfig();
        }
        this.messageHandlers.forEach((handler) => handler(message));
      } catch (error) {
        console.warn("Invalid helper message", error);
      }
    });
  }

  disconnect(): void {
    this.shouldReconnect = false;
    this.stopRequested = false;
    this.clearReconnectTimer();
    this.closeSocket();
    this.latestPlatform = null;
    this.rejectPending("Helper disconnected");
    this.emitStatus("disconnected");
  }

  get platformInfo(): HelperPlatformInfo | null {
    return this.latestPlatform;
  }

  get supportsShortcutRecording(): boolean { return this.recordingSupported; }
  beginShortcutRecording(sessionId: string): boolean {
    return this.recordingSupported && this.send("shortcut.record.begin", { sessionId });
  }
  cancelShortcutRecording(sessionId: string): void {
    this.send("shortcut.record.cancel", { sessionId });
  }

  private closeSocket(): void {
    this.recordingSupported = false;
    if (this.socket) {
      this.generation++;
      this.socket.close();
      this.socket = null;
    }
  }

  sendConfig(settings: AppSettings): boolean {
    this.latestSettings = settingsForHelper(settings);
    this.latestConfigRevision += 1;
    this.latestConfigRequestId = crypto.randomUUID();
    return this.flushLatestConfig();
  }

  isLatestConfigRevision(revision: unknown): boolean {
    return typeof revision === "number" && revision === this.latestConfigRevision;
  }

  ping(): boolean {
    return this.send("helper.ping", {});
  }

  requestUsage(): void {
    this.send("usage.get", {});
  }

  captureWindowLayout(): Promise<WindowLayoutCapture> {
    return this.request("window.layout.capture", {}).then((message) => {
      const data = message.data && typeof message.data === "object" ? message.data as Record<string, unknown> : {};
      return { windows: Array.isArray(data.windows) ? data.windows as WindowLayoutWindow[] : [], capturedAt: typeof data.capturedAt === "number" ? data.capturedAt : undefined };
    });
  }

  restoreWindowLayout(windows: WindowLayoutWindow[]): Promise<WindowLayoutRestoreResult> {
    return this.request("window.layout.restore", { windows }).then((message) => {
      const data = message.data && typeof message.data === "object" ? message.data as Record<string, unknown> : {};
      return { restored: typeof data.restored === "number" ? data.restored : 0, skipped: typeof data.skipped === "number" ? data.skipped : 0, errors: Array.isArray(data.errors) ? data.errors.map((item) => item && typeof item === "object" && typeof (item as Record<string, unknown>).details === "string" ? String((item as Record<string, unknown>).details) : String(item)) : [] };
    });
  }

  /** 把布局快捷键列表全量同步给 helper；helper 常驻键盘钩子负责匹配与触发。 */
  syncShortcutHotkeys(hotkeys: Array<{ id: string; accelerator: string }>): Promise<{ accepted: number; rejected: Array<{ id: string; reason: string }> }> {
    return this.request("shortcut.hotkeys.sync", { hotkeys }).then((message) => {
      const data = message.data && typeof message.data === "object" ? message.data as Record<string, unknown> : {};
      return {
        accepted: typeof data.accepted === "number" ? data.accepted : 0,
        rejected: Array.isArray(data.rejected) ? data.rejected.map((item) => {
          const rejected = item && typeof item === "object" ? item as Record<string, unknown> : {};
          return { id: typeof rejected.id === "string" ? rejected.id : "", reason: typeof rejected.reason === "string" ? rejected.reason : "" };
        }).filter((item) => item.id.length > 0) : []
      };
    });
  }

  stop(): boolean {
    this.shouldReconnect = false;
    this.clearReconnectTimer();
    if (this.send("helper.stop", {})) return true;
    this.stopRequested = true;
    if (!this.socket || this.socket.readyState !== WebSocket.CONNECTING) this.openSocket(false);
    return true;
  }

  onMessage(handler: MessageHandler): () => void {
    this.messageHandlers.add(handler);
    return () => this.messageHandlers.delete(handler);
  }

  onStatus(handler: StatusHandler): () => void {
    this.statusHandlers.add(handler);
    return () => this.statusHandlers.delete(handler);
  }

  private send(type: string, data: unknown): boolean {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      return false;
    }

    return this.sendMessage({
      id: crypto.randomUUID(),
      type,
      time: Date.now(),
      data
    });
  }

  private request(type: string, data: unknown): Promise<HelperMessage> {
    return new Promise((resolve, reject) => {
      if (!this.socket || this.socket.readyState !== WebSocket.OPEN || !this.protocolReady) { reject(new Error("Helper is not ready")); return; }
      const id = crypto.randomUUID();
      const timer = setTimeout(() => { this.pendingRequests.delete(id); reject(new Error("Helper request timed out")); }, 8_000);
      this.pendingRequests.set(id, { resolve, reject, timer });
      if (!this.sendMessage({ id, type, time: Date.now(), data })) { clearTimeout(timer); this.pendingRequests.delete(id); reject(new Error("Helper request was not sent")); }
    });
  }

  private resolvePending(message: HelperMessage): void {
    if (message.type !== "window.layout.captured" && message.type !== "window.layout.restored" && message.type !== "shortcut.hotkeys.synced" && message.type !== "runtime.error") return;
    const data = message.data && typeof message.data === "object" ? message.data as Record<string, unknown> : {};
    const requestId = typeof data.requestId === "string" ? data.requestId : message.id;
    const pending = this.pendingRequests.get(requestId);
    if (!pending) return;
    clearTimeout(pending.timer); this.pendingRequests.delete(requestId);
    if (message.type === "runtime.error") pending.reject(new Error(typeof data.details === "string" && data.details.trim() ? data.details : "Helper request failed"));
    else pending.resolve(message);
  }

  private rejectPending(message: string): void {
    for (const [id, pending] of this.pendingRequests) { clearTimeout(pending.timer); pending.reject(new Error(message)); this.pendingRequests.delete(id); }
  }

  private sendMessage(message: HelperMessage): boolean {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return false;
    try {
      this.socket.send(JSON.stringify(message));
      return true;
    } catch {
      return false;
    }
  }

  private emitStatus(status: HelperStatus): void {
    this.statusHandlers.forEach((handler) => handler(status));
  }

  private flushLatestConfig(): boolean {
    if (!this.latestSettings || !this.protocolReady) return false;
    return this.sendMessage({
      id: this.latestConfigRequestId,
      type: "config.update",
      time: Date.now(),
      data: {
        protocolVersion: SUPPORTED_HELPER_PROTOCOL,
        gestureLabels: gestureLabels(this.latestSettings.mouseGestures?.gestures ?? [], translator(this.languageProvider())),
        revision: this.latestConfigRevision,
        config: this.latestSettings
      }
    });
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) {
      return;
    }

    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, 1500);
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }
}

function isPlatformInfo(value: unknown): value is HelperPlatformInfo {
  if (!value || typeof value !== "object") return false;
  const data = value as Record<string, unknown>;
  if (!data.capabilities || typeof data.capabilities !== "object") return false;
  const capabilities = data.capabilities as Record<string, unknown>;
  const names = [
    "globalInput", "windowControl", "windowTopmost", "screenCapture",
    "ocr", "audio", "systemActions", "edgeHide"
  ];
  return typeof data.system === "string"
    && typeof data.architecture === "string"
    && names.every((name) => typeof capabilities[name] === "boolean");
}
