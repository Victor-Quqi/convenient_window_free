//! Taskbar work is isolated from the input engine. No native library is loaded before the first opt-in.
use crate::config::TaskbarAppearanceConfig;
use crate::ipc::messages::HelperMessage;
use crate::logging;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskbarAppearanceStatus {
    pub state: String,
    pub materials: Vec<String>,
    pub available: bool,
    pub backgrounds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

fn supported_materials() -> Vec<String> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        ["transparent", "acrylic", "solid"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    }
}

impl TaskbarAppearanceStatus {
    fn idle() -> Self {
        Self {
            state: "inactive".into(),
            materials: supported_materials(),
            available: cfg!(all(target_os = "windows", target_arch = "x86_64")),
            backgrounds: 0,
            error_code: None,
        }
    }
    fn failure(message: String) -> Self {
        Self {
            state: "error".into(),
            materials: supported_materials(),
            available: false,
            backgrounds: 0,
            error_code: Some(message),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeSnapshot {
    state: u32,
    error: i32,
    backgrounds: u32,
    explorer: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeAppearanceOptions {
    mode: u32,
    opacity: u32,
    tint: u32,
    show_border: i32,
}

fn parse_tint(value: &str) -> u32 {
    let value = value.trim().trim_start_matches('#');
    u32::from_str_radix(value, 16).unwrap_or(0x233A63) & 0x00FF_FFFF
}

impl From<&TaskbarAppearanceConfig> for NativeAppearanceOptions {
    fn from(config: &TaskbarAppearanceConfig) -> Self {
        Self {
            mode: match config.mode.as_str() {
                "transparent" => 0,
                "solid" => 2,
                "acrylic" => 1,
                _ => 0, // Preserve the original clear-background fallback.
            },
            opacity: config.opacity.clamp(0, 100),
            tint: parse_tint(&config.tint),
            show_border: i32::from(config.show_border),
        }
    }
}

fn decode_status(snapshot: NativeSnapshot) -> TaskbarAppearanceStatus {
    TaskbarAppearanceStatus {
        state: match snapshot.state {
            0 => "inactive",
            1 => "connecting",
            2 => "applied",
            3 => "restoring",
            4 => "conflict",
            5 => "unsupported",
            _ => "error",
        }
        .into(),
        available: snapshot.state != 5,
        materials: supported_materials(),
        backgrounds: snapshot.backgrounds,
        error_code: (snapshot.error != 0).then(|| format!("0x{:08X}", snapshot.error as u32)),
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod native {
    use super::*;
    use anyhow::{Context, Result};
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    // BOOL is an i32, not Rust bool. Both exports use cdecl on Windows x64.
    type Update = unsafe extern "C" fn(i32, *const NativeAppearanceOptions, *mut NativeSnapshot);
    type Close = unsafe extern "C" fn();
    pub struct Controller {
        module: HMODULE,
        update: Update,
        close: Close,
    }
    impl Controller {
        pub fn new() -> Result<Self> {
            let bytes: &[u8] = include_bytes!(env!("CW_TASKBAR_APPEARANCE_DLL"));
            // Content-addressed immutable filenames avoid replacing a DLL still held by Explorer.
            let identity = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
            let directory = crate::paths::data_file("taskbar-native")
                .context("helper data directory unavailable")?;
            std::fs::create_dir_all(&directory)?;
            let path = directory.join(format!("taskbar-{identity:016x}.dll"));
            if path.exists() {
                anyhow::ensure!(
                    std::fs::read(&path)? == bytes,
                    "taskbar component integrity mismatch"
                );
            } else {
                let temporary = directory.join(format!(
                    "taskbar-{identity:016x}-{}.tmp",
                    std::process::id()
                ));
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(&temporary, &path)?;
            }
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let module = unsafe {
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            }
            .context("load native taskbar component")?;
            let functions = (|| unsafe {
                let update = GetProcAddress(module, PCSTR(b"CWTaskbarUpdate\0".as_ptr()))
                    .context("CWTaskbarUpdate missing")?;
                let close = GetProcAddress(module, PCSTR(b"CWTaskbarClose\0".as_ptr()))
                    .context("CWTaskbarClose missing")?;
                Ok::<_, anyhow::Error>((
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, Update>(update),
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(close),
                ))
            })();
            match functions {
                Ok((update, close)) => Ok(Self {
                    module,
                    update,
                    close,
                }),
                Err(error) => {
                    unsafe {
                        let _ = FreeLibrary(module);
                    }
                    Err(error)
                }
            }
        }
        pub fn reset(&mut self) {
            // Retain the loaded DLL; only detach the controller mapping. The
            // resident keeps original brushes and can adopt the next mapping.
            unsafe { (self.close)() };
        }
        pub fn update(
            &mut self,
            enabled: bool,
            config: &TaskbarAppearanceConfig,
        ) -> TaskbarAppearanceStatus {
            let options = NativeAppearanceOptions::from(config);
            let mut snapshot = NativeSnapshot::default();
            unsafe { (self.update)(i32::from(enabled), &options, &mut snapshot) };
            decode_status(snapshot)
        }
    }
    impl Drop for Controller {
        fn drop(&mut self) {
            unsafe {
                (self.close)();
                let _ = FreeLibrary(self.module);
            }
        }
    }
}

struct Request {
    enabled: bool,
    revision: u64,
    settings: TaskbarAppearanceConfig,
}

struct Shared {
    request: Mutex<Request>,
    stop: AtomicBool,
    wake: Condvar,
    status: Mutex<TaskbarAppearanceStatus>,
}

pub struct TaskbarAppearanceWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}
impl TaskbarAppearanceWorker {
    pub fn new(events: broadcast::Sender<HelperMessage>) -> Self {
        let shared = Arc::new(Shared {
            request: Mutex::new(Request {
                enabled: false,
                revision: 0,
                settings: TaskbarAppearanceConfig::default(),
            }),
            stop: AtomicBool::new(false),
            wake: Condvar::new(),
            status: Mutex::new(TaskbarAppearanceStatus::idle()),
        });
        let state = shared.clone();
        let worker = thread::Builder::new()
            .name("taskbar-appearance".into())
            .spawn(move || run_worker(state, events));
        match worker {
            Ok(thread) => Self {
                shared,
                thread: Some(thread),
            },
            Err(error) => {
                *shared.status.lock().unwrap() =
                    TaskbarAppearanceStatus::failure(error.to_string());
                Self {
                    shared,
                    thread: None,
                }
            }
        }
    }
    pub fn configure(&self, enabled: bool, settings: &TaskbarAppearanceConfig) {
        let mut request = self
            .shared
            .request
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        request.enabled = enabled;
        request.settings = settings.clone();
        request.revision = request.revision.wrapping_add(1);
        self.shared.wake.notify_one();
    }
    #[cfg(test)]
    pub fn status(&self) -> TaskbarAppearanceStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
impl Drop for TaskbarAppearanceWorker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_one();
        // Native attachment is bounded to one second, close waits at most one second.
        if let Some(worker) = self.thread.take() {
            if worker.join().is_err() {
                logging::write_line("taskbar: worker panicked");
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum UpdateDecision {
    Poll,
    Reset,
    Wait,
}

#[derive(Default)]
struct RecoveryPolicy {
    revision: u64,
    attempts: usize,
    retry_at: Option<u64>,
    last_probe: Option<u64>,
}
impl RecoveryPolicy {
    const DELAYS: [u64; 4] = [300, 900, 1800, 3600];
    fn retryable(status: &TaskbarAppearanceStatus) -> bool {
        status.state == "error"
            && matches!(
                status.error_code.as_deref(),
                Some(
                    "0x80070102"
                        | "0x800705B4"
                        | "0x80070490"
                        | "0x8007045A"
                        | "0x80010108"
                        | "0x80010001"
                        | "0x800706BA"
                        | "0x80004005"
                )
            )
    }
    fn decide(
        &mut self,
        enabled: bool,
        revision: u64,
        now: u64,
        status: &TaskbarAppearanceStatus,
    ) -> UpdateDecision {
        if !enabled || revision != self.revision {
            self.revision = revision;
            self.attempts = 0;
            self.retry_at = None;
            self.last_probe = Some(now);
            return if enabled
                && matches!(status.state.as_str(), "error" | "conflict" | "unsupported")
            {
                UpdateDecision::Reset
            } else {
                UpdateDecision::Poll
            };
        }
        if Self::retryable(status) && self.attempts < Self::DELAYS.len() {
            let due = *self
                .retry_at
                .get_or_insert(now + Self::DELAYS[self.attempts]);
            if now < due {
                return UpdateDecision::Wait;
            }
            self.attempts += 1;
            self.retry_at = None;
            self.last_probe = Some(now);
            return UpdateDecision::Reset;
        }
        if matches!(status.state.as_str(), "error" | "conflict" | "unsupported") {
            // Poll even terminal results, at low frequency, so an Explorer
            // recreation or an exited competing tool can resume without Retry.
            if self
                .last_probe
                .is_some_and(|last| now.saturating_sub(last) < 2000)
            {
                return UpdateDecision::Wait;
            }
            self.last_probe = Some(now);
            return UpdateDecision::Poll;
        }
        self.retry_at = None;
        if status.state == "applied" {
            self.attempts = 0;
        }
        UpdateDecision::Poll
    }
    fn visible(
        &self,
        enabled: bool,
        mut status: TaskbarAppearanceStatus,
    ) -> TaskbarAppearanceStatus {
        if enabled && Self::retryable(&status) && self.attempts < Self::DELAYS.len() {
            status.state = "recovering".into();
        }
        status
    }
}

fn run_worker(shared: Arc<Shared>, events: broadcast::Sender<HelperMessage>) {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let mut controller: Option<native::Controller> = None;
    let start = Instant::now();
    let mut recovery = RecoveryPolicy::default();
    let mut native_status = TaskbarAppearanceStatus::idle();
    let mut last_report = Instant::now() - Duration::from_secs(1);
    let mut reported_status = None;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let (enabled, revision, appearance) = {
            let request = shared.request.lock().unwrap_or_else(|e| e.into_inner());
            (request.enabled, request.revision, request.settings.clone())
        };
        let decision = recovery.decide(
            enabled,
            revision,
            start.elapsed().as_millis() as u64,
            &native_status,
        );
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        if decision != UpdateDecision::Wait {
            if enabled && controller.is_none() {
                match native::Controller::new() {
                    Ok(native) => controller = Some(native),
                    Err(error) => {
                        logging::write_line(format!("taskbar: component unavailable: {error:#}"));
                        native_status = TaskbarAppearanceStatus::failure(format!("{error:#}"));
                    }
                }
            }
            if let Some(native) = controller.as_mut() {
                if decision == UpdateDecision::Reset {
                    native.reset();
                }
                native_status = native.update(enabled, &appearance);
                // Keep the controller while disabled after the first opt-in.
                // It owns the saved original brushes and avoids reinjection.
            } else if !enabled {
                native_status = TaskbarAppearanceStatus::idle();
            }
        }
        #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
        {
            native_status.state = "unavailable".into();
            native_status.available = false;
            let _ = (decision, &appearance);
        }
        let status = recovery.visible(enabled, native_status.clone());
        *shared.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
        if reported_status.as_ref() != Some(&status)
            || last_report.elapsed() >= Duration::from_secs(1)
        {
            let _ = events.send(HelperMessage::new(
                "taskbar.status",
                serde_json::to_value(&status).unwrap_or_default(),
            ));
            reported_status = Some(status);
            last_report = Instant::now();
        }
        let request = shared.request.lock().unwrap_or_else(|e| e.into_inner());
        if !shared.stop.load(Ordering::Acquire) && request.revision == revision {
            let _ = shared
                .wake
                .wait_timeout(request, Duration::from_millis(200));
        }
    }
    // Drop requests restoration before releasing the hook.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_worker_does_not_load_the_native_controller() {
        let (events, mut receiver) = broadcast::channel(8);
        let worker = TaskbarAppearanceWorker::new(events);
        worker.configure(false, &TaskbarAppearanceConfig::default());
        let message = receiver.blocking_recv().unwrap();
        assert_eq!(message.kind, "taskbar.status");
        let expected = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "inactive"
        } else {
            "unavailable"
        };
        assert_eq!(worker.status().state, expected);
        assert_eq!(worker.status().backgrounds, 0);
        drop(worker);
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn native_exports_accept_disabled_without_touching_explorer() {
        let mut controller = native::Controller::new().expect("load embedded component");
        let status = controller.update(false, &TaskbarAppearanceConfig::default());
        assert_eq!(status.state, "inactive");
        assert_eq!(status.backgrounds, 0);
        drop(controller);
    }
    #[test]
    fn transient_failure_retries_with_backoff_and_can_be_cancelled() {
        let error = decode_status(NativeSnapshot {
            state: 6,
            error: 0x80070102u32 as i32,
            ..Default::default()
        });
        let mut policy = RecoveryPolicy::default();
        assert_eq!(policy.decide(true, 1, 0, &error), UpdateDecision::Reset);
        assert_eq!(policy.visible(true, error.clone()).state, "recovering");
        assert_eq!(policy.decide(true, 1, 10, &error), UpdateDecision::Wait);
        assert_eq!(policy.decide(true, 1, 309, &error), UpdateDecision::Wait);
        assert_eq!(policy.decide(true, 1, 310, &error), UpdateDecision::Reset);
        assert_eq!(policy.decide(false, 2, 311, &error), UpdateDecision::Poll);
        assert_eq!(policy.attempts, 0);
    }
    #[test]
    fn persistent_failures_stop_resetting_and_do_not_hide_real_errors() {
        let error = decode_status(NativeSnapshot {
            state: 6,
            error: 0x80070490u32 as i32,
            ..Default::default()
        });
        let mut policy = RecoveryPolicy::default();
        let mut now = 0;
        for delay in RecoveryPolicy::DELAYS {
            assert_eq!(policy.decide(true, 0, now, &error), UpdateDecision::Wait);
            now += delay;
            assert_eq!(policy.decide(true, 0, now, &error), UpdateDecision::Reset);
        }
        assert_eq!(policy.visible(true, error.clone()).state, "error");
        assert_eq!(
            policy.decide(true, 0, now + 1999, &error),
            UpdateDecision::Wait
        );
        assert_eq!(
            policy.decide(true, 0, now + 2000, &error),
            UpdateDecision::Poll
        );
        assert_eq!(
            policy.decide(true, 1, now + 2001, &error),
            UpdateDecision::Reset
        );
        let denied = decode_status(NativeSnapshot {
            state: 6,
            error: 0x80070005u32 as i32,
            ..Default::default()
        });
        assert_eq!(policy.visible(true, denied).state, "error");
    }
    #[test]
    fn conflict_is_rechecked_without_manual_retry_and_reenable_interrupts_restore() {
        let conflict = decode_status(NativeSnapshot {
            state: 4,
            ..Default::default()
        });
        let mut policy = RecoveryPolicy::default();
        assert_eq!(policy.decide(true, 0, 0, &conflict), UpdateDecision::Poll);
        assert_eq!(
            policy.decide(true, 0, 1999, &conflict),
            UpdateDecision::Wait
        );
        assert_eq!(
            policy.decide(true, 0, 2000, &conflict),
            UpdateDecision::Poll
        );
        let restoring = decode_status(NativeSnapshot {
            state: 3,
            ..Default::default()
        });
        assert_eq!(
            policy.decide(true, 1, 2001, &restoring),
            UpdateDecision::Poll
        );
    }
    #[test]
    fn native_snapshot_matches_four_u32_abi() {
        assert_eq!(std::mem::size_of::<NativeSnapshot>(), 16);
        assert_eq!(std::mem::size_of::<NativeAppearanceOptions>(), 16);
    }
    #[test]
    fn errors_and_restoration_are_not_reported_as_success() {
        for state in [1, 3, 4, 5, 6, 99] {
            assert_ne!(
                decode_status(NativeSnapshot {
                    state,
                    ..Default::default()
                })
                .state,
                "applied"
            );
        }
        assert_eq!(
            decode_status(NativeSnapshot {
                state: 3,
                ..Default::default()
            })
            .state,
            "restoring"
        );
    }
    #[test]
    fn preserves_hresult_and_background_count() {
        let status = decode_status(NativeSnapshot {
            state: 6,
            error: 0x80070490u32 as i32,
            backgrounds: 2,
            explorer: 42,
        });
        assert_eq!(status.error_code.as_deref(), Some("0x80070490"));
        assert_eq!(status.backgrounds, 2);
    }
    #[test]
    fn inactive_is_eligible_for_explicit_enable() {
        assert!(decode_status(NativeSnapshot::default()).available);
    }
    #[test]
    fn material_parameters_have_distinct_modes_and_bounded_native_values() {
        let mut config = TaskbarAppearanceConfig::default();
        for (mode, expected) in [("transparent", 0), ("acrylic", 1), ("solid", 2)] {
            config.mode = mode.into();
            config.opacity = 140;
            config.tint = "#aabbcc".into();
            config.show_border = true;
            let options = NativeAppearanceOptions::from(&config);
            assert_eq!(options.mode, expected);
            assert_eq!(options.opacity, 100);
            assert_eq!(options.tint, 0xAABBCC);
            assert_eq!(options.show_border, 1);
        }
    }
    #[test]
    fn status_advertises_material_support_independently_of_persistence() {
        let json = serde_json::to_value(TaskbarAppearanceStatus::idle()).unwrap();
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            assert_eq!(
                json["materials"],
                serde_json::json!(["transparent", "acrylic", "solid"])
            );
        } else {
            assert_eq!(json["materials"], serde_json::json!([]));
        }
        assert_eq!(json["state"], "inactive");
    }
}
