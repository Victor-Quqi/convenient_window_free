//! A short, connection-owned recording session. Key releases remain paired after cancellation.
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEvent {
    pub session_id: String,
    pub phase: &'static str,
    pub value: Option<String>,
    pub hint: Option<String>,
    pub reason: Option<&'static str>,
}

struct Session {
    connection: String,
    id: String,
    foreground: isize,
    deadline: Instant,
    keys: [bool; 256],
}

struct RecordingState {
    session: Option<Session>,
    swallowed: [bool; 256],
    events: VecDeque<(String, RecordingEvent)>,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            session: None,
            swallowed: [false; 256],
            events: VecDeque::new(),
        }
    }
}

impl RecordingState {
    fn event(
        &mut self,
        phase: &'static str,
        value: Option<String>,
        hint: Option<String>,
        reason: Option<&'static str>,
    ) {
        let Some(session) = &self.session else { return };
        // Progress is replaceable; terminal events are retained within a fixed mailbox.
        if phase == "progress" {
            self.events.retain(|(_, event)| event.phase != "progress");
        }
        if self.events.len() == 8 {
            self.events.pop_front();
        }
        self.events.push_back((
            session.connection.clone(),
            RecordingEvent {
                session_id: session.id.clone(),
                phase,
                value,
                hint,
                reason,
            },
        ));
    }

    fn finish(&mut self, reason: &'static str) {
        self.event("cancelled", None, None, Some(reason));
        self.session = None;
    }

    fn key(&mut self, vk: usize, down: bool, foreground: isize, now: Instant) -> bool {
        if vk >= 256 {
            return false;
        }
        if let Some(session) = &self.session {
            if session.foreground != foreground {
                self.finish("focus-lost");
            } else if now >= session.deadline {
                self.finish("timeout");
            }
        }
        if self.swallowed[vk] {
            if !down {
                self.swallowed[vk] = false;
            }
            // Auto-repeat must not start a new recording or leak the held main key.
            if self.session.is_none() || down {
                return true;
            }
        } else if !down {
            if let Some(session) = &mut self.session {
                session.keys[vk] = false;
                if modifier(vk) {
                    clear_modifier_aliases(&mut session.keys, vk);
                }
            }
            return false; // This DOWN preceded recording and was delivered to the OS.
        }
        let Some(session) = &mut self.session else {
            return false;
        };
        session.keys[vk] = down;
        clear_modifier_aliases(&mut session.keys, vk);
        if down {
            self.swallowed[vk] = true;
        }
        if !down {
            return true;
        }
        if vk == 0x1b {
            self.finish("escape");
            return true;
        }
        let modifiers = modifier_names(&session.keys);
        if modifier(vk) {
            self.event("progress", None, Some(modifiers.join("+")), None);
        } else if let Some(key) = key_name(vk) {
            let value = modifiers
                .into_iter()
                .chain([key])
                .collect::<Vec<_>>()
                .join("+");
            self.event("committed", Some(value), None, None);
            self.session = None;
        } else {
            self.event(
                "progress",
                None,
                Some(modifiers.join("+")),
                Some("unsupported-key"),
            );
        }
        true
    }
}

fn modifier(vk: usize) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5b | 0x5c | 0xa0..=0xa5)
}
fn clear_modifier_aliases(keys: &mut [bool; 256], vk: usize) {
    match vk {
        0xa0 | 0xa1 => keys[0x10] = false,
        0xa2 | 0xa3 => keys[0x11] = false,
        0xa4 | 0xa5 => keys[0x12] = false,
        _ => {}
    }
}
fn modifier_names(keys: &[bool; 256]) -> Vec<String> {
    [
        (0x11, 0xa2, 0xa3, "Ctrl"),
        (0x12, 0xa4, 0xa5, "Alt"),
        (0x10, 0xa0, 0xa1, "Shift"),
        (0x5b, 0x5b, 0x5c, "Win"),
    ]
    .into_iter()
    .filter(|(a, b, c, _)| keys[*a] || keys[*b] || keys[*c])
    .map(|(_, _, _, name)| name.to_owned())
    .collect()
}
fn key_name(vk: usize) -> Option<String> {
    Some(match vk {
        0x30..=0x39 | 0x41..=0x5a => (vk as u8 as char).to_string(),
        0x70..=0x7b => format!("F{}", vk - 0x6f),
        0x0d => "Enter".into(),
        0x09 => "Tab".into(),
        0x20 => "Space".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        _ => return None,
    })
}
fn state() -> &'static Mutex<RecordingState> {
    static STATE: OnceLock<Mutex<RecordingState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RecordingState::default()))
}
pub fn supported() -> bool {
    cfg!(target_os = "windows")
}

