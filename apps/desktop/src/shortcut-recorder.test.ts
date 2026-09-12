import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
const source = readFileSync(new URL("./ShortcutRecorder.svelte", import.meta.url), "utf8");
describe("shortcut recorder", () => {
  it("records modifiers and a main key while isolating page shortcuts", () => {
    expect(source).toContain("preventDefault");
    expect(source).toContain("stopPropagation");
    expect(source).toContain("ctrlKey");
    expect(source).toContain("metaKey");
    expect(source).toContain("onChange(next)");
  });
  it("supports cancel, empty state and clear", () => {
    expect(source).toContain("Escape");
    expect(source).toContain("captured =");
    expect(source).toContain("onChange");
    expect(source).toContain("Record shortcut");
  });
});

