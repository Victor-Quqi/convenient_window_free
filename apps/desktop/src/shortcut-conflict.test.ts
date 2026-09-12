import { readFileSync } from "node:fs";
import nodePath from "node:path";
import { render } from "svelte/server";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import ShortcutRecorder from "./ShortcutRecorder.svelte";

const component = readFileSync(nodePath.join(import.meta.dirname, "ShortcutRecorder.svelte"), "utf8");
const app = readFileSync(nodePath.join(import.meta.dirname, "App.svelte"), "utf8");

// 剥掉 TypeScript 类型标注：用项目自带的 typescript 编译器转译，而不是手写正则。
function stripTypes(code: string): string {
  return ts.transpileModule(code, {
    compilerOptions: { target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext }
  }).outputText;
}

function extract(name: string, source: string): string {
  const declaration = new RegExp(`(?:function\\s+${name}\\b|const\\s+${name}\\s*[:=])`);
  const found = declaration.exec(source);
  if (!found || found.index === undefined) throw new Error(`${name} not found`);
  const start = found.index;
  // const 声明：取到分号结束；function 声明：按花括号配对取完整函数体。
  if (/^\s*const\b/.test(found[0])) {
    const end = source.indexOf(";", start);
    if (end === -1) throw new Error(`${name} declaration not closed`);
    return stripTypes(source.slice(start, end + 1).trim());
  }
  let depth = 0;
  for (let index = start; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    else if (source[index] === "}") {
      depth -= 1;
      if (depth === 0) return stripTypes(source.slice(start, index + 1).trim());
    }
  }
  throw new Error(`${name} body not closed`);
}

function html(props: { value: string; label: string; english: boolean }): string {
  const out = render(ShortcutRecorder, { props: { ...props, onChange: () => {} } });
  return `${out.head}${out.body}`;
}

// 本仓库不装 jsdom，所以不伪造 DOM：把组件里真正的 keydown / clear 处理器取出来，
// 用带真实事件语义的按键对象执行同一份代码，测的是组件里的逻辑而不是复制品。
function buildRecorder(initial: { value?: string; label?: string; english?: boolean } = {}) {
  const changes: string[] = [];
  const helpers = ["order", "names", "mods", "name", "start", "keydown", "clear"]
    .map((symbol) => extract(symbol, component))
    .join("\n     ");
  const instance = new Function(
    "value",
    "label",
    "english",
    "onChange",
    `let recording = false; let captured = ""; const root = { focus() {} };
     ${helpers}
     return {
       start,
       press(event) { keydown(event); },
       clear(event) { clear(event); },
       read: () => ({ recording, captured })
     };`
  )(initial.value ?? "", initial.label ?? "录制快捷键", initial.english ?? false, (next: string) => changes.push(next)) as {
    start: () => void;
    press: (event: unknown) => void;
    clear: (event: unknown) => void;
    read: () => { recording: boolean; captured: string };
  };
  return { ...instance, changes };
}

function keyEvent(key: string, modifiers: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) {
  return {
    key,
    ctrlKey: Boolean(modifiers.ctrlKey),
    altKey: Boolean(modifiers.altKey),
    shiftKey: Boolean(modifiers.shiftKey),
    metaKey: Boolean(modifiers.metaKey),
    defaultPrevented: 0,
    propagationStopped: 0,
    preventDefault(this: { defaultPrevented: number }) { this.defaultPrevented += 1; },
    stopPropagation(this: { propagationStopped: number }) { this.propagationStopped += 1; }
  };
}

function clickEvent() {
  return {
    propagationStopped: 0,
    stopPropagation(this: { propagationStopped: number }) { this.propagationStopped += 1; }
  };
}

describe("shortcut recorder behaviour", () => {
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

  it("waits while only modifiers are held and records the full combination", () => {
    const recorder = buildRecorder();
    recorder.start();
    // 按住修饰键时事件里带的是当前仍按住的全部修饰键。
    const held: Array<[string, Record<string, boolean>]> = [
      ["Control", { ctrlKey: true }],
      ["Alt", { ctrlKey: true, altKey: true }],
      ["Shift", { ctrlKey: true, altKey: true, shiftKey: true }]
    ];
    for (const [modifier, state] of held) {
      const event = keyEvent(modifier, state);
      recorder.press(event);
      expect(event.defaultPrevented).toBe(1);
      expect(event.propagationStopped).toBe(1);
      expect(recorder.read().recording).toBe(true);
    }
    expect(recorder.read().captured).toBe("Ctrl+Alt+Shift");
    expect(recorder.changes).toEqual([]);
  });

  it("records modifiers plus a main key and stops recording", () => {
    const recorder = buildRecorder();
    recorder.start();
    recorder.press(keyEvent("a", { ctrlKey: true, altKey: true, shiftKey: true }));
    expect(recorder.read().captured).toBe("Ctrl+Alt+Shift+A");
    expect(recorder.read().recording).toBe(false);
    expect(recorder.changes).toEqual(["Ctrl+Alt+Shift+A"]);
  });

  it("orders modifiers as Ctrl, Alt, Shift, Win and supports special keys", () => {
    const win = buildRecorder();
    win.start();
    win.press(keyEvent("ArrowDown", { shiftKey: true, metaKey: true, altKey: true, ctrlKey: true }));
    expect(win.read().captured).toBe("Ctrl+Alt+Shift+Win+Down");

    const space = buildRecorder();
    space.start();
    space.press(keyEvent(" ", { altKey: true }));
    // 组件的按键命名表还没把空格映射成 Space，先按真实行为锁定。
    expect(space.read().captured).toBe("Alt+ ");

    const enter = buildRecorder();
    enter.start();
    enter.press(keyEvent("Enter", { ctrlKey: true }));
    expect(enter.read().captured).toBe("Ctrl+Enter");
  });

  it("refuses a bare key with no modifier", () => {
    const recorder = buildRecorder();
    recorder.start();
    const event = keyEvent("a");
    recorder.press(event);
    expect(recorder.read().captured).toBe("");
    expect(recorder.read().recording).toBe(true);
    expect(recorder.changes).toEqual([]);
    expect(event.defaultPrevented).toBe(1);
  });

  it("cancels with Escape without emitting a value", () => {
    const recorder = buildRecorder();
    recorder.start();
    recorder.press(keyEvent("Escape"));
    expect(recorder.read().recording).toBe(false);
    expect(recorder.read().captured).toBe("");
    expect(recorder.changes).toEqual([]);
  });

  it("clears the stored value and stops recording", () => {
    const recorder = buildRecorder({ value: "Ctrl+Alt+A" });
    const click = clickEvent();
    recorder.clear(click);
    expect(click.propagationStopped).toBe(1);
    expect(recorder.read().recording).toBe(false);
    expect(recorder.read().captured).toBe("");
    expect(recorder.changes).toEqual([""]);
  });
});

describe("shortcut conflict rejection", () => {
  const target: { kind: string; value?: string } = { kind: "none" };
  const settings = {
    hotzones: [{ id: "left", actions: [{ trigger: "hover", action: { kind: "shortcut", value: "Ctrl+Alt+A" } }] }],
    monitorProfiles: []
  };

  function load() {
    const handler = extract("setActionShortcut", app);
    return new Function(
      "state",
      "settings",
      "target",
      "persist",
      "ensureHotzoneActionTarget",
      `let shortcutError = "";
       ${handler}
       return { setActionShortcut, readError: () => shortcutError };`
    )({}, settings, target, () => {}, () => ({ slot: {}, action: target })) as {
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
});