pub fn begin(connection: &str, id: &str) -> Result<()> {
    if !supported() {
        bail!("shortcut recording is unsupported on this platform");
    }
    uuid::Uuid::parse_str(id)?;
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        if !super::mouse_hook_is_healthy() {
            bail!("input hook is not ready");
        }
        let foreground = unsafe { GetForegroundWindow() }.0 as isize;
        if foreground == 0 {
            bail!("recording requires a foreground editor");
        }
        let mut state = state()
            .lock()
            .map_err(|_| anyhow::anyhow!("recording state unavailable"))?;
        if state.session.is_some() {
            bail!("another shortcut recording is active");
        }
        // A previous session's consumed keys must be released before starting another one.
        if state.swallowed.iter().any(|held| *held) {
            bail!("release the previous recording keys first");
        }
        let mut keys = std::array::from_fn(|vk| unsafe { GetAsyncKeyState(vk as i32) < 0 });
        // The hook reports left/right modifier VKs; generic aliases are snapshots,
        // not independent held keys and must not outlive a physical release.
        keys[0x10] = false;
        keys[0x11] = false;
        keys[0x12] = false;
        if (1..=6).any(|vk| keys[vk]) {
            bail!("release mouse buttons before recording");
        }
        if (0x08..256).any(|vk| keys[vk] && !modifier(vk)) {
            bail!("release held non-modifier keys before recording");
        }
        state.events.clear();
        state.session = Some(Session {
            connection: connection.to_owned(),
            id: id.to_owned(),
            foreground,
            deadline: Instant::now() + Duration::from_secs(15),
            keys,
        });
        state.event("started", None, None, None);
    }
    Ok(())
}

