<script lang="ts">
  import { formatShortcut, heldModifiers, isModifierKey, MODIFIER_NAMES, resolveKeyName } from "./shortcut-keys";
  export let value = ""; export let onChange: (value: string) => void; export let label = "录制快捷键"; 
  let recording = false; let captured = ""; let root: HTMLButtonElement;
  function start() { recording = true; captured = ""; root.focus(); }
  function keydown(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault(); e.stopPropagation();
    if (e.key === "Escape") { recording = false; captured = ""; return; }
    const modifiers = heldModifiers(e);
    if (isModifierKey(e)) { captured = modifiers.map((key) => MODIFIER_NAMES[key]).join("+"); return; }
    const key = resolveKeyName(e);
    if (!key) return;
    // 修饰键可选：单个字母、数字、F 键、方向键等都能直接作为快捷键。
    const next = formatShortcut(modifiers, key);
    captured = next; recording = false; onChange(next);
  }
  function clear(e: MouseEvent) { e.stopPropagation(); recording = false; captured = ""; onChange(""); }
</script>
<div class:recording class="shortcut-recorder"><button bind:this={root} aria-label={label} on:click={start} on:keydown={keydown} type="button">{#if recording && captured}<span class="key-row">{#each captured.split("+") as key}<kbd>{key}</kbd>{/each}</span>{:else if value}<span class="key-row">{#each value.split("+") as key}<kbd>{key}</kbd>{/each}</span>{:else}<span class="empty">{recording ? "请按任意键" : "点击录制快捷键（可单键）"}</span>{/if}</button>{#if value}<button class="clear" aria-label="清除快捷键" on:click={clear} type="button">×</button>{/if}</div>
<style>
.shortcut-recorder{min-width:0;display:grid;grid-template-columns:minmax(0,1fr) 30px;gap:6px}.shortcut-recorder:not(:has(.clear)){grid-template-columns:1fr}.shortcut-recorder>button:first-child{height:38px;min-width:0;padding:5px 9px;border:1px solid var(--line-strong);border-radius:6px;background:var(--bg);color:var(--ink);display:flex;align-items:center;justify-content:flex-start}.shortcut-recorder>button:first-child:hover,.shortcut-recorder.recording>button:first-child{border-color:var(--accent);box-shadow:0 0 0 3px var(--accent-bg)}.key-row{display:flex;gap:4px;overflow:hidden}.key-row kbd{min-width:28px;height:24px;padding:0 6px;border:1px solid var(--line-strong);border-bottom-width:2px;border-radius:4px;background:var(--surface);font:600 11px/21px "Segoe UI Variable",sans-serif;text-align:center}.empty{color:var(--muted);font-size:12px}.clear{width:30px;height:30px;border:1px solid var(--line);background:transparent;color:var(--muted);font-size:18px}.clear:hover{color:var(--danger);border-color:var(--danger)}
</style>
