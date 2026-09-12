<script lang="ts">
  import { onMount } from "svelte";
  import { nextRecordingStep } from "./shortcut-keys";
  export let value = ""; export let onChange: (value: string) => void; export let label = "录制快捷键"; 
  let recording = false; let captured = ""; let root: HTMLButtonElement;
  function start() { recording = true; captured = ""; root?.focus(); }
  function stop() { recording = false; captured = ""; }
  function clear(e: MouseEvent) { e.stopPropagation(); stop(); onChange(""); }

  // 录制期间在 window 的捕获阶段监听按键，而不是只监听按钮本身。
  // 否则焦点一旦不在按钮上，按键就送不到处理器；而 Ctrl+C、Ctrl+V、Ctrl+X
  // 这类组合键本身是浏览器快捷键，会被优先消费掉，表现成「组合键录不上」。
  // 按钮上也绑同一处理器作为兜底；捕获阶段已 stopPropagation，两者不会重复触发。
  function handleKeydown(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault(); e.stopPropagation();
    const step = nextRecordingStep(e);
    if (step.action === "cancel") { stop(); return; }
    if (step.action === "wait") { captured = step.hint; return; }
    recording = false; captured = step.value; onChange(step.value);
  }
  onMount(() => {
    window.addEventListener("keydown", handleKeydown, true);
    return () => window.removeEventListener("keydown", handleKeydown, true);
  });
</script>
<div class="shortcut-recorder-wrap"><div class:recording class="shortcut-recorder"><button bind:this={root} aria-label={label} on:click={start} on:keydown={handleKeydown} type="button">{#if recording && captured}<span class="key-row">{#each captured.split("+") as key}<kbd>{key}</kbd>{/each}</span>{:else if value}<span class="key-row">{#each value.split("+") as key}<kbd>{key}</kbd>{/each}</span>{:else}<span class="empty">{recording ? "请按任意键" : "点击录制快捷键（可单键）"}</span>{/if}</button>{#if value}<button class="clear" aria-label="清除快捷键" on:click={clear} type="button">×</button>{/if}</div></div>
<style>
.shortcut-recorder-wrap{min-width:0}
.shortcut-recorder{min-width:0;display:grid;grid-template-columns:minmax(0,1fr) 30px;gap:6px}.shortcut-recorder:not(:has(.clear)){grid-template-columns:1fr}.shortcut-recorder>button:first-child{height:38px;min-width:0;padding:5px 9px;border:1px solid var(--line-strong);border-radius:6px;background:var(--bg);color:var(--ink);display:flex;align-items:center;justify-content:flex-start}.shortcut-recorder>button:first-child:hover,.shortcut-recorder.recording>button:first-child{border-color:var(--accent);box-shadow:0 0 0 3px var(--accent-bg)}.key-row{display:flex;gap:4px;overflow:hidden}.key-row kbd{min-width:28px;height:24px;padding:0 6px;border:1px solid var(--line-strong);border-bottom-width:2px;border-radius:4px;background:var(--surface);font:600 11px/21px "Segoe UI Variable",sans-serif;text-align:center}.empty{color:var(--muted);font-size:12px}.clear{width:30px;height:30px;border:1px solid var(--line);background:transparent;color:var(--muted);font-size:18px}.clear:hover{color:var(--danger);border-color:var(--danger)}
</style>
