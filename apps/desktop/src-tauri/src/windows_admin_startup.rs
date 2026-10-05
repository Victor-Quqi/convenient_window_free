use crate::scheduled_owner::ScheduledOwner;
use crate::windows_elevation::{quote_argument, ElevatedProcess};
use crate::windows_process::{self, ProcessHandle};
use serde::Serialize;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{Interface, BSTR, VARIANT};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::Shell::CommandLineToArgvW;

const SOURCE: &str = "com.ximizhou.convenientwindow.admin-startup.v1";

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupState {
    pub enabled: bool,
    pub needs_repair: bool,
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(error)?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct Scheduler {
    folder: ITaskFolder,
    name: BSTR,
    // COM interfaces must be released before the apartment.
    _apartment: Apartment,
}

impl Scheduler {
    fn connect() -> Result<Self, String> {
        let apartment = Apartment::new()?;
        let sid = ProcessHandle::open(std::process::id())?.user_sid()?;
        let folder = unsafe {
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).map_err(error)?;
            let empty = VARIANT::default();
            service
                .Connect(&empty, &empty, &empty, &empty)
                .map_err(error)?;
            service.GetFolder(&BSTR::from("\\")).map_err(error)?
        };
        Ok(Self {
            folder,
            name: BSTR::from(format!("ConvenientWindow.AdminHelper.{sid}")),
            _apartment: apartment,
        })
    }

    fn task(&self) -> Result<Option<IRegisteredTask>, String> {
        let task = match unsafe { self.folder.GetTask(&self.name) } {
            Ok(task) => task,
            Err(err) if matches!(err.code().0 as u32, 0x80070002 | 0x80070003) => return Ok(None),
            Err(err) => return Err(error(err)),
        };
        let mut source = BSTR::new();
        unsafe {
            task.Definition()
                .map_err(error)?
                .RegistrationInfo()
                .map_err(error)?
                .Source(&mut source)
                .map_err(error)?;
        }
        if source.to_string() != SOURCE {
            return Err("Administrator startup task name is occupied".into());
        }
        Ok(Some(task))
    }
}

fn error(err: windows::core::Error) -> String {
    err.to_string()
}

fn arguments(desktop: &Path, data: &Path) -> String {
    format!(
        "--scheduled-owner \"$(Arg0)\" --data-dir {} --desktop-executable {}",
        quote_argument(&data.to_string_lossy()),
        quote_argument(&desktop.to_string_lossy())
    )
}

fn action_matches(task: &IRegisteredTask, helper: &Path, data: &Path) -> Result<bool, String> {
    let desktop = std::env::current_exe().map_err(|e| e.to_string())?;
    unsafe {
        let actions = task.Definition().map_err(error)?.Actions().map_err(error)?;
        let mut count = 0;
        actions.Count(&mut count).map_err(error)?;
        if count != 1 {
            return Ok(false);
        }
        let action: IExecAction = actions.get_Item(1).map_err(error)?.cast().map_err(error)?;
        let mut path = BSTR::new();
        let mut args = BSTR::new();
        action.Path(&mut path).map_err(error)?;
        action.Arguments(&mut args).map_err(error)?;
        Ok(
            windows_process::same_path(Path::new(&path.to_string()), helper)
                && args.to_string() == arguments(&desktop, data),
        )
    }
}

pub fn state(helper: &Path, data: &Path) -> Result<StartupState, String> {
    let scheduler = Scheduler::connect()?;
    let Some(task) = scheduler.task()? else {
        return Ok(StartupState::default());
    };
    Ok(StartupState {
        enabled: true,
        needs_repair: !unsafe { task.Enabled().map_err(error)? }.as_bool()
            || !action_matches(&task, helper, data)?
            || !crate::windows_autostart::enabled_for_current_executable()
                .map_err(|e| e.to_string())?,
    })
}

