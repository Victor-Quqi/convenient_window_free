import { loadHostValue, saveHostValue } from "./host-storage";
import { normalizeLayoutShortcut } from "./layout-hotkey-utils";

export interface WindowLayoutRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface WindowLayoutPlacement {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** 贴边收纳（edge-hide）状态，由 helper 在记录布局时附带。 */
export interface WindowLayoutEdgeHidden {
  edge: "left" | "top" | "right" | "bottom";
  restoreRect: WindowLayoutRect;
  hiddenRect: WindowLayoutRect;
}

export interface WindowLayoutWindow {
  title: string;
  processName: string;
  className: string;
  occurrence?: number;
  rect: WindowLayoutRect;
  placementRect?: WindowLayoutPlacement;
  minimized: boolean;
  maximized: boolean;
  topmost: boolean;
  monitorId?: string;
  /** 抓取时的 z-order，0 表示最顶层；旧快照没有该字段时视为未记录。 */
  zIndex?: number;
  /** 记录时窗口正被贴边收纳；恢复后 helper 会重新收纳到同一边。 */
  edgeHidden?: WindowLayoutEdgeHidden;
}

export interface WindowLayoutSnapshot {
  id: string;
  name: string;
  capturedAt: number;
  /** 布局全局快捷键，形如 "Ctrl+Alt+1"；未设置时不写入。 */
  shortcut?: string;
  windows: WindowLayoutWindow[];
}

export interface WindowLayoutCapture {
  windows: WindowLayoutWindow[];
  capturedAt?: number;
}

export interface WindowLayoutRestoreResult {
  restored: number;
  skipped: number;
  errors: string[];
}

const WINDOW_LAYOUTS_KEY = "convenient-window-layouts";
const MAX_LAYOUTS = 24;
const MAX_WINDOWS_PER_LAYOUT = 64;

export function loadWindowLayouts(): WindowLayoutSnapshot[] {
  const stored = loadHostValue<unknown>(WINDOW_LAYOUTS_KEY);
  if (!Array.isArray(stored)) return [];
  return stored
    .map((item, index) => normalizeSnapshot(item, index))
    .filter((item): item is WindowLayoutSnapshot => item !== null)
    .slice(0, MAX_LAYOUTS);
}

export function saveWindowLayouts(value: WindowLayoutSnapshot[]): void {
  const normalized = value
    .map((item, index) => normalizeSnapshot(item, index))
    .filter((item): item is WindowLayoutSnapshot => item !== null)
    .slice(0, MAX_LAYOUTS);
  saveHostValue(WINDOW_LAYOUTS_KEY, normalized);
}

export function createWindowLayoutId(): string {
  try { return crypto.randomUUID(); } catch { return `layout-${Date.now()}-${Math.random().toString(16).slice(2)}`; }
}

function normalizeSnapshot(value: unknown, index: number): WindowLayoutSnapshot | null {
  if (!value || typeof value !== "object") return null;
  const source = value as Record<string, unknown>;
  const windows = Array.isArray(source.windows)
    ? source.windows.map(normalizeWindow).filter((item): item is WindowLayoutWindow => item !== null).slice(0, MAX_WINDOWS_PER_LAYOUT)
    : [];
  const shortcut = normalizeLayoutShortcut(source.shortcut);
  return {
    id: typeof source.id === "string" && source.id.trim() ? source.id : `layout-${index + 1}`,
    name: typeof source.name === "string" && source.name.trim() ? source.name.trim().slice(0, 80) : `Layout ${index + 1}`,
    capturedAt: typeof source.capturedAt === "number" && Number.isFinite(source.capturedAt) ? source.capturedAt : Date.now(),
    ...(shortcut ? { shortcut } : {}),
    windows
  };
}

function normalizeWindow(value: unknown): WindowLayoutWindow | null {
  if (!value || typeof value !== "object") return null;
  const source = value as Record<string, unknown>;
  const rect = source.rect;
  if (!rect || typeof rect !== "object") return null;
  const rawRect = rect as Record<string, unknown>;
  const values = [rawRect.left, rawRect.top, rawRect.right, rawRect.bottom];
  if (!values.every((item) => typeof item === "number" && Number.isFinite(item))) return null;
  const edgeHidden = normalizeEdgeHidden(source.edgeHidden);
  return {
    title: stringValue(source.title),
    processName: stringValue(source.processName),
    className: stringValue(source.className),
    ...(typeof source.occurrence === "number" && Number.isFinite(source.occurrence) ? { occurrence: Math.max(0, Math.floor(source.occurrence)) } : {}),
    rect: { left: rawRect.left as number, top: rawRect.top as number, right: rawRect.right as number, bottom: rawRect.bottom as number },
    ...(normalizeRect(source.placementRect) ? { placementRect: normalizeRect(source.placementRect)! } : {}),
    minimized: source.minimized === true,
    maximized: source.maximized === true,
    topmost: source.topmost === true,
    ...(typeof source.monitorId === "string" && source.monitorId.trim() ? { monitorId: source.monitorId.trim() } : {}),
    ...(typeof source.zIndex === "number" && Number.isInteger(source.zIndex) && source.zIndex >= 0 ? { zIndex: source.zIndex } : {}),
    ...(edgeHidden ? { edgeHidden } : {})
  };
}

const EDGE_HIDDEN_EDGES = ["left", "top", "right", "bottom"] as const;

/** 只保留 helper 协议里的合法收纳状态；非法值（未知边、坏矩形）整段丢弃。 */
function normalizeEdgeHidden(value: unknown): WindowLayoutEdgeHidden | null {
  if (!value || typeof value !== "object") return null;
  const source = value as Record<string, unknown>;
  const edge = source.edge;
  if (typeof edge !== "string" || !(EDGE_HIDDEN_EDGES as readonly string[]).includes(edge)) return null;
  const restoreRect = normalizeRect(source.restoreRect);
  const hiddenRect = normalizeRect(source.hiddenRect);
  if (!restoreRect || !hiddenRect) return null;
  return { edge: edge as WindowLayoutEdgeHidden["edge"], restoreRect, hiddenRect };
}

function normalizeRect(value: unknown): WindowLayoutPlacement | null {
  if (!value || typeof value !== "object") return null;
  const source = value as Record<string, unknown>;
  const values = [source.left, source.top, source.right, source.bottom];
  if (!values.every((item) => typeof item === "number" && Number.isFinite(item))) return null;
  return { left: source.left as number, top: source.top as number, right: source.right as number, bottom: source.bottom as number };
}

function stringValue(value: unknown): string { return typeof value === "string" ? value.trim().slice(0, 120) : ""; }
