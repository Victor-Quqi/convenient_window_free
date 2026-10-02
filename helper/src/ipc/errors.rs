use serde::{Deserialize, Serialize};

/// Stable protocol identifiers. Platform diagnostics must never be used as UI copy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidConfig,
    UnsupportedSchema,
    ConfigSaveFailed,
    UnknownMessage,
    RuntimeOperationFailed,
    InputMonitorFailed,
    WindowOperationFailed,
    ActionFailed,
    ScreenCaptureFailed,
    OcrFailed,
    VolumeAdjustmentFailed,
    BrightnessAdjustmentFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeError {
    pub code: ErrorCode,
    /// Diagnostic text is for logs and support, never for translation or display.
    pub details: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl RuntimeError {
    pub fn new(code: ErrorCode, details: impl Into<String>) -> Self {
        Self {
            code,
            details: details.into(),
            request_id: None,
        }
    }

    pub fn for_request(mut self, id: &str) -> Self {
        self.request_id = Some(id.to_owned());
        self
    }

    pub fn data(&self) -> serde_json::Value {
        serde_json::json!(self)
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.details)
    }
}

impl std::error::Error for RuntimeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_the_frontend_contract_and_keep_request_correlation() {
        let codes: Vec<String> =
            serde_json::from_str(include_str!("../../../tests/fixtures/runtime-errors.json"))
                .unwrap();
        for code in codes {
            let typed: ErrorCode = serde_json::from_value(serde_json::json!(code)).unwrap();
            let payload = RuntimeError::new(typed, "diagnostic")
                .for_request("request-42")
                .data();
            assert_eq!(payload["code"], code);
            assert_eq!(payload["requestId"], "request-42");
            assert!(payload.get("message").is_none());
        }
    }
}