pub fn cancel(connection: &str, id: Option<&str>) {
    if let Ok(mut state) = state().lock() {
        if state
            .session
            .as_ref()
            .is_some_and(|s| s.connection == connection && id.is_none_or(|id| s.id == id))
        {
            state.finish("cancelled");
        }
    }
}
pub fn cancel_all() {
    if let Ok(mut state) = state().lock() {
        state.finish("stopped");
    }
}
pub fn active() -> bool {
    state()
        .lock()
        .is_ok_and(|s| s.session.is_some() || s.swallowed.iter().any(|x| *x))
}
pub fn has_work(connection: &str) -> bool {
    state().lock().is_ok_and(|s| {
        s.session
            .as_ref()
            .is_some_and(|session| session.connection == connection)
            || s.events.iter().any(|(owner, _)| owner == connection)
    })
}
pub fn take_event(connection: &str) -> Option<RecordingEvent> {
    let mut state = state().lock().ok()?;
    #[cfg(target_os = "windows")]
    if let Some(session) = &state.session {
        let foreground =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
        if foreground != session.foreground {
            state.finish("focus-lost");
        } else if Instant::now() >= session.deadline {
            state.finish("timeout");
        }
    }
    let index = state
        .events
        .iter()
        .position(|(owner, _)| owner == connection)?;
    state.events.remove(index).map(|(_, event)| event)
}
#[cfg(target_os = "windows")]
pub unsafe extern "system" fn keyboard_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code >= 0 {
        let message = wparam.0 as u32;
        if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) {
            let hook = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if hook.flags.contains(LLKHF_INJECTED)
                && hook.dwExtraInfo == super::windows::keyboard::KEYBOARD_INJECT_TAG
            {
                return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
            }
            if let Ok(mut state) = state().lock() {
                if state.session.is_none() && !state.swallowed.iter().any(|held| *held) {
                    drop(state);
                    // 没有录制会话时键盘钩子服务于布局全局热键：命中注册表就吞掉
                    // 这次按键，由 websocket 把触发事件回推给宿主。
                    let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
                    if super::shortcut_hotkeys::handle_key(hook.vkCode as u32, down) {
                        return LRESULT(1);
                    }
                    return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
                }
                if state.key(
                    hook.vkCode as usize,
                    matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN),
                    GetForegroundWindow().0 as isize,
                    Instant::now(),
                ) {
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recording() -> RecordingState {
        let mut state = RecordingState::default();
        state.session = Some(Session {
            connection: "connection".into(),
            id: "session".into(),
            foreground: 7,
            deadline: Instant::now() + Duration::from_secs(15),
            keys: [false; 256],
        });
        state
    }
    #[test]
    fn system_and_host_shortcuts_are_isolated_and_releases_are_paired() {
        for (modifier, key, expected) in [
            (0x11, 0x57, "Ctrl+W"),
            (0x12, 0x09, "Alt+Tab"),
            (0x5b, 0x26, "Win+Up"),
            (0x12, 0x73, "Alt+F4"),
        ] {
            let mut s = recording();
            let now = Instant::now();
            assert!(s.key(modifier, true, 7, now));
            assert!(s.key(key, true, 7, now));
            assert_eq!(s.events.back().unwrap().1.value.as_deref(), Some(expected));
            assert!(s.key(key, false, 7, now));
            assert!(s.key(modifier, false, 7, now));
            assert!(!s.key(0x41, true, 7, now));
        }
    }
    #[test]
    fn focus_loss_and_escape_cancel_without_leaking_an_unmatched_release() {
        for lose_focus in [true, false] {
            let mut s = recording();
            let now = Instant::now();
            assert!(s.key(0x12, true, 7, now));
            if lose_focus {
                assert!(!s.key(0x41, true, 8, now));
            } else {
                assert!(s.key(0x1b, true, 7, now));
                assert!(s.key(0x1b, false, 7, now));
            }
            assert!(s.session.is_none());
            assert!(s.key(0x12, false, 8, now));
            assert!(!s.key(0x41, false, 8, now));
        }
    }
    #[test]
    fn unsupported_keys_and_timeout_never_commit_a_partial_combination() {
        let mut s = recording();
        let now = Instant::now();
        assert!(s.key(0x11, true, 7, now));
        assert!(s.key(0x08, true, 7, now));
        assert!(s.key(0x08, false, 7, now));
        assert!(s.session.is_some());
        assert!(!s.key(0x41, true, 7, now + Duration::from_secs(16)));
        assert!(!s.events.iter().any(|(_, e)| e.phase == "committed"));
        assert!(s.key(0x11, false, 7, now + Duration::from_secs(16)));
    }
    #[test]
    fn preheld_modifier_release_is_passed_to_the_os() {
        let mut s = recording();
        s.session.as_mut().unwrap().keys[0x11] = true;
        assert!(!s.key(0x11, false, 7, Instant::now()));
        assert!(s.key(0x41, true, 7, Instant::now()));
        assert_eq!(s.events.back().unwrap().1.value.as_deref(), Some("A"));
    }

    #[test]
    fn releasing_a_preheld_left_modifier_clears_its_generic_alias() {
        let mut s = recording();
        s.session.as_mut().unwrap().keys[0x11] = true;
        s.session.as_mut().unwrap().keys[0xa2] = true;
        assert!(!s.key(0xa2, false, 7, Instant::now()));
        assert!(s.key(0x41, true, 7, Instant::now()));
        assert_eq!(s.events.back().unwrap().1.value.as_deref(), Some("A"));
    }

    #[test]
    fn releasing_one_side_keeps_the_other_held_modifier() {
        let mut s = recording();
        s.session.as_mut().unwrap().keys[0xa2] = true;
        s.session.as_mut().unwrap().keys[0xa3] = true;
        assert!(!s.key(0xa2, false, 7, Instant::now()));
        assert!(s.key(0x41, true, 7, Instant::now()));
        assert_eq!(s.events.back().unwrap().1.value.as_deref(), Some("Ctrl+A"));
    }

    #[test]
    fn cancellation_keeps_consumed_auto_repeat_and_release_paired() {
        let mut s = recording();
        let now = Instant::now();
        assert!(s.key(0xa4, true, 7, now));
        s.finish("cancelled");
        assert!(s.key(0xa4, true, 7, now));
        assert!(s.key(0xa4, false, 7, now));
        assert!(!s.key(0x41, true, 7, now));
        assert!(s.swallowed.iter().all(|held| !held));
    }

    #[test]
    fn progress_mailbox_stays_bounded_under_repeated_modifiers() {
        let mut s = recording();
        let now = Instant::now();
        for _ in 0..10000 {
            assert!(s.key(0xa2, true, 7, now));
            assert!(s.key(0xa2, false, 7, now));
            assert!(s.events.len() <= 8);
        }
        assert!(s.key(0x57, true, 7, now));
        assert_eq!(s.events.back().unwrap().1.value.as_deref(), Some("W"));
    }
}
