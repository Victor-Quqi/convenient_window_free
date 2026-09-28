<script lang="ts">
  import { onMount, tick } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import AdjustmentHud from "./AdjustmentHud.svelte";
  import { FeedbackOrder, type AdjustmentFeedback, type AdjustmentSnapshot } from "./adjustment-feedback";
  import { LANGUAGE_KEY, resolveInitialLanguage, translator, type Language } from "./i18n";

  // 桌面宿主的提示条接线：窗口与事件来自 Tauri，绘制交给共享的 AdjustmentHud。
  let feedback = $state<AdjustmentFeedback | null>(null);
  // 提示条是独立窗口，语言和设置窗口共用同一个 localStorage 键；默认同样跟随系统语言。
  let language = $state<Language>(initialLanguage());
  const ui = $derived(translator(language));
  const order = new FeedbackOrder();

  function initialLanguage(): Language {
    let stored: unknown = null;
    try { stored = localStorage.getItem(LANGUAGE_KEY); } catch { /* 忽略隐私模式读取失败 */ }
    return resolveInitialLanguage(stored, typeof navigator === "undefined" ? undefined : navigator.language);
  }

  function syncLanguage(): void {
    language = initialLanguage();
  }

  async function receive(snapshot: AdjustmentSnapshot) {
    if (!order.accept(snapshot)) return;
    // 设置窗口改语言不会重启这个窗口，每次收到反馈都按最新的持久化选择取词。
    syncLanguage();
    feedback = snapshot.feedback;
    if (!feedback) return;
    await tick();
    await invoke("adjustment_hud_present", { revision: snapshot.revision });
  }

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // 同一个 WebView2 配置下的其它窗口修改语言时也同步一份。
    const onStorage = (event: StorageEvent) => {
      if (event.key === LANGUAGE_KEY) syncLanguage();
    };
    window.addEventListener("storage", onStorage);
    void (async () => {
      unlisten = await listen<AdjustmentSnapshot>("adjustment-feedback", event => {
        void receive(event.payload);
      });
      if (disposed) { unlisten(); return; }
      // Revisions order both live events and the initial snapshot across reconnects.
      await receive(await invoke<AdjustmentSnapshot>("adjustment_hud_ready"));
    })();
    return () => { disposed = true; unlisten?.(); window.removeEventListener("storage", onStorage); };
  });

  $effect(() => {
    document.documentElement.lang = language === "en-US" ? "en" : "zh-CN";
    // 窗口本身无边框、不进任务栏，标题只影响辅助工具读取，按界面语言同步。
    void getCurrentWindow()
      .setTitle(`${ui("adjustmentVolume")} / ${ui("adjustmentBrightness")}`)
      .catch(() => { /* 权限或窗口不可用时忽略：标题不影响提示条本身 */ });
  });
</script>

<AdjustmentHud {feedback} {language} />
