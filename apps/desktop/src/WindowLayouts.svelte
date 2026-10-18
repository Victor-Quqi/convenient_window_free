<script lang="ts">
  import type { HelperClient } from "./helper-client";
  import { format, translator } from "./i18n";
  import type { UiKey } from "./i18n";
  import {
    createWindowLayoutId,
    loadWindowLayouts,
    saveWindowLayouts,
    type WindowLayoutCapture,
    type WindowLayoutSnapshot,
    type WindowLayoutWindow,
    type WindowLayoutRestoreResult
  } from "./window-layout-store";

  export let helper: HelperClient;
  export let english = false;
  export let connected = false;
  export let supported = false;
  export let platformSupportsWindowControl = true;

  let layouts: WindowLayoutSnapshot[] = loadWindowLayouts();
  let busy: "capture" | string = "";
  let editingId = "";
  let editName = "";
  let status: { kind: "success" | "warning" | "error"; text: string } | null = null;

  let ui = translator("zh-CN");
  $: ui = translator(english ? "en-US" : "zh-CN");
  function text(key: UiKey, values: Record<string, string | number> = {}): string { return format(ui(key), values); }

  async function captureLayout(): Promise<void> {
    if (!connected || !supported || busy) return;
    busy = "capture";
    status = null;
    try {
      const result: WindowLayoutCapture = await helper.captureWindowLayout();
      const windows = Array.isArray(result.windows) ? result.windows : [];
      const number = layouts.length + 1;
      const snapshot: WindowLayoutSnapshot = {
        id: createWindowLayoutId(),
        name: text("windowLayoutsDefaultName", { count: number }),
        capturedAt: result.capturedAt ?? Date.now(),
        windows
      };
      layouts = [snapshot, ...layouts].slice(0, 24);
      saveWindowLayouts(layouts);
      status = {
        kind: windows.length ? "success" : "warning",
        text: windows.length
          ? text("windowLayoutsRecorded", { count: windows.length })
          : text("windowLayoutsNoWindows")
      };
    } catch (error) {
      status = { kind: "error", text: errorMessage(error) };
    } finally {
      busy = "";
    }
  }

  async function restoreLayout(layout: WindowLayoutSnapshot): Promise<void> {
    if (!connected || !supported || busy) return;
    busy = layout.id;
    status = null;
    try {
      const result: WindowLayoutRestoreResult = await helper.restoreWindowLayout(layout.windows);
      const skipped = Number.isFinite(result.skipped) ? result.skipped : 0;
      const restored = Number.isFinite(result.restored) ? result.restored : 0;
      status = {
        kind: skipped ? "warning" : "success",
        text: skipped
          ? text("windowLayoutsRestoredSkipped", { restored, skipped })
          : text("windowLayoutsRestored", { count: restored })
      };
    } catch (error) {
      status = { kind: "error", text: errorMessage(error) };
    } finally {
      busy = "";
    }
  }

  function beginRename(layout: WindowLayoutSnapshot): void {
    editingId = layout.id;
    editName = layout.name;
  }

  function commitRename(layout: WindowLayoutSnapshot): void {
    const name = editName.trim().slice(0, 80);
    if (!name) return;
    layouts = layouts.map((item) => item.id === layout.id ? { ...item, name } : item);
    saveWindowLayouts(layouts);
    editingId = "";
    editName = "";
  }

  function deleteLayout(layout: WindowLayoutSnapshot): void {
    layouts = layouts.filter((item) => item.id !== layout.id);
    saveWindowLayouts(layouts);
    if (editingId === layout.id) editingId = "";
    status = { kind: "success", text: text("windowLayoutsDeleted") };
  }

  function previewStyle(layout: WindowLayoutSnapshot, window: WindowLayoutWindow): string {
    const rect = window.placementRect ?? window.rect;
    const bounds = layoutBounds(layout.windows);
    const width = Math.max(1, bounds.right - bounds.left);
    const height = Math.max(1, bounds.bottom - bounds.top);
    const left = ((rect.left - bounds.left) / width) * 100;
    const top = ((rect.top - bounds.top) / height) * 100;
    const itemWidth = ((rect.right - rect.left) / width) * 100;
    const itemHeight = ((rect.bottom - rect.top) / height) * 100;
    return `left:${clamp(left, 1, 94)}%;top:${clamp(top, 1, 94)}%;width:${clamp(itemWidth, 4, 96)}%;height:${clamp(itemHeight, 5, 94)}%`;
  }

  function layoutBounds(windows: WindowLayoutWindow[]): { left: number; top: number; right: number; bottom: number } {
    if (!windows.length) return { left: 0, top: 0, right: 1, bottom: 1 };
    const first = windows[0].placementRect ?? windows[0].rect;
    return windows.reduce((bounds, item) => {
      const rect = item.placementRect ?? item.rect;
      return {
        left: Math.min(bounds.left, rect.left),
        top: Math.min(bounds.top, rect.top),
        right: Math.max(bounds.right, rect.right),
        bottom: Math.max(bounds.bottom, rect.bottom)
      };
    }, { left: first.left, top: first.top, right: first.right, bottom: first.bottom });
  }

  function formatDate(value: number): string {
    try { return new Intl.DateTimeFormat(english ? "en-US" : "zh-CN", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(value); }
    catch { return new Date(value).toLocaleString(); }
  }

  function errorMessage(error: unknown): string {
    const message = error instanceof Error ? error.message.trim() : "";
    // Older helpers report an unsupported request as the raw message name
    // (for example `window.layout.capture`). Keep that protocol detail out
    // of the UI and show the actionable localised recovery hint instead.
    if (message && !message.includes("window.layout.") && !message.includes("unknown_message") && !message.includes("timed out")) return message;
    return text("windowLayoutsError");
  }

  function clamp(value: number, min: number, max: number): number { return Math.min(max, Math.max(min, value)); }
</script>

<section class="window-layouts-panel" aria-label={text("windowLayouts")}>
  <div class="window-layouts-intro">
    <div>
      <span class="eyebrow">{text("windowLayouts")}</span>
      <h2>{text("windowLayoutsHeading")}</h2>
      <p>{text("windowLayoutsDescription")}</p>
    </div>
    <button class="apply layout-capture" disabled={!connected || !supported || Boolean(busy)} on:click={captureLayout} type="button">
      <span aria-hidden="true">＋</span>{busy === "capture" ? text("windowLayoutsRecording") : text("windowLayoutsRecord")}
    </button>
  </div>

  {#if !connected}
    <div class="window-layouts-connection" role="status"><i></i>{text("windowLayoutsConnection")}</div>
  {:else if !platformSupportsWindowControl}
    <div class="window-layouts-connection" role="status"><i></i>{text("windowLayoutsUnsupported")}</div>
  {:else if !supported}
    <div class="window-layouts-connection" role="status"><i></i>{text("windowLayoutsHelperOutdated")}</div>
  {/if}
  {#if status}
    <div class:success={status.kind === "success"} class:warning={status.kind === "warning"} class:error={status.kind === "error"} class="window-layouts-status" role="status" aria-live="polite"><i></i>{status.text}</div>
  {/if}

  {#if layouts.length === 0}
    <div class="window-layouts-empty">
      <div class="layout-empty-mark" aria-hidden="true"><i></i><i></i><i></i></div>
      <strong>{text("windowLayoutsEmpty")}</strong>
      <p>{text("windowLayoutsEmptyDescription")}</p>
    </div>
  {:else}
    <div class="window-layout-list">
      {#each layouts as layout (layout.id)}
        <article class="window-layout-row">
          <button class="layout-preview" aria-label={text("windowLayoutsRestoreAria", { name: layout.name })} disabled={!connected || !supported || Boolean(busy)} on:click={() => restoreLayout(layout)} type="button">
            <span class="layout-preview-grid" aria-hidden="true"></span>
            {#each layout.windows.slice(0, 18) as item}
              <span class:minimized={item.minimized} class="layout-preview-window" style={previewStyle(layout, item)}></span>
            {/each}
            {#if layout.windows.length > 18}<small>+{layout.windows.length - 18}</small>{/if}
          </button>
          <div class="window-layout-copy">
            {#if editingId === layout.id}
              <input aria-label={text("windowLayoutsLayoutName")} bind:value={editName} on:keydown={(event) => { if (event.key === "Enter") commitRename(layout); if (event.key === "Escape") editingId = ""; }} />
            {:else}
              <button class="layout-name" on:click={() => beginRename(layout)} title={text("windowLayoutsRename")} type="button">{layout.name}</button>
            {/if}
            <p>{text("windowLayoutsWindowCount", { count: layout.windows.length })} · {formatDate(layout.capturedAt)}</p>
          </div>
          <div class="window-layout-actions">
            {#if editingId === layout.id}<button class="icon-button" aria-label={text("windowLayoutsSaveName")} title={text("windowLayoutsSaveName")} on:click={() => commitRename(layout)} type="button">✓</button>{/if}
            <button class="apply layout-restore" disabled={!connected || !supported || Boolean(busy)} on:click={() => restoreLayout(layout)} type="button">{busy === layout.id ? text("windowLayoutsRestoring") : text("windowLayoutsRestore")}</button>
            <button class="icon-button danger-icon" aria-label={text("windowLayoutsDeleted")} title={text("windowLayoutsDeleted")} on:click={() => deleteLayout(layout)} type="button">×</button>
          </div>
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  .window-layouts-panel { gap: 14px; }
  .window-layouts-intro { align-items: center; gap: 12px; }
  .window-layouts-intro h2 { font-size: var(--fs-xl); letter-spacing: 0; }
  .window-layouts-intro p { max-width: 32em; margin-top: 5px; line-height: 1.5; }
  .layout-capture, .layout-restore {
    border: 1px solid var(--accent-line);
    border-radius: var(--radius-sm);
    background: var(--accent-bg);
    color: var(--accent-soft);
    box-shadow: none;
    font-weight: 620;
    transition: background var(--dur-fast), border-color var(--dur-fast), transform var(--dur-fast);
  }
  .layout-capture:hover:not(:disabled), .layout-restore:hover:not(:disabled) {
    border-color: var(--accent);
    background: var(--hover);
    transform: translateY(-1px);
  }
  .window-layouts-status, .window-layouts-connection { border-radius: var(--radius-sm); }
  .window-layout-list { gap: 9px; }
  .window-layout-row {
    grid-template-columns: 64px minmax(0, 1fr);
    grid-template-rows: minmax(34px, auto) 34px;
    gap: 8px 12px;
    padding: 11px 12px;
    border-radius: var(--radius-sm);
    background: var(--surface);
    box-shadow: none;
    transition: border-color var(--dur-fast), background var(--dur-fast);
  }
  .window-layout-row:hover { border-color: var(--line-strong); background: var(--raised); }
  .layout-preview { grid-column: 1; grid-row: 1 / span 2; width: 64px; height: 50px; border-radius: 7px; }
  .layout-preview-window.minimized { border-style: dashed; opacity: .72; }
  .window-layout-copy { grid-column: 2; grid-row: 1; align-self: end; }
  .layout-name { font-size: var(--fs-md); }
  .window-layout-copy p { margin-top: 4px; }
  .window-layout-actions {
    grid-column: 1 / -1;
    grid-row: 2;
    display: grid;
    grid-template-columns: 32px minmax(0, 1fr) 32px;
    align-items: center;
    justify-items: stretch;
    gap: 8px;
    padding: 0;
  }
  .window-layout-actions .icon-button:first-child { grid-column: 1; }
  .window-layout-actions .layout-restore { grid-column: 2; justify-self: center; width: min(100%, 154px); min-height: 31px; }
  .window-layout-actions .danger-icon { grid-column: 3; justify-self: end; }
  .window-layout-actions .icon-button { width: 30px; height: 30px; border-radius: 7px; }
  .layout-name:hover { text-decoration: underline; text-underline-offset: 3px; }
  @media (max-width: 520px) {
    .window-layouts-intro { display: flex; align-items: flex-start; }
    .layout-capture { align-self: center; }
    .window-layout-row { grid-template-columns: 56px minmax(0, 1fr); }
    .layout-preview { width: 56px; height: 46px; }
  }
</style>
