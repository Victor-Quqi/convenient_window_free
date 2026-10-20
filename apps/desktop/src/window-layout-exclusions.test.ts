// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import {
  filterLayoutWindows,
  isLayoutWindowExcluded,
  loadLayoutExclusions,
  normalizeLayoutExclusions,
  saveLayoutExclusions,
  WINDOW_LAYOUT_EXCLUSIONS_KEY
} from "./window-layout-exclusions";

const obsidian = { title: "笔记库", processName: "Obsidian.exe", className: "Chrome_WidgetWin_1" };
const editor = { title: "Editor", processName: "Code.exe", className: "Chrome_WidgetWin_1" };
const terminal = { title: "Terminal", processName: "WindowsTerminal.exe", className: "CASCADIA_HOSTING_WINDOW_CLASS" };

describe("window layout exclusions", () => {
  beforeEach(() => localStorage.clear());

  it("matches the process name, window title and class name case-insensitively", () => {
    const exclusions = normalizeLayoutExclusions(["obsidian.exe", "deep work", "sunawtframe"]);
    expect(isLayoutWindowExcluded(obsidian, exclusions)).toBe(true);
    expect(isLayoutWindowExcluded({ title: "Deep Work", processName: "other.exe", className: "Other" }, exclusions)).toBe(true);
    expect(isLayoutWindowExcluded({ title: "Browser", processName: "msedge.exe", className: "SunaWtFrame" }, exclusions)).toBe(true);
    expect(isLayoutWindowExcluded(editor, exclusions)).toBe(false);
    expect(isLayoutWindowExcluded(terminal, exclusions)).toBe(false);
  });

  it("uses partial containment like the helper's is_paused_window", () => {
    const exclusions = normalizeLayoutExclusions(["obsid"]);
    expect(isLayoutWindowExcluded(obsidian, exclusions)).toBe(true);
    expect(isLayoutWindowExcluded({ title: "2026 笔记", processName: "viewer.exe", className: "Viewer" }, exclusions)).toBe(false);
    expect(isLayoutWindowExcluded({ title: "笔记", processName: "obsidian.EXE", className: "" }, exclusions)).toBe(true);
  });

  it("never excludes when the list or the window identity is empty", () => {
    expect(isLayoutWindowExcluded(obsidian, [])).toBe(false);
    expect(isLayoutWindowExcluded(obsidian, normalizeLayoutExclusions(["", "   ", 42, null]))).toBe(false);
    expect(isLayoutWindowExcluded({}, normalizeLayoutExclusions(["obsidian"]))).toBe(false);
    expect(isLayoutWindowExcluded(null, normalizeLayoutExclusions(["obsidian"]))).toBe(false);
    expect(isLayoutWindowExcluded(undefined, normalizeLayoutExclusions(["obsidian"]))).toBe(false);
  });

  it("trims entries, drops blanks and non-strings, dedupes case-insensitively and keeps the first casing", () => {
    expect(normalizeLayoutExclusions(["  Obsidian.exe ", "", "   ", "obsidian.EXE", 42, null, {}, "Code.exe"])).toEqual(["Obsidian.exe", "Code.exe"]);
    expect(normalizeLayoutExclusions("not-an-array")).toEqual([]);
    expect(normalizeLayoutExclusions(null)).toEqual([]);
    expect(normalizeLayoutExclusions(undefined)).toEqual([]);
    expect(normalizeLayoutExclusions({ 0: "a" })).toEqual([]);
  });

  it("caps the list at 64 entries", () => {
    const many = Array.from({ length: 70 }, (_, index) => `app-${index}.exe`);
    const normalized = normalizeLayoutExclusions(many);
    expect(normalized).toHaveLength(64);
    expect(normalized[0]).toBe("app-0.exe");
    expect(normalized[63]).toBe("app-63.exe");
  });

  it("filters windows without mutating the input and preserves order", () => {
    const windows = [obsidian, editor, terminal];
    const filtered = filterLayoutWindows(windows, ["obsidian.exe", "terminal"]);
    expect(filtered).toEqual([editor]);
    expect(windows).toEqual([obsidian, editor, terminal]);

    const untouched = filterLayoutWindows(windows, []);
    expect(untouched).toEqual(windows);
    expect(untouched).not.toBe(windows);
  });

  it("round-trips the list through host storage", () => {
    saveLayoutExclusions([" Obsidian.exe ", "obsidian.exe", "msedge.exe"]);
    expect(JSON.parse(localStorage.getItem(WINDOW_LAYOUT_EXCLUSIONS_KEY)!)).toEqual(["Obsidian.exe", "msedge.exe"]);
    expect(loadLayoutExclusions()).toEqual(["Obsidian.exe", "msedge.exe"]);

    saveLayoutExclusions(["  ", "", "Code.exe"]);
    expect(loadLayoutExclusions()).toEqual(["Code.exe"]);
  });

  it("falls back to an empty list when the stored value is missing or malformed", () => {
    expect(loadLayoutExclusions()).toEqual([]);

    localStorage.setItem(WINDOW_LAYOUT_EXCLUSIONS_KEY, "{not json");
    expect(loadLayoutExclusions()).toEqual([]);

    localStorage.setItem(WINDOW_LAYOUT_EXCLUSIONS_KEY, JSON.stringify({ value: "obsidian" }));
    expect(loadLayoutExclusions()).toEqual([]);

    localStorage.setItem(WINDOW_LAYOUT_EXCLUSIONS_KEY, JSON.stringify(["Code.exe", 7, null]));
    expect(loadLayoutExclusions()).toEqual(["Code.exe"]);
  });
});
