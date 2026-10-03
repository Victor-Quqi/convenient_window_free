import type { UiKey } from "./i18n";

export const runtimeErrorKeys = {
  invalid_config: "errorInvalidConfig",
  unsupported_schema: "errorUnsupportedSchema",
  config_save_failed: "errorConfigSave",
  unknown_message: "errorUnknownMessage",
  runtime_operation_failed: "errorRuntime",
  input_monitor_failed: "errorInputMonitor",
  window_operation_failed: "errorWindow",
  action_failed: "errorAction",
  screen_capture_failed: "errorScreenCapture",
  ocr_failed: "errorOcr",
  volume_adjustment_failed: "errorVolume",
  brightness_adjustment_failed: "errorBrightness"
} satisfies Record<string, UiKey>;

export interface RuntimeError {
  code: string;
  details?: string;
  requestId?: string;
}

export function runtimeErrorKey(value: unknown): UiKey {
  const code = value && typeof value === "object" ? (value as RuntimeError).code : undefined;
  const key = typeof code === "string" && Object.hasOwn(runtimeErrorKeys, code)
    ? runtimeErrorKeys[code as keyof typeof runtimeErrorKeys] : "errorRuntime";
  return key;
}

export function runtimeErrorText(value: unknown, ui: (key: UiKey) => string): string {
  return ui(runtimeErrorKey(value));
}
