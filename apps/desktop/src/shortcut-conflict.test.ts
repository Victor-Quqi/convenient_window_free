import { readFileSync } from "node:fs";
import nodePath from "node:path";
import { render } from "svelte/server";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import ShortcutRecorder from "./ShortcutRecorder.svelte";
import { formatShortcut, heldModifiers, isModifierKey, resolveKeyName } from "./shortcut-keys";

const component = readFileSync(nodePath.join(import.meta.dirname, "ShortcutRecorder.svelte"), "utf8");
const app = readFileSync(nodePath.join(import.meta.dirname, "App.svelte"), "utf8");

// 剥掉 TypeScript 类型标注：用项目自带的 typescript 编译器转译，而不是手写正则。
function stripTypes(code: string): string {
  return ts.transpileModule(code, {
    compilerOptions: { target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext }
  }).outputText;
}

// 测试直接调用 shortcut-keys 的真实实现，不做源码字符串提取。
type KeyState = Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>>;

function event(key: string, code: string, modifiers: KeyState = {}) {
  return {
    key,
    code,
    ctrlKey: Boolean(modifiers.ctrlKey),
    altKey: Boolean(modifiers.altKey),
    shiftKey: Boolean(modifiers.shiftKey),
    metaKey: Boolean(modifiers.metaKey)
  } as KeyboardEvent;
}

function html(props: { value: string; label: string; english: boolean }): string {
  const out = render(ShortcutRecorder, { props: { ...props, onChange: () => {} } });
  return `${out.head}${out.body}`;
}

describe("shortcut key resolution", () => {
  it("records a bare key without any modifier", () => {
    for (const [key, code, expected] of [
      ["a", "KeyA", "A"],
      ["5", "Digit5", "5"],
      ["F5", "F5", "F5"],
      ["ArrowLeft", "ArrowLeft", "Left"],
      [" ", "Space", "Space"],
      ["Enter", "Enter", "Enter"],
      ["Tab", "Tab", "Tab"]
    ] as const) {
      expect(resolveKeyName(event(key, code)), `${key} should resolve on its own`).toBe(expected);
    }
  });

  it("ignores keys the helper cannot execute", () => {
    // helper 的 parse_ascii_key 只接受「长度为 1 的 ASCII 字母或数字」，
    // 标点与编辑键都会在执行时报 unsupported shortcut key，所以录制阶段就不接收。
    for (const [key, code] of [
      ["/", "Slash"],
      [";", "Semicolon"],
      ["[", "BracketLeft"],
      ["Backspace", "Backspace"],
      ["Delete", "Delete"],
      ["Home", "Home"],
      ["PageUp", "PageUp"]
    ] as const) {
      expect(resolveKeyName(event(key, code)), `${key} must not be recordable`).toBeNull();
    }
  });

  it("treats modifier keys themselves as non-recordable", () => {
    for (const key of ["Control", "Alt", "Shift", "Meta"] as const) {
      expect(isModifierKey(event(key, key))).toBe(true);
      expect(resolveKeyName(event(key, key))).toBeNull();
    }
  });

  it("takes the physical key from the layout-independent code", () => {
    // AZERTY 等布局下 e.key 会变成布局字符，必须按 e.code 记录物理键位。
    expect(resolveKeyName(event("q", "KeyA"))).toBe("A");
    expect(resolveKeyName(event("&", "Digit1"))).toBe("1");
  });

  it("orders modifiers as Ctrl, Alt, Shift, Win", () => {
    const state = { shiftKey: true, metaKey: true, altKey: true, ctrlKey: true };
    expect(heldModifiers(event("a", "KeyA", state))).toEqual(["ctrl", "alt", "shift", "win"]);
    expect(formatShortcut(heldModifiers(event("a", "KeyA", state)), "A")).toBe("Ctrl+Alt+Shift+Win+A");
    expect(formatShortcut([], "F5")).toBe("F5");
    expect(formatShortcut(["ctrl"], "Enter")).toBe("Ctrl+Enter");
  });
});

