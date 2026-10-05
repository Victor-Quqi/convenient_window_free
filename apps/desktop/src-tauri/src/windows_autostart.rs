use std::path::Path;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

pub const STARTUP_NAME: &str = "Convenient Window";
const LEGACY_NAMES: [&str; 2] = ["便捷窗口", "convenient-window"];
const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";

fn is_our_command(command: &str, executable: &Path) -> bool {
    let command = command.trim();
    let path = if let Some(quoted) = command.strip_prefix('"') {
        let Some((path, args)) = quoted.split_once('"') else {
            return false;
        };
        if args.trim() != "--autostart" {
            return false;
        }
        path
    } else {
        let Some(path) = command.strip_suffix(" --autostart") else {
            return false;
        };
        path
    };
    // Upgrades retain the install directory. A different path belongs to another copy.
    Path::new(path).is_absolute() && path.eq_ignore_ascii_case(&executable.to_string_lossy())
}

/// Write the new entry and its OS override before removing the old names.
/// A pre-existing new entry wins, including a Task Manager disabled state.
fn migrate(run: &RegKey, approved: Option<&RegKey>, executable: &Path) -> std::io::Result<()> {
    for name in LEGACY_NAMES {
        let old: String = match run.get_value(name) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !is_our_command(&old, executable) {
            continue;
        }
        match run.get_value::<String, _>(STARTUP_NAME) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(key) = approved {
                    match key.get_raw_value(name) {
                        Ok(value) => key.set_raw_value(STARTUP_NAME, &value)?,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                run.set_value(
                    STARTUP_NAME,
                    &format!("\"{}\" --autostart", executable.display()),
                )?;
            }
            Ok(command) if is_our_command(&command, executable) => {}
            Ok(_) => continue,
            Err(error) => return Err(error),
        }
        run.delete_value(name)?;
        if let Some(key) = approved {
            match key.delete_value(name) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

pub fn migrate_current_user() -> std::io::Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let run = match user.open_subkey_with_flags(RUN, KEY_READ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let executable = std::env::current_exe()?;
    if !LEGACY_NAMES.iter().any(|name| {
        run.get_value::<String, _>(name)
            .is_ok_and(|command| is_our_command(&command, &executable))
    }) {
        return Ok(());
    }
    let run = user.open_subkey_with_flags(RUN, KEY_READ | KEY_WRITE)?;
    let approved = match user.open_subkey_with_flags(APPROVED, KEY_READ | KEY_WRITE) {
        Ok(key) => Some(key),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    migrate(&run, approved.as_ref(), &executable)
}

/// auto-launch 0.5 omits path quotes. Write the complete command in one operation.
pub fn enable() -> std::io::Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = user.create_subkey(RUN)?;
    run.set_value(
        STARTUP_NAME,
        &format!("\"{}\" --autostart", std::env::current_exe()?.display()),
    )?;
    match user.open_subkey_with_flags(APPROVED, KEY_WRITE) {
        Ok(approved) => match approved.delete_value(STARTUP_NAME) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn enabled_for_current_executable() -> std::io::Result<bool> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let run = match user.open_subkey(RUN) {
        Ok(run) => run,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let command: String = match run.get_value(STARTUP_NAME) {
        Ok(command) => command,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !is_our_command(&command, &std::env::current_exe()?) {
        return Ok(false);
    }
    match user
        .open_subkey(APPROVED)
        .and_then(|key| key.get_raw_value(STARTUP_NAME))
    {
        Ok(value) => Ok(!matches!(value.bytes.first(), Some(3 | 7))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::{enums::RegType::REG_BINARY, RegValue};

    #[test]
    fn migration_preserves_disabled_state_is_idempotent_and_respects_new_entry() {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!("Software\\ConvenientWindowTests-{}", uuid::Uuid::new_v4());
        let (root, _) = user.create_subkey(&path).unwrap();
        let (run, _) = root.create_subkey("Run").unwrap();
        let (approved, _) = root.create_subkey("Approved").unwrap();
        let result = std::panic::catch_unwind(|| {
            let executable = Path::new("C:\\Apps\\Convenient Window\\convenient-window.exe");
            let disabled = RegValue {
                bytes: vec![3, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0],
                vtype: REG_BINARY,
            };
            run.set_value(
                LEGACY_NAMES[0],
                &format!("{} --autostart", executable.display()),
            )
            .unwrap();
            approved.set_raw_value(LEGACY_NAMES[0], &disabled).unwrap();
            let read_only_run = root.open_subkey_with_flags("Run", KEY_READ).unwrap();
            assert!(migrate(&read_only_run, Some(&approved), executable).is_err());
            assert!(run.get_raw_value(LEGACY_NAMES[0]).is_ok());
            assert!(run.get_raw_value(STARTUP_NAME).is_err());
            drop(read_only_run);
            migrate(&run, Some(&approved), executable).unwrap();
            assert_eq!(
                approved.get_raw_value(STARTUP_NAME).unwrap().bytes,
                disabled.bytes
            );
            let command: String = run.get_value(STARTUP_NAME).unwrap();
            assert_eq!(
                command,
                "\"C:\\Apps\\Convenient Window\\convenient-window.exe\" --autostart"
            );
            assert!(run.get_raw_value(LEGACY_NAMES[0]).is_err());
            migrate(&run, Some(&approved), executable).unwrap();
            run.set_value(
                LEGACY_NAMES[1],
                &format!("\"{}\" --autostart", executable.display()),
            )
            .unwrap();
            migrate(&run, Some(&approved), executable).unwrap();
            assert_eq!(run.get_value::<String, _>(STARTUP_NAME).unwrap(), command);
            assert!(run.get_raw_value(LEGACY_NAMES[1]).is_err());
            assert_eq!(
                approved.get_raw_value(STARTUP_NAME).unwrap().bytes,
                disabled.bytes
            );
            let foreign = "C:\\Other\\convenient-window.exe --autostart";
            run.set_value(LEGACY_NAMES[0], &foreign).unwrap();
            migrate(&run, Some(&approved), executable).unwrap();
            assert_eq!(
                run.get_value::<String, _>(LEGACY_NAMES[0]).unwrap(),
                foreign
            );
            run.set_value(LEGACY_NAMES[0], &command).unwrap();
            run.set_value(STARTUP_NAME, &foreign).unwrap();
            migrate(&run, Some(&approved), executable).unwrap();
            assert_eq!(
                run.get_value::<String, _>(LEGACY_NAMES[0]).unwrap(),
                command
            );
            assert_eq!(run.get_value::<String, _>(STARTUP_NAME).unwrap(), foreign);
        });
        drop(run);
        drop(approved);
        drop(root);
        user.delete_subkey_all(&path).unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }

    #[test]
    fn unrelated_or_malformed_startup_commands_are_preserved() {
        let exe = Path::new("C:\\Apps\\convenient-window.exe");
        assert!(!is_our_command("C:\\other.exe --autostart", exe));
        assert!(!is_our_command(
            "C:\\Other\\convenient-window.exe --autostart",
            exe
        ));
        assert!(!is_our_command("convenient-window.exe --autostart", exe));
        assert!(is_our_command(
            "\"c:\\apps\\convenient-window.exe\" --autostart",
            exe
        ));
        assert!(!is_our_command(
            "\"C:\\convenient-window.exe --autostart",
            exe
        ));
        assert!(!is_our_command("C:\\convenient-window.exe --other", exe));
    }
}
