import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultSettings, loadSettings, normalizeSettings, saveSettings } from "./settings-store";
import type { AppSettings } from "./types";

const SETTINGS_KEY = "magic-corners.settings";
function browserStore(initial: unknown) {
  const values = new Map<string, string>();
  if (initial) values.set(SETTINGS_KEY, JSON.stringify(initial));
  vi.stubGlobal("window", {});
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value)
  });
  return values;
}
function legacyDisabled(): Omit<AppSettings, "topmostPin"> & { topmostPin: { enabled: boolean } } {
  const settings = structuredClone(defaultSettings);
  delete (settings.topmostPin as { defaultsVersion?: number }).defaultsVersion;
  settings.topmostPin = { enabled: false };
  settings.edgeHide.animationEnabled = false;
  settings.ocr.pinOffset = false;
  settings.mouseGestures.enabled = true;
  settings.pausedApps = ["example.exe"];
  return settings as Omit<AppSettings, "topmostPin"> & { topmostPin: { enabled: boolean } };
}

afterEach(() => vi.unstubAllGlobals());

describe("topmost pin default repair", () => {
  it("shows pins by default for a fresh profile", () => {
    browserStore(null);
    expect(loadSettings().topmostPin).toMatchObject({ enabled: true, defaultsVersion: 1 });
  });
  it("repairs a legacy hidden false once without resetting unrelated preferences", async () => {
    const original = legacyDisabled();
    browserStore(original);
    const loaded = loadSettings();
    expect(loaded.topmostPin).toMatchObject({ enabled: true, defaultsVersion: 1 });
    expect(loaded.edgeHide.animationEnabled).toBe(false);
    expect(loaded.ocr.pinOffset).toBe(false);
    expect(loaded.mouseGestures.enabled).toBe(true);
    expect(loaded.pausedApps).toEqual(["example.exe"]);
    await saveSettings(loaded);
    expect(loadSettings().topmostPin.enabled).toBe(true);
  });
  it("keeps an explicit opt-out after repair across save and reload", async () => {
    browserStore(legacyDisabled());
    const loaded = loadSettings();
    expect(loaded.topmostPin.enabled).toBe(true);
    loaded.topmostPin.enabled = false;
    await saveSettings(normalizeSettings(loaded));
    expect(loadSettings().topmostPin).toMatchObject({ enabled: false, defaultsVersion: 1 });
  });
  it("does not override explicit false during normalization or import", () => {
    expect(normalizeSettings(legacyDisabled()).topmostPin.enabled).toBe(false);
    expect(normalizeSettings({ topmostPin: { enabled: false, defaultsVersion: 1 } }).topmostPin.enabled).toBe(false);
  });
});