// No logon trigger: the ordinary login host supplies its exact PID and stop event.
fn task_xml(sid: &str, desktop: &Path, helper: &Path, data: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Source>{SOURCE}</Source><Description>Convenient Window administrator helper</Description></RegistrationInfo>
  <Principals><Principal id="User"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>false</StartWhenAvailable><AllowStartOnDemand>true</AllowStartOnDemand><Enabled>true</Enabled><Hidden>true</Hidden><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
  <Actions Context="User"><Exec><Command>{helper}</Command><Arguments>{args}</Arguments><WorkingDirectory>{directory}</WorkingDirectory></Exec></Actions>
</Task>"#,
        helper = xml(&helper.to_string_lossy()),
        args = xml(&arguments(desktop, data)),
        directory = xml(&helper.parent().unwrap().to_string_lossy())
    )
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn registered_desktop(arguments: &str) -> Result<PathBuf, String> {
    let command = windows_process::wide(OsStr::new(&format!("helper.exe {arguments}")));
    let args = unsafe {
        let mut count = 0;
        let argv = CommandLineToArgvW(windows_process::ptr(&command), &mut count);
        if argv.is_null() {
            return Err(windows::core::Error::from_win32().to_string());
        }
        let result: Result<Vec<_>, _> = std::slice::from_raw_parts(argv, count as usize)
            .iter()
            .map(|value| value.to_string())
            .collect();
        let _ = LocalFree(HLOCAL(argv.cast()));
        result.map_err(|error| error.to_string())?
    };
    if args.len() != 7
        || args[1] != "--scheduled-owner"
        || args[2] != "$(Arg0)"
        || args[3] != "--data-dir"
        || args[5] != "--desktop-executable"
        || !Path::new(&args[6]).is_absolute()
    {
        return Err("Invalid administrator startup action".into());
    }
    Ok(PathBuf::from(&args[6]))
}

fn register(helper: &Path, data: &Path) -> Result<(), String> {
    if !windows_process::current_elevated()? {
        return Err("Administrator access is required to register the task".into());
    }
    let scheduler = Scheduler::connect()?;
    scheduler.task()?;
    let sid = ProcessHandle::open(std::process::id())?.user_sid()?;
    let desktop = std::env::current_exe().map_err(|e| e.to_string())?;
    // The ordinary account can read, run and delete the task, but cannot change its action or ACL.
    let sddl = format!("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGXSD;;;{sid})");
    unsafe {
        scheduler
            .folder
            .RegisterTask(
                &scheduler.name,
                &BSTR::from(task_xml(&sid, &desktop, helper, data)),
                TASK_CREATE_OR_UPDATE.0 | TASK_DONT_ADD_PRINCIPAL_ACE.0,
                &VARIANT::from(sid.as_str()),
                &VARIANT::default(),
                TASK_LOGON_INTERACTIVE_TOKEN,
                &VARIANT::from(sddl.as_str()),
            )
            .map_err(error)?;
    }
    Ok(())
}

pub fn remove(only_this_installation: bool) -> Result<(), String> {
    let scheduler = Scheduler::connect()?;
    let Some(task) = scheduler.task()? else {
        return Ok(());
    };
    if only_this_installation {
        let mut arguments = BSTR::new();
        unsafe {
            let actions = task.Definition().map_err(error)?.Actions().map_err(error)?;
            let mut count = 0;
            actions.Count(&mut count).map_err(error)?;
            if count != 1 {
                return Err("Invalid administrator startup action".into());
            }
            let action: IExecAction = actions.get_Item(1).map_err(error)?.cast().map_err(error)?;
            action.Arguments(&mut arguments).map_err(error)?;
        }
        let desktop = std::env::current_exe().map_err(|e| e.to_string())?;
        // Task Scheduler rewrites RegistrationInfo.URI to the registered task name.
        if !windows_process::same_path(&registered_desktop(&arguments.to_string())?, &desktop) {
            return Ok(());
        }
    }
    // Deleting a registration leaves this session's managed helper under its owner.
    unsafe {
        scheduler
            .folder
            .DeleteTask(&scheduler.name, 0)
            .map_err(error)
    }
}

