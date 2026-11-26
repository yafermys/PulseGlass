use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

pub const DEFAULT_SUMMARY_HOTKEY: &str = "Ctrl+Enter";
pub const DEFAULT_GHOST_HOTKEY: &str = "Shift+Enter";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleSummary,
    ToggleGhost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    pub key: String,
}

impl HotkeySpec {
    /// Parses ANY single key or modifier+key combination (e.g. "Insert", "F9", "A", "Ctrl+Alt+S", "Win+Shift+G").
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut win = false;
        let mut key: Option<String> = None;

        for raw in s.split('+') {
            let tok = raw.trim();
            if tok.is_empty() {
                continue;
            }
            match tok.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "win" | "super" | "meta" | "cmd" => win = true,
                other => {
                    key = Some(normalize_key_name(other)?);
                }
            }
        }

        let k = key?;
        // Bare Escape is reserved to cancel capture; Escape with modifiers is allowed.
        if k == "Escape" && !ctrl && !shift && !alt && !win {
            return None;
        }
        Some(Self {
            ctrl,
            shift,
            alt,
            win,
            key: k,
        })
    }

    pub fn format(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(self.key.as_str());
        parts.join("+")
    }

    pub fn from_egui_key(modifiers: egui::Modifiers, key: egui::Key) -> Option<Self> {
        let key_name = egui_key_to_name(key)?;
        let ctrl = modifiers.ctrl;
        let shift = modifiers.shift;
        let alt = modifiers.alt;
        let win = modifiers.mac_cmd || (modifiers.command && !modifiers.ctrl);
        if key_name == "Escape" && !ctrl && !shift && !alt && !win {
            return None;
        }
        Some(Self {
            ctrl,
            shift,
            alt,
            win,
            key: key_name.to_string(),
        })
    }
}

fn normalize_key_name(lower: &str) -> Option<String> {
    let s = match lower {
        "enter" | "return" => "Enter",
        "space" | "spacebar" => "Space",
        "tab" => "Tab",
        "backspace" | "back" => "Backspace",
        "escape" | "esc" => "Escape",
        "insert" | "ins" => "Insert",
        "delete" | "del" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "minus" | "-" => "Minus",
        "equals" | "=" | "plus" => "Equals",
        "backtick" | "`" | "grave" => "Backtick",
        "openbracket" | "[" => "OpenBracket",
        "closebracket" | "]" => "CloseBracket",
        "backslash" | "\\" => "Backslash",
        "semicolon" | ";" => "Semicolon",
        "quote" | "'" => "Quote",
        "comma" | "," => "Comma",
        "period" | "." => "Period",
        "slash" | "/" => "Slash",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        _ => {
            if lower.len() == 1 {
                let ch = lower.chars().next()?;
                if ch.is_ascii_alphanumeric() {
                    return Some(ch.to_ascii_uppercase().to_string());
                }
            }
            return None;
        }
    };
    Some(s.to_string())
}

fn egui_key_to_name(key: egui::Key) -> Option<&'static str> {
    use egui::Key;
    Some(match key {
        Key::Enter => "Enter",
        Key::Space => "Space",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::Escape => "Escape",
        Key::Insert => "Insert",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::Minus => "Minus",
        Key::Equals | Key::Plus => "Equals",
        Key::Backtick => "Backtick",
        Key::OpenBracket => "OpenBracket",
        Key::CloseBracket => "CloseBracket",
        Key::Backslash => "Backslash",
        Key::Semicolon => "Semicolon",
        Key::Quote => "Quote",
        Key::Comma => "Comma",
        Key::Period => "Period",
        Key::Slash => "Slash",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        _ => return None,
    })
}

