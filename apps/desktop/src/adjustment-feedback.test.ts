import { describe, expect, it } from "vitest";
import {
  FeedbackOrder,
  feedbackKindLabel,
  feedbackStatusLine,
  feedbackValueText,
  type AdjustmentFeedback
} from "./adjustment-feedback";
import { translator } from "./i18n";

describe("adjustment feedback", () => {
  const zh = translator("zh-CN");
  const en = translator("en-US");
  const feedback: AdjustmentFeedback = {
    interaction: 1, sequence: 1, kind: "volume", pending: false, error: null,
    level: { value: 0.426, muted: false, deviceName: "Speakers" }
  };
  const muted = { ...feedback, level: { ...feedback.level!, muted: true } };

  it("shows the readback, mute state and asynchronous failures in both languages", () => {
    expect(feedbackValueText(feedback, zh)).toBe("43%");
    expect(feedbackValueText({ ...feedback, pending: true }, zh)).toBe("43%");
    expect(feedbackValueText(muted, zh)).toBe("静音");
    expect(feedbackValueText(muted, en)).toBe("Muted");
    expect(feedbackValueText({ ...feedback, pending: true, level: null }, zh)).toBe("调节中");
    expect(feedbackValueText({ ...feedback, pending: true, level: null }, en)).toBe("Adjusting");
    expect(feedbackValueText({ ...feedback, error: { code: "volume_adjustment_failed" } }, zh)).toBe("调节失败");
    expect(feedbackValueText({ ...feedback, error: { code: "volume_adjustment_failed" }, pending: true }, en)).toBe("Adjustment failed");
    expect(feedbackKindLabel(feedback, zh)).toBe("音量");
    expect(feedbackKindLabel({ ...feedback, kind: "brightness" }, en)).toBe("Brightness");
  });

  it("builds the single-line status used by hosts without an indicator window", () => {
    expect(feedbackStatusLine(feedback, zh)).toBe("音量 43%");
    expect(feedbackStatusLine(feedback, en)).toBe("Volume 43%");
    expect(feedbackStatusLine(muted, zh)).toBe("音量 · 静音");
    expect(feedbackStatusLine({ ...feedback, level: null, pending: true }, en)).toBe("Volume · Adjusting");
    expect(feedbackStatusLine({ ...feedback, level: null, pending: true }, zh)).toBe("音量 · 调节中");
    // Diagnostic details never become user-facing copy.
    expect(feedbackStatusLine({ ...feedback, error: { code: "volume_adjustment_failed", details: "pactl missing" } }, en)).toBe(en("errorVolume"));
  });

  it("rejects late snapshots and accepts a new helper session after reset", () => {
    const order = new FeedbackOrder();
    expect(order.accept({ revision: 2, feedback })).toBe(true);
    expect(order.accept({ revision: 1, feedback: { ...feedback, pending: true } })).toBe(false);
    expect(order.accept({ revision: 2, feedback })).toBe(false);
    expect(order.accept({ revision: 3, feedback: null })).toBe(true);
    expect(order.accept({ revision: 4, feedback: { ...feedback, sequence: 1 } })).toBe(true);
  });
});
