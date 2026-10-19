//! 布局复位的全局热键注册表。
//!
//! uTools 不向插件开放注册任意全局热键的接口（`utools.registerGlobalShortcut` 并不存在），
//! 因此两套宿主统一把布局快捷键同步给 helper：helper 在输入线程上已经安装常驻的
//! `WH_KEYBOARD_LL` 钩子，命中注册表时吞掉按键并回推 `shortcut.hotkey.triggered`，
//! 前端再执行 `window.layout.restore`。
//!
//! 注册表按连接归属：后同步的连接覆盖前一个，连接断开时清理，避免快捷键残留在系统里。

use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

const MOD_CTRL: u8 = 1;
const MOD_ALT: u8 = 2;
const MOD_SHIFT: u8 = 4;
const MOD_WIN: u8 = 8;

/// 事件邮箱上限：热键触发是低频用户操作，只保留最近若干条避免内存持续增长。
const MAX_EVENTS: usize = 8;
/// 单连接快捷键数量上限，比布局快照的 24 条上限留出余量。
const MAX_HOTKEYS: usize = 64;

#[derive(Clone, Debug)]
struct Hotkey {
    id: String,
    accelerator: String,
    modifiers: u8,
    vk: u32,
}

/// 发给前端的触发事件。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggeredHotkey {
    pub hotkey_id: String,
    pub accelerator: String,
}

/// 未能注册的条目；前端据此在布局卡片上显示告警。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedHotkey {
    pub id: String,
    pub reason: String,
}

#[derive(Debug)]
struct State {
    owner: Option<String>,
    hotkeys: Vec<Hotkey>,
    events: VecDeque<TriggeredHotkey>,
    /// 由钩子维护的当前修饰键状态，只跟踪注册表生命周期内发生的按键。
    held_modifiers: u8,
    /// 被热键吞掉的按键，释放事件要配对吞掉，避免前台应用收到孤立 keyup。
    swallowed: [bool; 256],
}