#[cfg(windows)]
fn key_to_win32_vk(key: &str) -> Option<u32> {
    Some(match key {
        "Enter" => 0x0D,
        "Space" => 0x20,
        "Tab" => 0x09,
        "Backspace" => 0x08,
        "Escape" => 0x1B,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "Minus" => 0xBD,
        "Equals" => 0xBB,
        "Backtick" => 0xC0,
        "OpenBracket" => 0xDB,
        "CloseBracket" => 0xDD,
        "Backslash" => 0xDC,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        other if other.len() == 1 => {
            let b = other.as_bytes()[0];
            if b.is_ascii_uppercase() || b.is_ascii_digit() {
                b as u32
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn key_to_linux_code(key: &str) -> Option<global_hotkey::hotkey::Code> {
    use global_hotkey::hotkey::Code;
    Some(match key {
        "Enter" => Code::Enter,
        "Space" => Code::Space,
        "Tab" => Code::Tab,
        "Backspace" => Code::Backspace,
        "Escape" => Code::Escape,
        "Insert" => Code::Insert,
        "Delete" => Code::Delete,
        "Home" => Code::Home,
        "End" => Code::End,
        "PageUp" => Code::PageUp,
        "PageDown" => Code::PageDown,
        "Up" => Code::ArrowUp,
        "Down" => Code::ArrowDown,
        "Left" => Code::ArrowLeft,
        "Right" => Code::ArrowRight,
        "Minus" => Code::Minus,
        "Equals" => Code::Equal,
        "Backtick" => Code::Backquote,
        "OpenBracket" => Code::BracketLeft,
        "CloseBracket" => Code::BracketRight,
        "Backslash" => Code::Backslash,
        "Semicolon" => Code::Semicolon,
        "Quote" => Code::Quote,
        "Comma" => Code::Comma,
        "Period" => Code::Period,
        "Slash" => Code::Slash,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        _ => return None,
    })
}

struct SharedBindings {
    summary: HotkeySpec,
    ghost: HotkeySpec,
    #[cfg(target_os = "linux")]
    dirty: bool,
    status_warning: String,
}

pub struct HotkeyManager {
    pub rx: Receiver<HotkeyAction>,
    shared: Arc<Mutex<SharedBindings>>,
    #[cfg(windows)]
    thread_id: Arc<std::sync::atomic::AtomicU32>,
}

impl HotkeyManager {
    pub fn start(ctx: egui::Context, summary_str: &str, ghost_str: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        let summary = HotkeySpec::parse(summary_str)
            .unwrap_or_else(|| HotkeySpec::parse(DEFAULT_SUMMARY_HOTKEY).unwrap());
        let ghost = HotkeySpec::parse(ghost_str)
            .unwrap_or_else(|| HotkeySpec::parse(DEFAULT_GHOST_HOTKEY).unwrap());

        let shared = Arc::new(Mutex::new(SharedBindings {
            summary,
            ghost,
            #[cfg(target_os = "linux")]
            dirty: false,
            status_warning: String::new(),
        }));

        #[cfg(windows)]
        let thread_id = Arc::new(std::sync::atomic::AtomicU32::new(0));

        #[cfg(windows)]
        {
            let shared_bg = Arc::clone(&shared);
            let tid_bg = Arc::clone(&thread_id);
            std::thread::Builder::new()
                .name("pulseglass-hotkeys".to_string())
                .spawn(move || unsafe {
                    use std::sync::atomic::Ordering;
                    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
                    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
                        MOD_SHIFT, MOD_WIN,
                    };
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetMessageW, PeekMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY,
                    };

                    const ID_GHOST: i32 = 9001;
                    const ID_SUMMARY: i32 = 9002;
                    const WM_APP_REBIND: u32 = WM_APP + 1;

                    let mut dummy: MSG = std::mem::zeroed();
                    PeekMessageW(&mut dummy, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
                    tid_bg.store(GetCurrentThreadId(), Ordering::SeqCst);

                    let register_both = |sh: &Arc<Mutex<SharedBindings>>| {
                        let _ = UnregisterHotKey(std::ptr::null_mut(), ID_GHOST);
                        let _ = UnregisterHotKey(std::ptr::null_mut(), ID_SUMMARY);
                        let (sum_spec, gh_spec) = match sh.lock() {
                            Ok(g) => (g.summary.clone(), g.ghost.clone()),
                            Err(_) => return,
                        };
                        let to_mods = |s: &HotkeySpec| -> u32 {
                            let mut m = MOD_NOREPEAT;
                            if s.ctrl {
                                m |= MOD_CONTROL;
                            }
                            if s.shift {
                                m |= MOD_SHIFT;
                            }
                            if s.alt {
                                m |= MOD_ALT;
                            }
                            if s.win {
                                m |= MOD_WIN;
                            }
                            m
                        };
                        let mut warnings = Vec::new();
                        if let Some(vk) = key_to_win32_vk(&sum_spec.key) {
                            if RegisterHotKey(
                                std::ptr::null_mut(),
                                ID_SUMMARY,
                                to_mods(&sum_spec),
                                vk,
                            ) == 0
                            {
                                warnings.push(format!("{} kullanımda", sum_spec.format()));
                            }
                        }
                        if let Some(vk) = key_to_win32_vk(&gh_spec.key) {
                            if RegisterHotKey(
                                std::ptr::null_mut(),
                                ID_GHOST,
                                to_mods(&gh_spec),
                                vk,
                            ) == 0
                            {
                                warnings.push(format!("{} kullanımda", gh_spec.format()));
                            }
                        }
                        if let Ok(mut g) = sh.lock() {
                            g.status_warning = warnings.join(" | ");
                        }
                    };

                    register_both(&shared_bg);

                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        if msg.message == WM_APP_REBIND {
                            register_both(&shared_bg);
                            ctx.request_repaint();
                        } else if msg.message == WM_HOTKEY {
                            let id = msg.wParam as i32;
                            let action = if id == ID_SUMMARY {
                                Some(HotkeyAction::ToggleSummary)
                            } else if id == ID_GHOST {
                                Some(HotkeyAction::ToggleGhost)
                            } else {
                                None
                            };
                            if let Some(a) = action {
                                if tx.send(a).is_err() {
                                    break;
                                }
                                ctx.request_repaint();
                            }
                        }
                    }

                    let _ = UnregisterHotKey(std::ptr::null_mut(), ID_GHOST);
                    let _ = UnregisterHotKey(std::ptr::null_mut(), ID_SUMMARY);
                })
                .ok();
        }

        #[cfg(target_os = "linux")]
        {
            let shared_bg = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("pulseglass-hotkeys".to_string())
                .spawn(move || {
                    use global_hotkey::hotkey::{HotKey, Modifiers};
                    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
                    use std::time::Duration;

                    let Ok(manager) = GlobalHotKeyManager::new() else {
                        return;
                    };

                    let make_hk = |s: &HotkeySpec| -> Option<HotKey> {
                        let code = key_to_linux_code(&s.key)?;
                        let mut mods = Modifiers::empty();
                        if s.ctrl {
                            mods |= Modifiers::CONTROL;
                        }
                        if s.shift {
                            mods |= Modifiers::SHIFT;
                        }
                        if s.alt {
                            mods |= Modifiers::ALT;
                        }
                        if s.win {
                            mods |= Modifiers::SUPER;
                        }
                        let m_opt = if mods.is_empty() { None } else { Some(mods) };
                        Some(HotKey::new(m_opt, code))
                    };

                    let mut cur_sum: Option<HotKey> = None;
                    let mut cur_gh: Option<HotKey> = None;

                    let mut apply_bindings = |sh: &Arc<Mutex<SharedBindings>>| {
                        if let Some(h) = cur_sum.take() {
                            let _ = manager.unregister(h);
                        }
                        if let Some(h) = cur_gh.take() {
                            let _ = manager.unregister(h);
                        }
                        if let Ok(mut g) = sh.lock() {
                            g.dirty = false;
                            if let Some(h) = make_hk(&g.summary) {
                                if manager.register(h).is_ok() {
                                    cur_sum = Some(h);
                                }
                            }
                            if let Some(h) = make_hk(&g.ghost) {
                                if manager.register(h).is_ok() {
                                    cur_gh = Some(h);
                                }
                            }
                        }
                    };

                    apply_bindings(&shared_bg);

                    let rx_ev = GlobalHotKeyEvent::receiver();
                    loop {
                        if let Ok(g) = shared_bg.lock() {
                            if g.dirty {
                                drop(g);
                                apply_bindings(&shared_bg);
                            }
                        }
                        if let Ok(ev) = rx_ev.recv_timeout(Duration::from_millis(120)) {
                            if ev.state != HotKeyState::Pressed {
                                continue;
                            }
                            let action = if cur_sum.map(|h| h.id()) == Some(ev.id) {
                                Some(HotkeyAction::ToggleSummary)
                            } else if cur_gh.map(|h| h.id()) == Some(ev.id) {
                                Some(HotkeyAction::ToggleGhost)
                            } else {
                                None
                            };
                            if let Some(a) = action {
                                if tx.send(a).is_err() {
                                    break;
                                }
                                ctx.request_repaint();
                            }
                        }
                    }
                })
                .ok();
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = (tx, ctx);
        }

        Self {
            rx,
            shared,
            #[cfg(windows)]
            thread_id,
        }
    }

    pub fn rebind(&self, summary_str: &str, ghost_str: &str) {
        let Some(sum) = HotkeySpec::parse(summary_str) else {
            return;
        };
        let Some(gh) = HotkeySpec::parse(ghost_str) else {
            return;
        };
        if let Ok(mut g) = self.shared.lock() {
            g.summary = sum;
            g.ghost = gh;
            #[cfg(target_os = "linux")]
            {
                g.dirty = true;
            }
        }
        #[cfg(windows)]
        {
            use std::sync::atomic::Ordering;
            use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};
            let tid = self.thread_id.load(Ordering::SeqCst);
            if tid != 0 {
                unsafe {
                    let _ = PostThreadMessageW(tid, WM_APP + 1, 0, 0);
                }
            }
        }
    }

    pub fn warning(&self) -> String {
        self.shared
            .lock()
            .map(|g| g.status_warning.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_parse_and_format_roundtrip() {
        let s1 = HotkeySpec::parse("ctrl+enter").unwrap();
        assert_eq!(s1.format(), "Ctrl+Enter");

        let s2 = HotkeySpec::parse("Shift + Alt + F9").unwrap();
        assert_eq!(s2.format(), "Shift+Alt+F9");

        // Any single key or Win+combo is now supported!
        assert_eq!(HotkeySpec::parse("f10").unwrap().format(), "F10");
        assert_eq!(HotkeySpec::parse("insert").unwrap().format(), "Insert");
        assert_eq!(HotkeySpec::parse("a").unwrap().format(), "A");
        assert_eq!(HotkeySpec::parse("win+shift+g").unwrap().format(), "Shift+Win+G");
        // Bare Escape reserved for cancelling capture
        assert!(HotkeySpec::parse("Escape").is_none());
        assert!(HotkeySpec::parse("Ctrl+Escape").is_some());
    }
}
