//! Taskbar work is isolated from the input engine. No native library is loaded while disabled.
use crate::config::TaskbarAppearanceConfig;
use crate::ipc::messages::HelperMessage;
use crate::logging;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
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
        // Native attachment is bounded to one second, close waits at most one second.
        if let Some(worker) = self.thread.take() {
            if worker.join().is_err() {
                logging::write_line("taskbar: worker panicked");
            }
        }
    }
}

fn run_worker(shared: Arc<Shared>, events: broadcast::Sender<HelperMessage>) {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let mut controller: Option<native::Controller> = None;
    let mut previous_revision = 0;
    let mut last_report = std::time::Instant::now() - Duration::from_secs(1);
    let mut reported_status = None;
    let mut last_attempt: Option<u64> = None;
    while !shared.stop.load(Ordering::Acquire) {
        let (enabled, revision, appearance) = {
            let request = shared.request.lock().unwrap_or_else(|e| e.into_inner());
            (request.enabled, request.revision, request.settings.clone())
        };
        let mut status = shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            let terminal = matches!(status.state.as_str(), "error" | "unsupported" | "conflict");
            if enabled && terminal && revision != previous_revision {
                controller = None;
                last_attempt = None;
            }
            if enabled && controller.is_none() && last_attempt != Some(revision) {
                last_attempt = Some(revision);
                match native::Controller::new() {
                    Ok(native) => controller = Some(native),
                    Err(error) => {
                        logging::write_line(format!("taskbar: component unavailable: {error:#}"));
                        status = TaskbarAppearanceStatus::failure(format!("{error:#}"));
                    }
                }
            }
            if let Some(native) = controller.as_mut() {
                if !enabled || !terminal || revision != previous_revision {
                    status = native.update(enabled, &appearance);
                }
                if !enabled && status.state == "inactive" {
                    controller = None;
                }
            } else if !enabled {
                status = TaskbarAppearanceStatus::idle();
            }
        }
        #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
        {
            status.state = "unavailable".into();
            status.available = false;
            let _ = (
                &mut previous_revision,
                &mut last_attempt,
                revision,
                enabled,
                &appearance,
            );
        }
        previous_revision = revision;
        *shared.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
        // Periodic reports also serve newly connected/reloaded UI clients.
        if reported_status.as_ref() != Some(&status)
            || last_report.elapsed() >= Duration::from_secs(1)
        {
            let _ = events.send(HelperMessage::new(
                "taskbar.status",
                serde_json::to_value(&status).unwrap_or_default(),
            ));
            reported_status = Some(status);
            last_report = std::time::Instant::now();
        }
        thread::sleep(Duration::from_millis(250));
    }
    // Dropping the controller requests restoration before releasing the hook.
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