impl Default for State {
    fn default() -> Self {
        Self {
            owner: None,
            hotkeys: Vec::new(),
            events: VecDeque::new(),
            held_modifiers: 0,
            swallowed: [false; 256],
        }
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

/// 是否支持全局热键；只有 Windows 输入线程有常驻键盘钩子。
pub fn supported() -> bool {
    cfg!(target_os = "windows")
}

/// 解析 accelerator：修饰键固定 Ctrl/Alt/Shift/Super（大小写不敏感），
/// 主键只接受 A-Z、0-9、F1-F12，且至少包含一个修饰键。
fn parse_accelerator(value: &str) -> Result<(u8, u32)> {
    let mut modifiers = 0u8;
    let mut main_key: Option<u32> = None;
    for part in value.split('+').map(str::trim) {
        if part.is_empty() {
            bail!("empty accelerator segment");
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                if modifiers & MOD_CTRL != 0 {
                    bail!("duplicate modifier");
                }
                modifiers |= MOD_CTRL;
                continue;
            }
            "alt" => {
                if modifiers & MOD_ALT != 0 {
                    bail!("duplicate modifier");
                }
                modifiers |= MOD_ALT;
                continue;
            }
            "shift" => {
                if modifiers & MOD_SHIFT != 0 {
                    bail!("duplicate modifier");
                }
                modifiers |= MOD_SHIFT;
                continue;
            }
            "win" | "super" | "meta" => {
                if modifiers & MOD_WIN != 0 {
                    bail!("duplicate modifier");
                }
                modifiers |= MOD_WIN;
                continue;
            }
            _ => {}
        }
        if main_key.is_some() {
            bail!("accelerator has more than one main key");
        }
        main_key = Some(parse_main_key(part)?);
    }
    let Some(main_key) = main_key else {
        bail!("accelerator has no main key");
    };
    if modifiers == 0 {
        bail!("accelerator needs at least one modifier");
    }
    Ok((modifiers, main_key))
}

fn parse_main_key(part: &str) -> Result<u32> {
    let upper = part.to_ascii_uppercase();
    if upper.len() == 1 {
        let byte = upper.as_bytes()[0];
        if byte.is_ascii_alphanumeric() {
            return Ok(u32::from(byte));
        }
        bail!("unsupported main key");
    }
    if let Some(number) = upper
        .strip_prefix('F')
        .and_then(|digits| digits.parse::<u32>().ok())
    {
        if (1..=12).contains(&number) {
            return Ok(0x70 + number - 1);
        }
    }
    bail!("unsupported main key")
}

/// 用最新列表替换 owner 的注册表；返回被拒绝的条目。空列表等于注销全部。
pub fn sync(owner: &str, hotkeys: &[(String, String)]) -> Vec<RejectedHotkey> {
    let Ok(mut state) = state().lock() else {
        return hotkeys
            .iter()
            .map(|(id, _)| RejectedHotkey {
                id: id.clone(),
                reason: "hotkey state unavailable".into(),
            })
            .collect();
    };
    sync_locked(&mut state, owner, hotkeys)
}

fn sync_locked(
    state: &mut State,
    owner: &str,
    hotkeys: &[(String, String)],
) -> Vec<RejectedHotkey> {
    let mut rejected = Vec::new();
    let mut parsed = Vec::new();
    let mut seen: Vec<(u8, u32)> = Vec::new();
    for (index, (id, accelerator)) in hotkeys.iter().enumerate() {
        if index >= MAX_HOTKEYS {
            rejected.push(RejectedHotkey {
                id: id.clone(),
                reason: "too many hotkeys".into(),
            });
            continue;
        }
        match parse_accelerator(accelerator) {
            Ok((modifiers, vk)) => {
                if seen.contains(&(modifiers, vk)) {
                    rejected.push(RejectedHotkey {
                        id: id.clone(),
                        reason: "duplicate accelerator".into(),
                    });
                    continue;
                }
                seen.push((modifiers, vk));
                parsed.push(Hotkey {
                    id: id.clone(),
                    accelerator: accelerator.clone(),
                    modifiers,
                    vk,
                });
            }
            Err(error) => rejected.push(RejectedHotkey {
                id: id.clone(),
                reason: format!("{error:#}"),
            }),
        }
    }
    state.owner = Some(owner.to_owned());
    state.hotkeys = parsed;
    state.events.clear();
    // 旧连接按下未释放的修饰键状态不延续到新注册表。
    state.held_modifiers = 0;
    rejected
}

/// 清理 owner 的注册表；owner 为 None 时清理全部。
pub fn clear(owner: Option<&str>) {
    if let Ok(mut state) = state().lock() {
        if owner.is_none_or(|owner| state.owner.as_deref() == Some(owner)) {
            state.owner = None;
            state.hotkeys.clear();
            state.events.clear();
            state.held_modifiers = 0;
        }
    }
}

pub fn has_work(owner: &str) -> bool {
    state()
        .lock()
        .is_ok_and(|state| state.owner.as_deref() == Some(owner) && !state.events.is_empty())
}

pub fn take_event(owner: &str) -> Option<TriggeredHotkey> {
    let mut state = state().lock().ok()?;
    if state.owner.as_deref() != Some(owner) {
        return None;
    }
    state.events.pop_front()
}

/// 键盘钩子回调入口；返回 true 表示吞掉这次按键。
pub fn handle_key(vk: u32, down: bool) -> bool {
    if !supported() {
        return false;
    }
    let Ok(mut state) = state().lock() else {
        return false;
    };
    if state.hotkeys.is_empty() && !state.swallowed.iter().any(|held| *held) {
        return false;
    }
    handle_key_locked(&mut state, vk, down)
}

fn handle_key_locked(state: &mut State, vk: u32, down: bool) -> bool {
    let Ok(index) = usize::try_from(vk) else {
        return false;
    };
    if index >= state.swallowed.len() {
        return false;
    }
    if let Some(bit) = modifier_bit(vk) {
        if down {
            state.held_modifiers |= bit;
        } else {
            state.held_modifiers &= !bit;
        }
        return false;
    }
    if down {
        if state.swallowed[index] {
            // 自动重复：第一次按下已触发，重复的 down 只吞不再上报。
            return true;
        }
        let matched = state
            .hotkeys
            .iter()
            .find(|hotkey| hotkey.vk == vk && hotkey.modifiers == state.held_modifiers)
            .map(|hotkey| (hotkey.id.clone(), hotkey.accelerator.clone()));
        let Some((hotkey_id, accelerator)) = matched else {
            return false;
        };
        state.swallowed[index] = true;
        if state.events.len() == MAX_EVENTS {
            state.events.pop_front();
        }
        state.events.push_back(TriggeredHotkey {
            hotkey_id,
            accelerator,
        });
        true
    } else if state.swallowed[index] {
        state.swallowed[index] = false;
        true
    } else {
        false
    }
}

fn modifier_bit(vk: u32) -> Option<u8> {
    Some(match vk {
        0x10 | 0xa0 | 0xa1 => MOD_SHIFT,
        0x11 | 0xa2 | 0xa3 => MOD_CTRL,
        0x12 | 0xa4 | 0xa5 => MOD_ALT,
        0x5b | 0x5c => MOD_WIN,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(id, accelerator)| ((*id).to_owned(), (*accelerator).to_owned()))
            .collect()
    }

    fn registered(accelerator: &str) -> State {
        let mut state = State::default();
        let rejected = sync_locked(
            &mut state,
            "connection",
            &entries(&[("layout-1", accelerator)]),
        );
        assert!(rejected.is_empty(), "unexpected rejections: {rejected:?}");
        state
    }

    #[test]
    fn accelerators_parse_modifiers_and_main_keys() {
        assert_eq!(
            parse_accelerator("Ctrl+Alt+1").unwrap(),
            (MOD_CTRL | MOD_ALT, 0x31)
        );
        assert_eq!(
            parse_accelerator("Alt+Shift+F5").unwrap(),
            (MOD_ALT | MOD_SHIFT, 0x74)
        );
        // 存储格式用 Win，宿主 accelerator 用 Super，两者都要接受。
        assert_eq!(
            parse_accelerator("Super+K").unwrap(),
            (MOD_WIN, u32::from(b'K'))
        );
        assert_eq!(
            parse_accelerator("win+ctrl+a").unwrap(),
            (MOD_CTRL | MOD_WIN, u32::from(b'A'))
        );
    }

    #[test]
    fn invalid_accelerators_are_rejected_with_a_reason() {
        for value in [
            "",
            "Ctrl",
            "A",
            "Ctrl+Ctrl+A",
            "Ctrl+A+B",
            "Ctrl+Space",
            "Ctrl+F13",
            "Ctrl+;",
            "Ctrl++A",
        ] {
            assert!(
                parse_accelerator(value).is_err(),
                "{value} should be rejected"
            );
        }
        let rejected = sync_locked(
            &mut State::default(),
            "connection",
            &entries(&[("ok", "Ctrl+Alt+1"), ("bad", "A")]),
        );
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].id, "bad");
        assert!(!rejected[0].reason.is_empty());
    }

    #[test]
    fn modifier_plus_main_key_triggers_once_and_swallows_the_pair() {
        let mut state = registered("Ctrl+Alt+1");
        let ctrl = 0x11;
        let alt = 0x12;
        let one = 0x31;
        assert!(!handle_key_locked(&mut state, ctrl, true));
        assert!(!handle_key_locked(&mut state, alt, true));
        assert!(handle_key_locked(&mut state, one, true));
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].hotkey_id, "layout-1");
        // 自动重复只吞不重复上报。
        assert!(handle_key_locked(&mut state, one, true));
        assert_eq!(state.events.len(), 1);
        assert!(handle_key_locked(&mut state, one, false));
        assert!(!handle_key_locked(&mut state, alt, false));
        assert!(!handle_key_locked(&mut state, ctrl, false));
        // 释放事件配对完成，后续按键不再被吞。
        assert!(!handle_key_locked(&mut state, one, true));
    }

    #[test]
    fn incomplete_modifier_combination_is_not_swallowed() {
        let mut state = registered("Ctrl+Alt+1");
        assert!(!handle_key_locked(&mut state, 0x11, true));
        // 只按 Ctrl + 1，缺少 Alt：不触发也不吞键。
        assert!(!handle_key_locked(&mut state, 0x31, true));
        assert!(state.events.is_empty());
        assert!(!handle_key_locked(&mut state, 0x31, false));
    }

    #[test]
    fn resync_and_clear_drop_registrations() {
        let mut state = registered("Ctrl+Alt+1");
        assert!(!handle_key_locked(&mut state, 0x11, true));
        assert!(!handle_key_locked(&mut state, 0x12, true));
        let rejected = sync_locked(&mut state, "connection", &entries(&[]));
        assert!(rejected.is_empty());
        assert!(!handle_key_locked(&mut state, 0x31, true));
        assert!(state.events.is_empty());
    }

    #[test]
    fn event_mailbox_stays_bounded() {
        let mut state = registered("Ctrl+Alt+1");
        for _ in 0..MAX_EVENTS * 3 {
            assert!(!handle_key_locked(&mut state, 0x11, true));
            assert!(!handle_key_locked(&mut state, 0x12, true));
            assert!(handle_key_locked(&mut state, 0x31, true));
            assert!(handle_key_locked(&mut state, 0x31, false));
            assert!(!handle_key_locked(&mut state, 0x12, false));
            assert!(!handle_key_locked(&mut state, 0x11, false));
        }
        assert_eq!(state.events.len(), MAX_EVENTS);
    }

    #[test]
    fn triggered_event_serializes_with_camel_case_for_the_frontend() {
        let value = serde_json::to_value(TriggeredHotkey {
            hotkey_id: "layout-1".into(),
            accelerator: "Ctrl+Alt+1".into(),
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"hotkeyId": "layout-1", "accelerator": "Ctrl+Alt+1"})
        );
        let rejected = serde_json::to_value(RejectedHotkey {
            id: "layout-2".into(),
            reason: "unsupported main key".into(),
        })
        .unwrap();
        assert_eq!(
            rejected,
            serde_json::json!({"id": "layout-2", "reason": "unsupported main key"})
        );
    }
}
