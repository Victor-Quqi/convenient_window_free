//! A managed helper exits when the exact desktop process that launched it exits.
use crate::scheduled_owner::ScheduledOwner;
use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use tokio::sync::broadcast;

static MANAGED: AtomicBool = AtomicBool::new(false);
static SCHEDULED_OWNER: OnceLock<ScheduledOwner> = OnceLock::new();

pub fn scheduled_instance() -> Option<String> {
    SCHEDULED_OWNER.get().map(ScheduledOwner::parameter)
}

fn scheduled_arguments(args: &[String]) -> Result<Option<ScheduledOwner>> {
    if args.get(1).map(String::as_str) != Some("--scheduled-owner") {
        return Ok(None);
    }
    // Validate before paths::initialize can write to the supplied data directory.
    if args.len() != 7
        || args[3] != "--data-dir"
        || args[5] != "--desktop-executable"
        || !std::path::Path::new(&args[4]).is_absolute()
        || !std::path::Path::new(&args[6]).is_absolute()
    {
        bail!("Invalid scheduled helper arguments");
    }
    Ok(Some(
        ScheduledOwner::parse(&args[2]).map_err(anyhow::Error::msg)?,
    ))
}

pub fn validate_scheduled_start() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let Some(request) = scheduled_arguments(&args)? else {
        return Ok(());
    };
    #[cfg(target_os = "windows")]
    {
        crate::windows_process::verify_desktop(
            request.pid,
            request.birth,
            std::path::Path::new(&args[6]),
        )
        .map_err(anyhow::Error::msg)?;
        if !crate::windows_process::current_elevated().map_err(anyhow::Error::msg)? {
            bail!("Scheduled helper did not receive administrator access");
        }
        let stop = open_stop_event(&request.stop_event())?;
        if unsafe { windows::Win32::System::Threading::WaitForSingleObject(stop.raw(), 0) }
            != windows::Win32::Foundation::WAIT_TIMEOUT
        {
            bail!("Scheduled helper start was cancelled");
        }
        let _ = SCHEDULED_OWNER.set(request);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = request;
        bail!("Scheduled helper requires Windows");
    }
}

#[cfg(target_os = "windows")]
fn open_stop_event(name: &str) -> Result<crate::windows_process::ProcessHandle> {
    use windows::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_ACCESS_RIGHTS};
    let name = crate::windows_process::wide(std::ffi::OsStr::new(name));
    Ok(unsafe {
        OpenEventW(
            SYNCHRONIZATION_ACCESS_RIGHTS(0x00100000),
            false,
            crate::windows_process::ptr(&name),
        )
    }
    .map(crate::windows_process::ProcessHandle::new)?)
}

pub fn managed() -> bool {
    MANAGED.load(Ordering::Acquire)
}

pub fn initialize(shutdown: broadcast::Sender<()>) -> Result<()> {
    let args = if let Some(request) = SCHEDULED_OWNER.get() {
        vec![
            "--desktop-owner".into(),
            request.pid.to_string(),
            request.birth.to_string(),
            "--desktop-stop-event".into(),
            request.stop_event(),
        ]
    } else {
        std::env::args().collect::<Vec<_>>()
    };
    let Some(index) = args.iter().position(|arg| arg == "--desktop-owner") else {
        return Ok(());
    };
    let pid: u32 = args
        .get(index + 1)
        .context("Missing desktop PID")?
        .parse()?;
    let birth: u64 = args
        .get(index + 2)
        .context("Missing desktop creation time")?
        .parse()?;
    #[cfg(target_os = "windows")]
    {
        let owner = crate::windows_process::ProcessHandle::open(pid).map_err(anyhow::Error::msg)?;
        if owner.birth().map_err(anyhow::Error::msg)? != birth
            || owner.try_wait().map_err(anyhow::Error::msg)?.is_some()
        {
            bail!("Desktop owner has exited or its PID was reused");
        }
        let stop = if let Some(index) = args.iter().position(|arg| arg == "--desktop-stop-event") {
            let name = args.get(index + 1).context("Missing desktop stop event")?;
            Some(open_stop_event(name)?)
        } else {
            None
        };
        MANAGED.store(true, Ordering::Release);
        std::thread::Builder::new()
            .name("desktop-owner".into())
            .spawn(move || {
                while matches!(owner.try_wait(), Ok(None)) {
                    if let Some(stop) = &stop {
                        use windows::Win32::Foundation::WAIT_OBJECT_0;
                        use windows::Win32::System::Threading::WaitForSingleObject;
                        if unsafe { WaitForSingleObject(stop.raw(), 0) } == WAIT_OBJECT_0 {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                crate::logging::write_line(
                    "main: desktop owner exited or requested stop; stopping managed helper",
                );
                let _ = shutdown.send(());
                // Bound cleanup even when a native hook or runtime task is stalled.
                std::thread::sleep(std::time::Duration::from_secs(3));
                std::process::exit(0);
            })?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (pid, birth, shutdown);
        bail!("Desktop owner monitoring requires Windows");
    }
    Ok(())
}

pub fn elevated() -> bool {
    #[cfg(target_os = "windows")]
    {
        crate::windows_process::current_elevated().unwrap_or(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_mode_rejects_argument_injection_before_data_access() {
        let owner = ScheduledOwner {
            pid: 12,
            birth: 34,
            nonce: uuid::Uuid::new_v4(),
        };
        let root = std::env::temp_dir();
        let mut args = vec![
            "helper.exe".into(),
            "--scheduled-owner".into(),
            owner.parameter(),
            "--data-dir".into(),
            root.to_string_lossy().into_owned(),
            "--desktop-executable".into(),
            root.join("desktop.exe").to_string_lossy().into_owned(),
        ];
        assert_eq!(scheduled_arguments(&args).unwrap(), Some(owner));
        args.push("--desktop-owner".into());
        assert!(scheduled_arguments(&args).is_err());
        args.pop();
        args[2].push_str("\" --data-dir C:\\injected");
        assert!(scheduled_arguments(&args).is_err());
        assert_eq!(scheduled_arguments(&["helper.exe".into()]).unwrap(), None);
    }
}
