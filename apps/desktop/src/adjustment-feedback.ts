import { runtimeErrorText } from "./runtime-error";
import type { RuntimeError } from "./runtime-error";
// 音量/亮度调节反馈的纯逻辑：两套宿主共用，界面措辞一律来自共享字典。
//
// 本文件在两套前端中必须逐字节一致：
//   utools-plugin/src/adjustment-feedback.ts            （私有 uTools 插件）
//   open-source/apps/desktop/src/adjustment-feedback.ts （公开独立桌面）
// 私有仓库 scripts/check-i18n-parity.mjs 会校验两侧哈希，任一侧单独修改都会失败。
// 桌面用它渲染底部提示条，uTools 宿主用它拼状态栏文字，措辞因此不会各写一套。

import type { UiKey } from "./i18n";

/** 与宿主无关的翻译函数：通常传 `translator(language)`。 */
export type AdjustmentTranslator = (key: UiKey) => string;

export interface AdjustmentFeedback {
  interaction: number;
  sequence: number;
  kind: "volume" | "brightness";
  level: { value: number; muted: boolean; deviceName: string } | null;
  pending: boolean;
  error: RuntimeError | null;
}

export function feedbackKindLabel(feedback: AdjustmentFeedback, ui: AdjustmentTranslator): string {
  return feedback.kind === "volume" ? ui("adjustmentVolume") : ui("adjustmentBrightness");
}

/** 提示条右侧的短值：百分比、静音、调节中；失败时为失败文案，没有可显示内容时返回空串。 */
export function feedbackValueText(feedback: AdjustmentFeedback, ui: AdjustmentTranslator): string {
  if (feedback.error) return ui("adjustmentFailed");
  if (feedback.level?.muted) return ui("adjustmentMuted");
  if (feedback.level) return `${Math.round(feedback.level.value * 100)}%`;
  return feedback.pending ? ui("adjustmentPending") : "";
}

/** 单行状态文案，供状态栏这类只有一行可用的宿主使用；失败时保留 helper 给出的原因。 */
export function feedbackStatusLine(feedback: AdjustmentFeedback, ui: AdjustmentTranslator): string {
  const label = feedbackKindLabel(feedback, ui);
  if (feedback.error) return runtimeErrorText(feedback.error, ui);
  if (feedback.level?.muted) return `${label} · ${ui("adjustmentMuted")}`;
  if (feedback.level) return `${label} ${Math.round(feedback.level.value * 100)}%`;
  return feedback.pending ? `${label} · ${ui("adjustmentPending")}` : label;
}

export interface AdjustmentSnapshot {
  revision: number;
  feedback: AdjustmentFeedback | null;
}

/** 桌面提示条用它丢弃重连后迟到的旧快照；uTools 宿主按 sequence 自行判断，用不到这个类。 */
export class FeedbackOrder {
  private revision = -1;
  accept(snapshot: AdjustmentSnapshot): boolean {
    if (snapshot.revision <= this.revision) return false;
    this.revision = snapshot.revision;
    return true;
  }
}
