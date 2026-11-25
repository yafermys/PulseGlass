use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleSummary,
    ToggleGhost,
}

/// Starts a background listener for global hotkeys:
/// - Ctrl + Enter -> Toggle Summary OSD mode
/// - Shift + Enter -> Toggle Ghost (click-through) mode
pub fn start_global_hotkeys(ctx: egui::Context) -> Receiver<HotkeyAction> {
    let (tx, rx) = mpsc::channel();

    #[cfg(windows)]
    {
        std::thread::Builder::new()
            .name("pulseglass-hotkeys".to_string())
            .spawn(move || unsafe {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT, VK_RETURN,
                };
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetMessageW, MSG, WM_HOTKEY,
                };

                const ID_GHOST: i32 = 9001;
                const ID_SUMMARY: i32 = 9002;

                let _ = RegisterHotKey(
                    std::ptr::null_mut(),
                    ID_GHOST,
                    MOD_SHIFT,
                    VK_RETURN as u32,
                );
                let _ = RegisterHotKey(
                    std::ptr::null_mut(),
                    ID_SUMMARY,
                    MOD_CONTROL,
                    VK_RETURN as u32,
                );

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.message == WM_HOTKEY {
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
        std::thread::Builder::new()
            .name("pulseglass-hotkeys".to_string())
            .spawn(move || {
                use global_hotkey::hotkey::{Code, HotKey, Modifiers};
                use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

                let Ok(manager) = GlobalHotKeyManager::new() else {
                    return;
                };
                let hk_summary = HotKey::new(Some(Modifiers::CONTROL), Code::Enter);
                let hk_ghost = HotKey::new(Some(Modifiers::SHIFT), Code::Enter);
                let _ = manager.register(hk_summary);
                let _ = manager.register(hk_ghost);

                let rx_ev = GlobalHotKeyEvent::receiver();
                while let Ok(ev) = rx_ev.recv() {
                    if ev.state != HotKeyState::Pressed {
                        continue;
                    }
                    let action = if ev.id == hk_summary.id() {
                        Some(HotkeyAction::ToggleSummary)
                    } else if ev.id == hk_ghost.id() {
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
            })
            .ok();
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (tx, ctx);
    }

    rx
}