pub fn enable(payload: &Path, data: &Path) -> Result<(), String> {
    if windows_process::current_elevated()? {
        return Err("adminDesktopElevated".into());
    }
    let helper = crate::supervisor::validate_payload(payload)?;
    let desktop = std::env::current_exe().map_err(|e| e.to_string())?;
    let args = format!(
        "--register-admin-startup {} {} {} {}",
        std::process::id(),
        windows_process::current_birth()?,
        quote_argument(&helper.to_string_lossy()),
        quote_argument(&data.to_string_lossy())
    );
    let process = crate::windows_elevation::run_as_admin(&desktop, &args)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match process.try_wait()? {
            Some(0) => break,
            Some(_) => return Err("adminStartupFailed".into()),
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            None => return Err("adminStartupFailed".into()),
        }
    }
    if let Err(error) = crate::windows_autostart::enable() {
        remove(false)?;
        return Err(error.to_string());
    }
    let status = state(&helper, data)?;
    if !status.enabled || status.needs_repair {
        return Err("adminStartupFailed".into());
    }
    Ok(())
}

pub fn handle_cli() -> Option<Result<(), String>> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--remove-admin-startup") => Some(if args.len() == 2 {
            remove(true)
        } else {
            Err("Invalid cleanup arguments".into())
        }),
        Some("--register-admin-startup") => Some((|| {
            if args.len() != 6 {
                return Err("Invalid registration arguments".into());
            }
            let pid = args[2].parse().map_err(|_| "Invalid desktop PID")?;
            let birth = args[3]
                .parse()
                .map_err(|_| "Invalid desktop creation time")?;
            let desktop = std::env::current_exe().map_err(|e| e.to_string())?;
            let _owner = windows_process::verify_desktop(pid, birth, &desktop)?;
            let helper = PathBuf::from(&args[4]);
            let data = PathBuf::from(&args[5]);
            let packaged = desktop
                .parent()
                .ok_or("Missing desktop directory")?
                .join("helper");
            let development = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/helper");
            let payload = helper.parent().ok_or("Missing helper directory")?;
            if !data.is_absolute()
                || !(windows_process::same_path(payload, &packaged)
                    || windows_process::same_path(payload, &development))
                || crate::supervisor::validate_payload(payload)? != helper
            {
                return Err("Invalid startup paths".into());
            }
            register(&helper, &data)
        })()),
        _ => None,
    }
}

pub fn launch(helper: &Path, data: &Path, owner_birth: u64) -> Result<ElevatedProcess, String> {
    let scheduler = Scheduler::connect()?;
    let task = scheduler.task()?.ok_or("adminStartupMissing")?;
    if !action_matches(&task, helper, data)? {
        return Err("adminStartupRepair".into());
    }
    unsafe {
        if task
            .GetInstances(0)
            .map_err(error)?
            .Count()
            .map_err(error)?
            != 0
        {
            return Err("Administrator startup task is already running".into());
        }
    }
    let owner = ScheduledOwner {
        pid: std::process::id(),
        birth: owner_birth,
        nonce: uuid::Uuid::new_v4(),
    };
    let owner = ScheduledOwner::parse(&owner.parameter())?;
    let name = windows_process::wide(OsStr::new(&owner.stop_event()));
    let stop = unsafe { CreateEventW(None, true, false, windows_process::ptr(&name)) }
        .map(ProcessHandle::new)
        .map_err(error)?;
    let current = ProcessHandle::open(std::process::id())?;
    let instance = unsafe {
        task.RunEx(
            &VARIANT::from(owner.parameter().as_str()),
            TASK_RUN_USE_SESSION_ID.0,
            current.session()? as i32,
            &BSTR::new(),
        )
        .map_err(error)?
    };
    let result = await_helper(helper, data, &owner);
    match result {
        Ok(process) => Ok(ElevatedProcess::from_handles(process, stop)),
        Err(error) => {
            unsafe {
                SetEvent(stop.raw()).map_err(self::error)?;
            }
            // Cancel a queued launch as well as a process that failed before ready.
            let _ = unsafe { instance.Stop() };
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let _ = unsafe { instance.Refresh() };
                match unsafe { instance.State() } {
                    Ok(state) if state != TASK_STATE_RUNNING && state != TASK_STATE_QUEUED => break,
                    Err(err) if err.code().0 as u32 == 0x8004130b => break,
                    _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                    _ => return Err("adminStartupStopFailed".into()),
                }
            }
            Err(error)
        }
    }
}

