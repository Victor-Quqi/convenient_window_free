//! Process handles used across the desktop/helper privilege boundary.
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, FILETIME, HANDLE, HLOCAL, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenSessionId, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessId, GetProcessTimes, OpenProcess,
    OpenProcessToken, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

pub struct ProcessHandle(isize);

impl ProcessHandle {
    pub fn new(handle: HANDLE) -> Self {
        Self(handle.0 as isize)
    }

    pub fn raw(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }

    pub fn open(pid: u32) -> Result<Self, String> {
        unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .map(Self::new)
        .map_err(|error| error.to_string())
    }

    pub fn id(&self) -> u32 {
        unsafe { GetProcessId(self.raw()) }
    }

    pub fn birth(&self) -> Result<u64, String> {
        process_birth(self.raw())
    }

    pub fn elevated(&self) -> Result<bool, String> {
        is_elevated(self.raw())
    }

    pub fn image(&self) -> Result<std::path::PathBuf, String> {
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                self.raw(),
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .map_err(|error| error.to_string())?;
        Ok(std::path::PathBuf::from(String::from_utf16_lossy(
            &buffer[..length as usize],
        )))
    }

    pub fn user_sid(&self) -> Result<String, String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(self.raw(), TOKEN_QUERY, &mut token)
                .map_err(|error| error.to_string())?;
            let token = Self::new(token);
            let mut length = 0;
            let _ = GetTokenInformation(token.raw(), TokenUser, None, 0, &mut length);
            let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
            GetTokenInformation(
                token.raw(),
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                length,
                &mut length,
            )
            .map_err(|error| error.to_string())?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut string = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut string)
                .map_err(|error| error.to_string())?;
            let result = string.to_string().map_err(|error| error.to_string());
            let _ = LocalFree(HLOCAL(string.0.cast()));
            result
        }
    }

    pub fn session(&self) -> Result<u32, String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(self.raw(), TOKEN_QUERY, &mut token)
                .map_err(|error| error.to_string())?;
            let token = Self::new(token);
            let mut session = 0u32;
            let mut length = 0;
            GetTokenInformation(
                token.raw(),
                TokenSessionId,
                Some((&mut session as *mut u32).cast()),
                4,
                &mut length,
            )
            .map_err(|error| error.to_string())?;
            Ok(session)
        }
    }

    pub fn try_wait(&self) -> Result<Option<i32>, String> {
        unsafe {
            match WaitForSingleObject(self.raw(), 0) {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    GetExitCodeProcess(self.raw(), &mut code).map_err(|error| error.to_string())?;
                    Ok(Some(code as i32))
                }
                _ => Err("Unable to wait for helper process".into()),
            }
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.raw());
        }
    }
}

pub fn current_elevated() -> Result<bool, String> {
    is_elevated(unsafe { GetCurrentProcess() })
}

pub fn current_birth() -> Result<u64, String> {
    process_birth(unsafe { GetCurrentProcess() })
}

pub fn same_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()),
        _ => false,
    }
}

pub fn verify_desktop(
    pid: u32,
    birth: u64,
    executable: &std::path::Path,
) -> Result<ProcessHandle, String> {
    let owner = ProcessHandle::open(pid)?;
    let current = ProcessHandle::open(std::process::id())?;
    if owner.birth()? != birth
        || owner.try_wait()?.is_some()
        || owner.elevated()?
        || owner.user_sid()? != current.user_sid()?
        || owner.session()? != current.session()?
        || !same_path(&owner.image()?, executable)
    {
        return Err("Desktop owner identity does not match".into());
    }
    Ok(owner)
}

fn is_elevated(process: HANDLE) -> Result<bool, String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(|error| error.to_string())?;
        let token = ProcessHandle::new(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut length = 0;
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
        .map_err(|error| error.to_string())?;
        Ok(elevation.TokenIsElevated != 0)
    }
}

fn process_birth(process: HANDLE) -> Result<u64, String> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
        .map_err(|error| error.to_string())?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

pub fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

pub fn ptr(value: &[u16]) -> PCWSTR {
    PCWSTR(value.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opened_process_keeps_identity_and_elevation() {
        let process = ProcessHandle::open(std::process::id()).unwrap();
        assert_eq!(process.id(), std::process::id());
        assert_eq!(process.birth().unwrap(), current_birth().unwrap());
        assert_eq!(process.elevated().unwrap(), current_elevated().unwrap());
        assert_eq!(process.try_wait().unwrap(), None);
        assert!(same_path(
            &process.image().unwrap(),
            &std::env::current_exe().unwrap()
        ));
        assert!(process.user_sid().unwrap().starts_with("S-1-5-"));
        let _ = process.session().unwrap();
        assert!(verify_desktop(
            std::process::id(),
            current_birth().unwrap() + 1,
            &std::env::current_exe().unwrap()
        )
        .is_err());
    }
}
