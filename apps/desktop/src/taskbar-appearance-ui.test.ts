import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { zh, en } from "./i18n";
const source = readFileSync(new URL("./TaskbarAppearance.svelte", import.meta.url), "utf8");
describe("taskbar recovery controls", () => {
  it("allows re-enable while a previous restore is pending", () => {
    expect(source).toContain("disabled={!canApply || (busy && appearance.enabled) || applied}");
    expect(source).not.toContain("disabled={!canApply || busy || applied}");
  });
  it("keeps cancellation available while connecting or recovering", () => {
    expect(source).toContain('status?.state === "recovering"');
    expect(source).toContain("disabled={!appearance.enabled && !applied && !busy}");
    expect(source).toContain("onChange({ enabled: false })");
  });
  it("reports recovery honestly and puts raw error codes in optional details", () => {
    expect(zh.beautyRecovering).toContain("自动恢复");
    expect(en.beautyRecovering).toContain("automatically");
    expect(source).toContain('current.state === "recovering"');
    expect(source).toContain('<details class="beauty-diagnostics">');
    expect(source).toContain("t.beautyDiagnostics");
    expect(source).toContain('current.errorCode === "0x80070666"');
  });
});