describe("shortcut recorder component", () => {
  it("renders the record hint, key caps and clear control", () => {
    const empty = html({ value: "", label: "录制快捷键", english: false });
    expect(empty).toContain("点击录制快捷键");
    expect(empty).not.toContain("清除快捷键");
    expect(html({ value: "", label: "Record shortcut", english: true })).toContain("Record shortcut");

    const filled = html({ value: "Ctrl+Alt+A", label: "录制快捷键", english: false });
    expect(filled).toMatch(/<kbd[^>]*>Ctrl<\/kbd>/);
    expect(filled).toMatch(/<kbd[^>]*>Alt<\/kbd>/);
    expect(filled).toMatch(/<kbd[^>]*>A<\/kbd>/);
    expect(filled).toContain("清除快捷键");
  });

  it("isolates page shortcuts and keeps recording state in the component", () => {
    expect(component).toContain("preventDefault");
    expect(component).toContain("stopPropagation");
    expect(component).toContain("if (!recording) return");
    expect(component).toContain("onChange(next)");
  });

  it("cancels with Escape and supports clearing", () => {
    expect(component).toContain('if (e.key === "Escape")');
    expect(component).toContain('onChange("")');
  });
});

describe("shortcut conflict rejection", () => {
  const target: { kind: string; value?: string } = { kind: "none" };
  const settings = {
    hotzones: [{ id: "left", actions: [{ trigger: "hover", action: { kind: "shortcut", value: "Ctrl+Alt+A" } }] }],
    monitorProfiles: []
  };

  // 从 App.svelte 提取真实的 setActionShortcut 执行，测的是产品代码而不是复制品。
  function load() {
    const declaration = /function\s+setActionShortcut\b/.exec(app);
    if (!declaration || declaration.index === undefined) throw new Error("setActionShortcut not found");
    let depth = 0;
    let end = declaration.index;
    for (let i = declaration.index; i < app.length; i += 1) {
      if (app[i] === "{") depth += 1;
      else if (app[i] === "}") {
        depth -= 1;
        if (depth === 0) { end = i + 1; break; }
      }
    }
    const handler = stripTypes(app.slice(declaration.index, end));
    return new Function(
      "settings",
      "target",
      "persist",
      "ensureHotzoneActionTarget",
      `let shortcutError = "";
       ${handler}
       return { setActionShortcut, readError: () => shortcutError };`
    )(settings, target, () => {}, () => ({ slot: {}, action: target })) as {
      setActionShortcut: (value: string) => void;
      readError: () => string;
    };
  }

  it("keeps the previous value and reports the clash", () => {
    const instance = load();
    target.kind = "none";
    delete target.value;
    instance.setActionShortcut("Ctrl+Alt+A");
    expect(instance.readError()).toContain("Ctrl+Alt+A");
    expect(instance.readError()).toContain("已被其他动作使用");
    expect(target.kind).toBe("none");
    expect(target.value).toBeUndefined();
  });

  it("accepts a fresh combination", () => {
    const instance = load();
    target.kind = "none";
    delete target.value;
    instance.setActionShortcut("Ctrl+Shift+K");
    expect(instance.readError()).toBe("");
    expect(target.kind).toBe("shortcut");
    expect(target.value).toBe("Ctrl+Shift+K");
  });

  it("turns an empty recording into a cleared action", () => {
    const instance = load();
    target.kind = "shortcut";
    target.value = "Ctrl+Alt+A";
    instance.setActionShortcut("");
    expect(target.kind).toBe("none");
    expect(target.value).toBeUndefined();
    expect(instance.readError()).toBe("");
  });

  it("accepts a bare key when it is not already taken", () => {
    const instance = load();
    target.kind = "none";
    delete target.value;
    instance.setActionShortcut("F5");
    expect(instance.readError()).toBe("");
    expect(target.value).toBe("F5");
  });
});