fn await_helper(
    helper: &Path,
    data: &Path,
    owner: &ScheduledOwner,
) -> Result<ProcessHandle, String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let ready: Result<ProcessHandle, String> = (|| {
            let token = crate::supervisor::read_valid_token(&data.join("auth-token"))?;
            let mut socket = crate::supervisor::connect_authenticated(&token)?;
            let message = socket.read().map_err(|e| e.to_string())?;
            let value: serde_json::Value =
                serde_json::from_str(message.to_text().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if value["type"] != "helper.ready"
                || value["data"]["scheduledInstance"].as_str() != Some(&owner.parameter())
            {
                return Err("Scheduled helper identity does not match".into());
            }
            let pid = value["data"]["processId"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or("Missing helper PID")?;
            let process = ProcessHandle::open(pid)?;
            if !process.elevated()?
                || process.try_wait()?.is_some()
                || !windows_process::same_path(&process.image()?, helper)
            {
                return Err("Scheduled helper process does not match".into());
            }
            Ok(process)
        })();
        if let Ok(process) = ready {
            return Ok(process);
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    Err("adminStartupFailed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_keeps_paths_literal_and_elevates_only_the_helper() {
        let desktop = Path::new(r"C:\Apps & Tools\ConvenientWindow.exe");
        let helper = Path::new(r"C:\Apps & Tools\helper\magic-corners-helper.exe");
        let data = Path::new(r"C:\Users\测试\Data");
        let definition = task_xml("S-1-5-21-123", desktop, helper, data);
        assert!(definition
            .contains("<Command>C:\\Apps &amp; Tools\\helper\\magic-corners-helper.exe</Command>"));
        assert!(definition.contains("--scheduled-owner &quot;$(Arg0)&quot;"));
        assert!(definition.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(!definition.contains("<LogonTrigger"));
        assert!(definition.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
    }

    #[test]
    fn windows_accepts_the_task_definition_without_registering_it() {
        let _apartment = Apartment::new().unwrap();
        let sid = ProcessHandle::open(std::process::id())
            .unwrap()
            .user_sid()
            .unwrap();
        let definition = task_xml(
            &sid,
            Path::new(r"C:\Apps\ConvenientWindow.exe"),
            Path::new(r"C:\Apps\helper\magic-corners-helper.exe"),
            Path::new(r"C:\Data"),
        );
        unsafe {
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).unwrap();
            let task = service.NewTask(0).unwrap();
            task.SetXmlText(&BSTR::from(definition)).unwrap();
        }
    }

    #[test]
    fn uninstall_uses_the_recorded_executable_with_windows_argument_quoting() {
        let desktop = Path::new(r"C:\测试 & Apps\ConvenientWindow.exe");
        let data = Path::new("C:\\Data with spaces\\");
        assert_eq!(
            registered_desktop(&arguments(desktop, data)).unwrap(),
            desktop
        );
        assert!(registered_desktop("--desktop-executable C:\\Other.exe").is_err());
        assert!(registered_desktop(&format!("{} --extra", arguments(desktop, data))).is_err());
    }
}
