pub mod gauges;
pub mod i18n;

use crate::hotkeys::{start_global_hotkeys, HotkeyAction};
use crate::installer::{
    install_current_to_system, is_autostart_enabled, is_system_installed, open_file_location,
    open_url_non_elevated, set_autostart, uninstall_from_system, APP_VERSION,
};
use crate::models::{HardwareSnapshot, UpdateCheckResult};
use crate::settings::{is_portable_mode, load_settings, parse_hex_color, save_settings, AppSettings};
use crate::telemetry::{kill_process_by_pid, TelemetryHandle};
use crate::updater::{check_all_updates, download_and_apply_app_update, install_windows_updates};
use gauges::{draw_circular_gauge, draw_shadowed_osd_text, glass_card};
use i18n::I18n;

use egui::{Color32, RichText, Rounding, Stroke, Vec2, ViewportCommand};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    CompactWidget,
    SummaryOsd,
    DetailedWidget,
    FullWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Overview,
    Hardware,
    Processes,
    Settings,
    Updates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcSort {
    Cpu,
    Ram,
    Disk,
    Name,
}

pub struct PulseGlassApp {
    settings: AppSettings,
    mode: ViewMode,
    prev_non_summary_mode: ViewMode,
    tab: DetailTab,
    ghost_mode: bool,
    positioned_initial: bool,
    last_osd_size: Vec2,

    telemetry: TelemetryHandle,
    hotkeys: Receiver<HotkeyAction>,

    // Process view state
    proc_search: String,
    proc_sort: ProcSort,
    proc_group: bool,

    // Updates state
    update_result: Arc<RwLock<Option<UpdateCheckResult>>>,
    checking_updates: Arc<std::sync::atomic::AtomicBool>,
    update_action_msg: Arc<RwLock<String>>,
    last_update_check: Instant,
    startup_update_triggered: bool,

    // Installer UI feedback
    install_desktop_shortcut: bool,
    install_status_msg: String,
    is_installed: bool,
    is_portable: bool,
    autostart_enabled: bool,

    // Optional automated UI screenshot verification (--screenshot-test <path>)
    screenshot_out: Option<String>,
    screenshot_requested: bool,
    frame_count: u32,

    // Native Win32 manual window drag state: (hwnd, start_cursor_x, start_cursor_y, start_win_x, start_win_y)
    win_drag: Option<(isize, i32, i32, i32, i32)>,
}

impl PulseGlassApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = load_settings();
        load_system_fonts(&cc.egui_ctx);
        configure_visuals(&cc.egui_ctx);

        let telemetry = TelemetryHandle::spawn(&settings, cc.egui_ctx.clone());
        let hotkeys = start_global_hotkeys(cc.egui_ctx.clone());

        Self {
            autostart_enabled: is_autostart_enabled(),
            is_installed: is_system_installed(),
            is_portable: is_portable_mode(),
            settings,
            mode: ViewMode::CompactWidget,
            prev_non_summary_mode: ViewMode::CompactWidget,
            tab: DetailTab::Overview,
            ghost_mode: false,
            positioned_initial: false,
            last_osd_size: Vec2::ZERO,
            telemetry,
            hotkeys,
            proc_search: String::new(),
            proc_sort: ProcSort::Cpu,
            proc_group: true,
            update_result: Arc::new(RwLock::new(None)),
            checking_updates: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_action_msg: Arc::new(RwLock::new(String::new())),
            last_update_check: Instant::now(),
            startup_update_triggered: false,
            install_desktop_shortcut: true,
            install_status_msg: String::new(),
            screenshot_out: {
                let args: Vec<String> = std::env::args().collect();
                args.windows(2)
                    .find(|w| w[0] == "--screenshot-test")
                    .map(|w| w[1].clone())
            },
            screenshot_requested: false,
            frame_count: 0,
            win_drag: None,
        }
    }

    fn begin_window_drag(&mut self, ctx: &egui::Context) {
        if self.ghost_mode {
            return;
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{POINT, RECT};
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                FindWindowW, GetCursorPos, GetWindowRect,
            };
            let title: Vec<u16> = "PulseGlass Hardware Monitor\0".encode_utf16().collect();
            unsafe {
                let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                if !hwnd.is_null() {
                    let mut pt = POINT { x: 0, y: 0 };
                    let mut rc = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    if GetCursorPos(&mut pt) != 0 && GetWindowRect(hwnd, &mut rc) != 0 {
                        self.win_drag = Some((hwnd as isize, pt.x, pt.y, rc.left, rc.top));
                        ctx.request_repaint();
                        return;
                    }
                }
            }
        }
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    fn tick_window_drag(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if let Some((hwnd_raw, sx, sy, wx, wy)) = self.win_drag {
            use windows_sys::Win32::Foundation::POINT;
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetCursorPos, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            };
            unsafe {
                let l_down = (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0;
                let m_down = (GetAsyncKeyState(VK_MBUTTON as i32) as u16 & 0x8000) != 0;
                if l_down || m_down {
                    let mut pt = POINT { x: 0, y: 0 };
                    if GetCursorPos(&mut pt) != 0 {
                        let nx = wx + (pt.x - sx);
                        let ny = wy + (pt.y - sy);
                        let _ = SetWindowPos(
                            hwnd_raw as _,
                            std::ptr::null_mut(),
                            nx,
                            ny,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                        if self.mode == ViewMode::CompactWidget || self.mode == ViewMode::SummaryOsd {
                            let ppp = ctx.pixels_per_point().max(1.0);
                            self.settings.pos_x = Some(nx as f32 / ppp);
                            self.settings.pos_y = Some(ny as f32 / ppp);
                        }
                    }
                    ctx.request_repaint();
                } else {
                    self.win_drag = None;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
        }
    }

    fn primary_color(&self) -> Color32 {
        parse_hex_color(&self.settings.primary_hex, Color32::from_rgb(255, 77, 109))
    }

    fn secondary_color(&self) -> Color32 {
        parse_hex_color(
            &self.settings.secondary_hex,
            Color32::from_rgb(255, 176, 133),
        )
    }

    fn bg_base_color(&self) -> Color32 {
        parse_hex_color(&self.settings.bg_hex, Color32::from_rgb(20, 14, 22))
    }

    /// Independent opacities:
    /// - Summary OSD: ALWAYS completely transparent (0 alpha)
    /// - Compact/Detailed Widget: uses settings.widget_opacity ONLY
    /// - Full Window: uses settings.window_opacity ONLY
    fn active_bg_color(&self) -> Color32 {
        match self.mode {
            ViewMode::SummaryOsd => Color32::TRANSPARENT,
            ViewMode::FullWindow => {
                let base = self.bg_base_color();
                let a = (self.settings.window_opacity.clamp(0.35, 1.0) * 255.0) as u8;
                Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a)
            }
            ViewMode::CompactWidget | ViewMode::DetailedWidget => {
                let base = self.bg_base_color();
                let a = (self.settings.widget_opacity.clamp(0.25, 1.0) * 255.0) as u8;
                Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a)
            }
        }
    }

    fn toggle_summary(&mut self, ctx: &egui::Context) {
        if self.mode == ViewMode::SummaryOsd {
            self.mode = self.prev_non_summary_mode;
        } else {
            self.prev_non_summary_mode = self.mode;
            self.mode = ViewMode::SummaryOsd;
            self.last_osd_size = Vec2::ZERO;
        }
        self.apply_window_geometry(ctx);
    }

    fn toggle_ghost(&mut self, ctx: &egui::Context) {
        self.ghost_mode = !self.ghost_mode;
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(self.ghost_mode));
    }

    fn apply_window_geometry(&self, ctx: &egui::Context) {
        let s = self.settings.ui_scale.clamp(0.85, 1.35);
        if (s - 1.0).abs() > 0.01 {
            ctx.set_zoom_factor(s);
        } else {
            ctx.set_zoom_factor(1.0);
        }

        match self.mode {
            ViewMode::SummaryOsd => {
                ctx.send_viewport_cmd(ViewportCommand::Resizable(false));
                ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(520.0, 32.0)));
            }
            ViewMode::CompactWidget => {
                ctx.send_viewport_cmd(ViewportCommand::Resizable(false));
                let lvl = if self.settings.topmost {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ctx.send_viewport_cmd(ViewportCommand::WindowLevel(lvl));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(480.0, 294.0)));
                if let (Some(x), Some(y)) = (self.settings.pos_x, self.settings.pos_y) {
                    if x > -5000.0 && y > -5000.0 {
                        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(x, y)));
                    }
                }
            }
            ViewMode::DetailedWidget => {
                ctx.send_viewport_cmd(ViewportCommand::Resizable(false));
                let lvl = if self.settings.topmost {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ctx.send_viewport_cmd(ViewportCommand::WindowLevel(lvl));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(660.0, 560.0)));
            }
            ViewMode::FullWindow => {
                ctx.send_viewport_cmd(ViewportCommand::Resizable(true));
                ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(900.0, 640.0)));
                if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                    let x = ((mon.x - 900.0) * 0.5).max(16.0);
                    let y = ((mon.y - 640.0) * 0.5).max(16.0);
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(x, y)));
                }
            }
        }
    }

    fn trigger_update_check(&mut self, ctx: &egui::Context) {
        if self.checking_updates.swap(true, Ordering::SeqCst) {
            return;
        }
        self.last_update_check = Instant::now();
        let en = self.settings.language.eq_ignore_ascii_case("en");
        let res_arc = Arc::clone(&self.update_result);
        let flag = Arc::clone(&self.checking_updates);
        let ctx_bg = ctx.clone();

        std::thread::Builder::new()
            .name("pulseglass-updater".to_string())
            .spawn(move || {
                let r = check_all_updates(en);
                if let Ok(mut g) = res_arc.write() {
                    *g = Some(r);
                }
                flag.store(false, Ordering::SeqCst);
                ctx_bg.request_repaint();
            })
            .ok();
    }

    /// Restores exact original PulseGlass summary format: CPU 28% (52°C) • GPU 0% (41°C) • RAM 78% • SSD 56°C • FPS: --
    fn build_summary_line(&self, snap: &HardwareSnapshot, tr: &I18n) -> String {
        let mut parts = Vec::new();
        if self.settings.sum_cpu_pct || self.settings.sum_cpu_temp {
            let mut s = String::from("CPU");
            if self.settings.sum_cpu_pct {
                s.push_str(&format!(" {:.0}%", snap.cpu.usage_percent));
            }
            if self.settings.sum_cpu_temp {
                s.push_str(&format!(" ({})", snap.cpu.temp_text()));
            }
            parts.push(s);
        }
        if self.settings.sum_gpu_pct || self.settings.sum_gpu_temp {
            let mut s = String::from("GPU");
            if self.settings.sum_gpu_pct {
                s.push_str(&format!(" {:.0}%", snap.gpu.usage_percent));
            }
            if self.settings.sum_gpu_temp {
                s.push_str(&format!(" ({})", snap.gpu.temp_text()));
            }
            parts.push(s);
        }
        if self.settings.sum_igpu_pct && snap.igpu.is_available {
            parts.push(format!("iGPU {:.0}%", snap.igpu.usage_percent));
        }
        if self.settings.sum_ram_pct {
            parts.push(format!("RAM {:.0}%", snap.ram.usage_percent));
        }
        if self.settings.sum_ssd_temp {
            parts.push(format!("SSD {}", snap.storage.temp_text()));
        }
        if self.settings.sum_gpu_watts {
            parts.push(format!("GPU {}", snap.power.gpu_text()));
        }
        if self.settings.sum_device_watts {
            parts.push(format!("{} {}", tr.t("Cihaz", "Sys"), snap.power.device_text()));
        }
        if self.settings.sum_fps {
            match snap.fps {
                Some(f) => parts.push(format!("FPS: {}", f)),
                None => parts.push("FPS: --".to_string()),
            }
        }
        if parts.is_empty() {
            tr.t("Özet boş (Ayarlar'dan seçin)", "Summary empty (select in Settings)")
                .to_string()
        } else {
            parts.join("  •  ")
        }
    }
}

fn header_btn(ui: &mut egui::Ui, text: &str, active: bool, primary: Color32, secondary: Color32) -> egui::Response {
    let fill = if active {
        Color32::from_rgb(68, 30, 44)
    } else {
        Color32::from_rgb(40, 32, 44)
    };
    let txt_col = if active {
        secondary
    } else {
        Color32::from_rgb(241, 224, 222)
    };
    let stroke = if active {
        Stroke::new(1.0_f32, primary)
    } else {
        Stroke::new(0.8_f32, Color32::from_rgb(62, 50, 66))
    };
    ui.add(
        egui::Button::new(RichText::new(text).size(10.5).color(txt_col))
            .fill(fill)
            .stroke(stroke)
            .rounding(Rounding::same(7.0))
            .min_size(Vec2::new(22.0, 22.0)),
    )
}

fn tab_pill_btn(ui: &mut egui::Ui, text: &str, selected: bool, secondary: Color32) -> egui::Response {
    let fill = if selected {
        Color32::from_rgb(66, 28, 42)
    } else {
        Color32::TRANSPARENT
    };
    let txt_col = if selected {
        secondary
    } else {
        Color32::from_rgb(160, 174, 192)
    };
    ui.add(
        egui::Button::new(RichText::new(text).size(11.5).strong().color(txt_col))
            .fill(fill)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(8.0))
            .min_size(Vec2::new(56.0, 26.0)),
    )
}

fn shortcut_pill_btn(ui: &mut egui::Ui, text: &str, secondary: Color32) -> egui::Response {
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::Button::new(RichText::new(text).size(11.5).strong().color(Color32::from_rgb(248, 236, 232)))
            .fill(Color32::from_rgb(38, 30, 42))
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(secondary.r(), secondary.g(), secondary.b(), 55),
            ))
            .rounding(Rounding::same(9.0)),
    )
}

impl eframe::App for PulseGlassApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.telemetry.stop.store(true, Ordering::Relaxed);
        save_settings(&self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame_count = self.frame_count.saturating_add(1);

        // Handle --screenshot-test <path> for automated visual verification
        if let Some(ref out_path) = self.screenshot_out.clone() {
            if !self.screenshot_requested && self.frame_count >= 5 {
                self.screenshot_requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot);
            }
            let captured = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(Arc::clone(image))
                    } else {
                        None
                    }
                })
            });
            if let Some(img) = captured {
                let rgba: Vec<u8> = img
                    .pixels
                    .iter()
                    .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
                    .collect();
                let _ = image::save_buffer(
                    out_path,
                    &rgba,
                    img.width() as u32,
                    img.height() as u32,
                    image::ColorType::Rgba8,
                );
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
        }

        // 1. Default top-right positioning on first launch (validating saved coords so minimized coords never trap off-screen)
        if !self.positioned_initial {
            self.positioned_initial = true;
            let mon = ctx.input(|i| i.viewport().monitor_size);
            let valid_saved = match (self.settings.pos_x, self.settings.pos_y, mon) {
                (Some(x), Some(y), Some(m)) => {
                    x >= -100.0 && x <= m.x - 80.0 && y >= 0.0 && y <= m.y - 60.0
                }
                (Some(x), Some(y), None) => x > -5000.0 && y > -5000.0,
                _ => false,
            };
            if valid_saved {
                if let (Some(x), Some(y)) = (self.settings.pos_x, self.settings.pos_y) {
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(x, y)));
                }
            } else if let Some(m) = mon {
                let x = (m.x - 480.0 - 16.0).max(16.0);
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(x, 16.0)));
                self.settings.pos_x = Some(x);
                self.settings.pos_y = Some(16.0);
            }
            if (self.settings.ui_scale - 1.0).abs() > 0.01 {
                ctx.set_zoom_factor(self.settings.ui_scale.clamp(0.85, 1.35));
            }
        }

        // Save current position only when on-screen and not minimized (-32000 on Windows)
        if self.mode == ViewMode::CompactWidget || self.mode == ViewMode::SummaryOsd {
            if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
                if rect.min.x > -5000.0 && rect.min.y > -5000.0 {
                    self.settings.pos_x = Some(rect.min.x);
                    self.settings.pos_y = Some(rect.min.y);
                }
            }
        }

        // 2. Process global hotkeys (Ctrl+Enter -> Summary OSD, Shift+Enter -> Ghost Mode)
        while let Ok(action) = self.hotkeys.try_recv() {
            match action {
                HotkeyAction::ToggleSummary => self.toggle_summary(ctx),
                HotkeyAction::ToggleGhost => self.toggle_ghost(ctx),
            }
        }
        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::Enter) {
                self.toggle_summary(ctx);
            } else if i.modifiers.shift && i.key_pressed(egui::Key::Enter) {
                self.toggle_ghost(ctx);
            }
        });

        // 3. Startup (after 6s) + 6-hour periodic update check
        if (!self.startup_update_triggered && self.last_update_check.elapsed() > Duration::from_secs(6))
            || self.last_update_check.elapsed() > Duration::from_secs(6 * 3600)
        {
            self.startup_update_triggered = true;
            self.trigger_update_check(ctx);
        }

        let need_procs = matches!(self.mode, ViewMode::DetailedWidget | ViewMode::FullWindow)
            && matches!(self.tab, DetailTab::Processes | DetailTab::Overview);
        self.telemetry
            .include_processes
            .store(need_procs, Ordering::Relaxed);
        self.telemetry
            .group_processes
            .store(self.proc_group, Ordering::Relaxed);

        let snap = self
            .telemetry
            .snapshot
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        let tr = I18n::new(self.settings.language.eq_ignore_ascii_case("en"));
        let primary = self.primary_color();
        let secondary = self.secondary_color();

        // Continue active Win32 drag smoothly every frame until mouse button released
        self.tick_window_drag(ctx);

        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
        let middle_pressed = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle));

        // Middle-click drag works from ANYWHERE on the window
        if middle_pressed && !self.ghost_mode {
            self.begin_window_drag(ctx);
        }

        // 4. PURE SUMMARY OSD MODE — ONLY selected values, ZERO chrome, ALWAYS transparent, instant left-drag!
        if self.mode == ViewMode::SummaryOsd {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(Color32::TRANSPARENT))
                .show(ctx, |ui| {
                    let line = self.build_summary_line(&snap, &tr);
                    let (rect, resp) = draw_shadowed_osd_text(
                        ui,
                        &line,
                        Color32::from_rgb(255, 228, 214),
                        13.5,
                    );
                    if !self.ghost_mode && (resp.hovered() && primary_pressed) {
                        self.begin_window_drag(ctx);
                    }
                    let target = Vec2::new((rect.width() + 12.0).max(140.0), (rect.height() + 6.0).max(26.0));
                    if (target - self.last_osd_size).length() > 3.0 {
                        self.last_osd_size = target;
                        ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
                    }
                });
            return;
        }

        // 5. GLASSMORPHIC FRAME FOR WIDGET & WINDOW MODES
        let bg_fill = self.active_bg_color();
        let border_color = if !snap.alerts.is_empty() {
            primary
        } else {
            Color32::from_rgba_unmultiplied(secondary.r(), secondary.g(), secondary.b(), 82)
        };

        let frame = egui::Frame::none()
            .fill(bg_fill)
            .stroke(Stroke::new(1.1_f32, border_color))
            .rounding(Rounding::same(if self.mode == ViewMode::FullWindow {
                10.0
            } else {
                16.0
            }))
            .outer_margin(egui::Margin::same(4.0))
            .inner_margin(egui::Margin::symmetric(14.0, 11.0));

        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |ui| {
                // --- HEADER BAR (Instant drag on primary_pressed over title/dot/spacer, buttons 100% clickable!) ---
                let mut wants_drag = false;
                let mut btn_hovered = false;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let dot_color = if !snap.alerts.is_empty() {
                        Color32::from_rgb(239, 68, 68)
                    } else {
                        primary
                    };
                    let (dot_rect, dot_resp) =
                        ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::click_and_drag());
                    ui.painter().circle_filled(dot_rect.center(), 4.2, dot_color);
                    if dot_resp.hovered() && primary_pressed {
                        wants_drag = true;
                    }

                    let title_resp = ui.add(
                        egui::Label::new(
                            RichText::new("PulseGlass")
                                .strong()
                                .size(13.5)
                                .color(Color32::from_rgb(255, 241, 235)),
                        )
                        .sense(egui::Sense::click_and_drag()),
                    );
                    if title_resp.hovered() && primary_pressed {
                        wants_drag = true;
                    }

                    if let Some(alert) = snap.alerts.first() {
                        ui.label(
                            RichText::new(format!("! {}", alert))
                                .size(10.0)
                                .strong()
                                .color(primary),
                        );
                    } else if self.ghost_mode {
                        ui.label(
                            RichText::new(tr.t("Geçirgen", "Ghost"))
                                .size(10.0)
                                .color(secondary),
                        );
                    }

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let r = header_btn(ui, "×", false, primary, secondary)
                                .on_hover_text(tr.t("Kapat", "Close"));
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                ctx.send_viewport_cmd(ViewportCommand::Close);
                            }

                            let r = header_btn(ui, "—", false, primary, secondary)
                                .on_hover_text(tr.t("Küçült", "Minimize"));
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                            }

                            let det_lbl = if self.mode == ViewMode::DetailedWidget {
                                tr.t("Kompakt", "Compact")
                            } else {
                                tr.t("Detay", "Detail")
                            };
                            let r = header_btn(ui, det_lbl, self.mode == ViewMode::DetailedWidget, primary, secondary);
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                self.mode = if self.mode == ViewMode::DetailedWidget {
                                    ViewMode::CompactWidget
                                } else {
                                    ViewMode::DetailedWidget
                                };
                                self.apply_window_geometry(ctx);
                            }

                            let win_lbl = if self.mode == ViewMode::FullWindow {
                                tr.t("Widget", "Widget")
                            } else {
                                tr.t("Pencere", "Window")
                            };
                            let r = header_btn(ui, win_lbl, self.mode == ViewMode::FullWindow, primary, secondary);
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                self.mode = if self.mode == ViewMode::FullWindow {
                                    ViewMode::CompactWidget
                                } else {
                                    ViewMode::FullWindow
                                };
                                self.apply_window_geometry(ctx);
                            }

                            let r = header_btn(ui, tr.t("Özet", "Summary"), false, primary, secondary)
                                .on_hover_text(tr.t("Özet Modu (Ctrl+Enter)", "Summary OSD (Ctrl+Enter)"));
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                self.toggle_summary(ctx);
                            }

                            let r = header_btn(ui, tr.t("Üstte", "Pin"), self.settings.topmost, primary, secondary)
                                .on_hover_text(tr.t("Her zaman üstte", "Always on top"));
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                self.settings.topmost = !self.settings.topmost;
                                self.apply_window_geometry(ctx);
                            }

                            let r = header_btn(ui, tr.t("Hayalet", "Ghost"), self.ghost_mode, primary, secondary)
                                .on_hover_text(tr.t("Geçirgen Mod (Shift+Enter)", "Ghost Click-Through (Shift+Enter)"));
                            btn_hovered |= r.hovered();
                            if r.clicked() {
                                self.toggle_ghost(ctx);
                            }

                            // Empty spacer between title and buttons is also immediately draggable
                            let rem = ui.available_size();
                            if rem.x > 4.0 {
                                let (_, sp) = ui.allocate_exact_size(Vec2::new(rem.x, 22.0), egui::Sense::click_and_drag());
                                if sp.hovered() && primary_pressed {
                                    wants_drag = true;
                                }
                            }
                        },
                    );
                });

                ui.add_space(7.0);

                match self.mode {
                    ViewMode::CompactWidget => {
                        let shortcut_hovered =
                            self.render_compact_widget(ui, ctx, &snap, &tr, primary, secondary);
                        if !self.ghost_mode && primary_pressed && !btn_hovered && !shortcut_hovered {
                            wants_drag = true;
                        }
                    }
                    ViewMode::DetailedWidget | ViewMode::FullWindow => {
                        self.render_detailed_dashboard(ui, ctx, &snap, &tr, primary, secondary);
                    }
                    ViewMode::SummaryOsd => {}
                }

                if wants_drag && !self.ghost_mode {
                    self.begin_window_drag(ctx);
                }
            });
    }
}

impl PulseGlassApp {
    fn render_compact_widget(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        snap: &HardwareSnapshot,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) -> bool {
        // 4 Circular Gauges (matching original compact-widget.png)
        ui.columns(4, |cols| {
            cols[0].vertical_centered(|ui| {
                draw_circular_gauge(
                    ui,
                    76.0,
                    snap.cpu.usage_percent,
                    "CPU",
                    &snap.cpu.temp_text(),
                    secondary,
                    primary,
                    self.settings.cpu_use_thresh,
                );
                ui.add_space(2.0);
                ui.label(RichText::new("CPU").strong().size(11.5).color(Color32::from_rgb(255, 232, 224)));
                ui.label(
                    RichText::new(format!("{:.2} GHz", snap.cpu.current_clock_ghz))
                        .size(10.5)
                        .color(secondary),
                );
            });

            cols[1].vertical_centered(|ui| {
                draw_circular_gauge(
                    ui,
                    76.0,
                    snap.gpu.usage_percent,
                    "GPU",
                    &snap.gpu.temp_text(),
                    secondary,
                    primary,
                    85.0,
                );
                ui.add_space(2.0);
                ui.label(RichText::new(tr.t("Harici GPU", "Discrete GPU")).strong().size(11.0).color(Color32::from_rgb(255, 232, 224)));
                ui.label(
                    RichText::new(snap.gpu.vram_text())
                        .size(10.5)
                        .color(secondary),
                );
            });

            cols[2].vertical_centered(|ui| {
                let igpu_temp = if snap.igpu.is_available {
                    snap.igpu.temp_text()
                } else {
                    "N/A".to_string()
                };
                draw_circular_gauge(
                    ui,
                    76.0,
                    if snap.igpu.is_available { snap.igpu.usage_percent } else { 0.0 },
                    "iGPU",
                    &igpu_temp,
                    secondary,
                    primary,
                    85.0,
                );
                ui.add_space(2.0);
                ui.label(RichText::new(tr.t("Dahili GPU", "Integrated GPU")).strong().size(11.0).color(Color32::from_rgb(255, 232, 224)));
                let sub = if snap.igpu.is_available {
                    format!("{:.1} GB", snap.igpu.total_vram_gb.max(snap.igpu.used_vram_gb))
                } else {
                    tr.t("Yok", "None").to_string()
                };
                ui.label(RichText::new(sub).size(10.5).color(secondary));
            });

            cols[3].vertical_centered(|ui| {
                let ram_sub = if snap.ram.used_gb >= 1.0 {
                    format!("{:.0} GB", snap.ram.used_gb.round())
                } else {
                    format!("{:.0} MB", snap.ram.used_gb * 1024.0)
                };
                draw_circular_gauge(
                    ui,
                    76.0,
                    snap.ram.usage_percent,
                    "RAM",
                    &ram_sub,
                    secondary,
                    primary,
                    75.0_f32.min(self.settings.ram_use_thresh),
                );
                ui.add_space(2.0);
                ui.label(RichText::new("RAM").strong().size(11.5).color(Color32::from_rgb(255, 232, 224)));
                ui.label(
                    RichText::new(format!("{:.1} / {:.0} GB", snap.ram.used_gb, snap.ram.installed_gb))
                        .size(10.5)
                        .color(secondary),
                );
            });
        });

        ui.add_space(8.0);

        // Compact Telemetry Footer: SSD (M.2/SATA + Temp) + Power (GPU W / Device W) + Network + FPS
        glass_card(ui, secondary, |ui| {
            ui.horizontal(|ui| {
                let ssd_iface = if snap.storage.primary_interface.is_empty() {
                    "SSD".to_string()
                } else {
                    format!("SSD [{}]", snap.storage.primary_interface)
                };
                ui.label(
                    RichText::new(format!(
                        "{}: %{:.0}  •  {}  •  GPU {} / {}: {}",
                        ssd_iface,
                        snap.storage.usage_percent,
                        snap.storage.temp_text(),
                        snap.power.gpu_text(),
                        tr.t("Cihaz", "Sys"),
                        snap.power.device_text()
                    ))
                    .size(10.5)
                    .color(secondary),
                );
            });

            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "↓ {:.2} MB/s    ↑ {:.2} MB/s",
                        snap.network.download_mbps, snap.network.upload_mbps
                    ))
                    .size(10.5)
                    .color(Color32::from_rgb(203, 213, 225)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let fps_txt = match snap.fps {
                        Some(f) => format!("FPS: {}", f),
                        None => "FPS: --".to_string(),
                    };
                    ui.label(RichText::new(fps_txt).strong().size(11.5).color(primary));
                });
            });
        });

        ui.add_space(8.0);

        // Restored 2 bottom shortcut buttons matching original compact-widget.png (Apps & Hardware)
        let mut shortcut_hovered = false;
        ui.columns(2, |cols| {
            let r1 = shortcut_pill_btn(&mut cols[0], tr.t("Uygulamalar", "Apps"), secondary);
            shortcut_hovered |= r1.hovered();
            if r1.clicked() {
                self.tab = DetailTab::Processes;
                self.mode = ViewMode::DetailedWidget;
                self.apply_window_geometry(ctx);
            }
            let r2 = shortcut_pill_btn(&mut cols[1], tr.t("Donanım", "Hardware"), secondary);
            shortcut_hovered |= r2.hovered();
            if r2.clicked() {
                self.tab = DetailTab::Hardware;
                self.mode = ViewMode::DetailedWidget;
                self.apply_window_geometry(ctx);
            }
        });
        shortcut_hovered
    }

    fn render_detailed_dashboard(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        snap: &HardwareSnapshot,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) {
        // Restored clean tab bar matching detailed-overview.png
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let tabs = [
                (DetailTab::Overview, tr.t("Genel Bakış", "Overview")),
                (DetailTab::Hardware, tr.t("Donanım", "Hardware Details")),
                (DetailTab::Processes, tr.t("Uygulamalar", "Processes")),
                (DetailTab::Settings, tr.t("Ayarlar", "Settings")),
                (DetailTab::Updates, tr.t("Güncellemeler", "Updates")),
            ];
            for (t, label) in tabs {
                if tab_pill_btn(ui, label, self.tab == t, secondary).clicked() {
                    self.tab = t;
                }
            }
        });
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                DetailTab::Overview => self.render_tab_overview(ui, snap, tr, primary, secondary),
                DetailTab::Hardware => self.render_tab_hardware(ui, snap, tr, primary, secondary),
                DetailTab::Processes => self.render_tab_processes(ui, snap, tr, primary, secondary),
                DetailTab::Settings => self.render_tab_settings(ui, ctx, tr, primary, secondary),
                DetailTab::Updates => self.render_tab_updates(ui, ctx, tr, primary, secondary),
            });
    }

    fn render_tab_overview(
        &mut self,
        ui: &mut egui::Ui,
        snap: &HardwareSnapshot,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) {
        // 4 Rounded Gauge Cards matching detailed-overview.png
        ui.columns(4, |cols| {
            glass_card(&mut cols[0], secondary, |ui| {
                ui.vertical_centered(|ui| {
                    draw_circular_gauge(
                        ui,
                        82.0,
                        snap.cpu.usage_percent,
                        "CPU",
                        &snap.cpu.temp_text(),
                        secondary,
                        primary,
                        self.settings.cpu_use_thresh,
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr.t("İşlemci (CPU)", "Processor (CPU)")).strong().size(11.0).color(Color32::from_rgb(255, 241, 235)));
                    ui.label(
                        RichText::new(format!(
                            "{:.2} GHz ({}C/{}T)",
                            snap.cpu.current_clock_ghz, snap.cpu.physical_cores, snap.cpu.logical_processors
                        ))
                        .size(10.0)
                        .color(secondary),
                    );
                });
            });

            glass_card(&mut cols[1], secondary, |ui| {
                ui.vertical_centered(|ui| {
                    draw_circular_gauge(
                        ui,
                        82.0,
                        snap.gpu.usage_percent,
                        "GPU",
                        &snap.gpu.temp_text(),
                        secondary,
                        primary,
                        85.0,
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr.t("Harici GPU", "Discrete GPU")).strong().size(11.0).color(Color32::from_rgb(255, 241, 235)));
                    ui.label(RichText::new(snap.gpu.vram_text()).size(10.0).color(secondary));
                });
            });

            glass_card(&mut cols[2], secondary, |ui| {
                ui.vertical_centered(|ui| {
                    let igpu_t = if snap.igpu.is_available {
                        snap.igpu.temp_text()
                    } else {
                        "N/A".to_string()
                    };
                    draw_circular_gauge(
                        ui,
                        82.0,
                        if snap.igpu.is_available { snap.igpu.usage_percent } else { 0.0 },
                        "iGPU",
                        &igpu_t,
                        secondary,
                        primary,
                        85.0,
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr.t("Dahili GPU (iGPU)", "Integrated GPU")).strong().size(11.0).color(Color32::from_rgb(255, 241, 235)));
                    let sub = if snap.igpu.is_available {
                        format!("{:.1} GB", snap.igpu.total_vram_gb.max(snap.igpu.used_vram_gb))
                    } else {
                        tr.t("Algılanmadı", "Not Detected").to_string()
                    };
                    ui.label(RichText::new(sub).size(10.0).color(secondary));
                });
            });

            glass_card(&mut cols[3], secondary, |ui| {
                ui.vertical_centered(|ui| {
                    let ram_sub = if snap.ram.used_gb >= 1.0 {
                        format!("{:.0} GB", snap.ram.used_gb.round())
                    } else {
                        format!("{:.0} MB", snap.ram.used_gb * 1024.0)
                    };
                    draw_circular_gauge(
                        ui,
                        82.0,
                        snap.ram.usage_percent,
                        "RAM",
                        &ram_sub,
                        secondary,
                        primary,
                        75.0_f32.min(self.settings.ram_use_thresh),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr.t("Bellek (RAM)", "Memory (RAM)")).strong().size(11.0).color(Color32::from_rgb(255, 241, 235)));
                    ui.label(
                        RichText::new(format!("{:.1} / {:.0} GB", snap.ram.used_gb, snap.ram.installed_gb))
                            .size(10.0)
                            .color(secondary),
                    );
                });
            });
        });

        ui.add_space(8.0);

        // Storage + Network + FPS Strip (2 clean rows so FPS is never clipped by long SSD names)
        glass_card(ui, secondary, |ui| {
            ui.horizontal(|ui| {
                let ssd_iface = if snap.storage.primary_interface.is_empty() {
                    "SSD".to_string()
                } else {
                    format!("SSD [{}]", snap.storage.primary_interface)
                };
                ui.label(
                    RichText::new(format!(
                        "{}: %{:.0}  •  {}: {}",
                        ssd_iface,
                        snap.storage.usage_percent,
                        tr.t("Sıcaklık", "Temp"),
                        snap.storage.temp_text(),
                    ))
                    .size(11.0)
                    .color(secondary),
                );
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{}: ↓{:.2}  ↑{:.2} MB/s",
                        tr.t("Ağ", "Net"),
                        snap.network.download_mbps,
                        snap.network.upload_mbps
                    ))
                    .size(11.0)
                    .color(Color32::from_rgb(203, 213, 225)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let fps_txt = match snap.fps {
                        Some(f) => format!("FPS: {}", f),
                        None => "FPS: --".to_string(),
                    };
                    ui.label(RichText::new(fps_txt).strong().size(12.5).color(primary));
                });
            });
        });

        ui.add_space(8.0);

        // Power & Wattage Card (GPU & Total Device only)
        glass_card(ui, secondary, |ui| {
            ui.label(
                RichText::new(tr.t("Güç Tüketimi (Watt)", "Power Consumption (Watts)"))
                    .strong()
                    .size(12.0)
                    .color(secondary),
            );
            ui.add_space(4.0);
            ui.columns(2, |cols| {
                cols[0].vertical(|ui| {
                    ui.label(RichText::new(tr.t("Ekran Kartı (GPU)", "GPU Power")).size(10.5).color(Color32::GRAY));
                    ui.label(RichText::new(snap.gpu.power_text()).strong().size(14.5).color(primary));
                });
                cols[1].vertical(|ui| {
                    ui.label(RichText::new(tr.t("Genel Cihaz", "Total Device")).size(10.5).color(Color32::GRAY));
                    ui.label(
                        RichText::new(snap.power.device_text())
                            .strong()
                            .size(14.5)
                            .color(Color32::from_rgb(255, 241, 235)),
                    );
                });
            });
            if !snap.power.source_note.is_empty() {
                let bat = snap
                    .power
                    .battery_percent
                    .map(|b| format!("  •  Pil: {}%", b))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!("{}{}", snap.power.source_note, bat))
                        .size(10.0)
                        .color(Color32::from_rgb(160, 175, 190)),
                );
            }
        });

        ui.add_space(8.0);

        // Core Load Bars
        glass_card(ui, secondary, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} ({}C / {}T)",
                    tr.t("Çekirdek Yükleri", "CPU Core Loads"),
                    snap.cpu.physical_cores,
                    snap.cpu.logical_processors
                ))
                .strong()
                .size(11.5)
                .color(secondary),
            );
            ui.add_space(4.0);
            let cols = if snap.cpu.per_core_usage.len() > 8 { 4 } else { 2 };
            egui::Grid::new("core_grid").num_columns(cols).spacing([10.0, 4.0]).show(ui, |ui| {
                for (i, &u) in snap.cpu.per_core_usage.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("C{:02}", i)).size(10.0).color(Color32::GRAY));
                        let bar = egui::ProgressBar::new((u / 100.0).clamp(0.0, 1.0))
                            .desired_width(64.0)
                            .desired_height(9.0)
                            .fill(if u >= self.settings.cpu_use_thresh { primary } else { secondary });
                        ui.add(bar);
                        ui.label(RichText::new(format!("{:.0}%", u)).size(10.0));
                    });
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        });
    }

    fn render_tab_hardware(
        &mut self,
        ui: &mut egui::Ui,
        snap: &HardwareSnapshot,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) {
        glass_card(ui, secondary, |ui| {
            ui.label(RichText::new(tr.t("İŞLEMCİ (CPU)", "PROCESSOR (CPU)")).strong().size(11.0).color(secondary));
            ui.label(RichText::new(&snap.cpu.name).strong().size(13.0).color(Color32::from_rgb(255, 241, 235)));
            ui.label(
                RichText::new(format!(
                    "{}: {} / {}   |   {}: {:.2} GHz   |   {}: {:.1}%   |   {}: {}",
                    tr.t("Çekirdek", "Cores"),
                    snap.cpu.physical_cores,
                    snap.cpu.logical_processors,
                    tr.t("Frekans", "Clock"),
                    snap.cpu.current_clock_ghz,
                    tr.t("Kullanım", "Usage"),
                    snap.cpu.usage_percent,
                    tr.t("Sıcaklık", "Temp"),
                    snap.cpu.temp_text()
                ))
                .size(11.0)
                .color(Color32::from_rgb(203, 213, 225)),
            );

            #[cfg(windows)]
            if !snap.pawnio_installed {
                ui.add_space(3.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(tr.t(
                            "Not: Gelişmiş CPU çekirdek sıcaklığı için PawnIO sürücüsü önerilir (WinRing0 yerine):",
                            "Note: For detailed CPU core temperatures, install the PawnIO driver (replaces WinRing0):",
                        ))
                        .size(10.5)
                        .color(secondary),
                    );
                    if ui.link(RichText::new("pawnio.eu").size(10.5).color(primary)).clicked() {
                        open_url_non_elevated("https://pawnio.eu");
                    }
                });
            }
        });

        ui.add_space(6.0);

        glass_card(ui, secondary, |ui| {
            ui.label(RichText::new(tr.t("HARİCİ EKRAN KARTI (dGPU)", "DISCRETE GPU")).strong().size(11.0).color(secondary));
            ui.label(RichText::new(if snap.gpu.name.is_empty() { "N/A" } else { &snap.gpu.name }).strong().size(13.0).color(Color32::from_rgb(255, 241, 235)));
            ui.label(
                RichText::new(format!(
                    "VRAM: {}   |   {}: {:.0}%   |   {}: {}   |   {}: {}\n{}: {}   |   Fan: {}   |   {}: {}",
                    snap.gpu.vram_text(),
                    tr.t("Kullanım", "Usage"),
                    snap.gpu.usage_percent,
                    tr.t("Sıcaklık", "Temp"),
                    snap.gpu.temp_text(),
                    tr.t("Güç", "Power"),
                    snap.gpu.power_text(),
                    tr.t("Saat Hızı", "Clock"),
                    snap.gpu.clock_mhz.map(|c| format!("{} MHz", c)).unwrap_or_else(|| "N/A".to_string()),
                    snap.gpu.fan_percent.map(|f| format!("{}%", f)).unwrap_or_else(|| "N/A".to_string()),
                    tr.t("Sürücü", "Driver"),
                    if snap.gpu.driver_version.is_empty() { "N/A" } else { &snap.gpu.driver_version }
                ))
                .size(11.0)
                .color(Color32::from_rgb(203, 213, 225)),
            );
        });

        ui.add_space(6.0);

        if snap.igpu.is_available {
            glass_card(ui, secondary, |ui| {
                ui.label(RichText::new(tr.t("DAHİLİ GPU (iGPU)", "INTEGRATED GPU")).strong().size(11.0).color(secondary));
                ui.label(RichText::new(&snap.igpu.name).strong().size(12.5).color(Color32::from_rgb(255, 241, 235)));
                ui.label(
                    RichText::new(format!(
                        "{}: {:.0}%   |   VRAM: {}   |   {}: {}",
                        tr.t("Kullanım", "Usage"),
                        snap.igpu.usage_percent,
                        snap.igpu.vram_text(),
                        tr.t("Sürücü", "Driver"),
                        if snap.igpu.driver_version.is_empty() { "N/A" } else { &snap.igpu.driver_version }
                    ))
                    .size(11.0)
                    .color(Color32::from_rgb(203, 213, 225)),
                );
            });
            ui.add_space(6.0);
        }

        glass_card(ui, secondary, |ui| {
            ui.label(RichText::new(tr.t("GÜÇ & WATT DEĞERLERİ", "POWER & WATTAGE")).strong().size(11.0).color(secondary));
            ui.label(
                RichText::new(format!(
                    "{}: {}   |   {}: {}\n{}: {}   |   {}: {}",
                    tr.t("Ekran Kartı (GPU)", "GPU"),
                    snap.gpu.power_text(),
                    tr.t("Genel Cihaz", "Total Device"),
                    snap.power.device_text(),
                    tr.t("Kaynak", "Source"),
                    if snap.power.source_note.is_empty() { "N/A" } else { &snap.power.source_note },
                    tr.t("Güç Durumu", "Power Mode"),
                    if snap.power.on_battery { tr.t("Pil", "Battery") } else { tr.t("Adaptör / AC", "AC Power") }
                ))
                .size(11.0)
                .color(Color32::from_rgb(203, 213, 225)),
            );
        });

        ui.add_space(6.0);

        glass_card(ui, secondary, |ui| {
            ui.label(RichText::new(tr.t("BELLEK (RAM)", "MEMORY (RAM)")).strong().size(11.0).color(secondary));
            ui.label(
                RichText::new(format!(
                    "{:.1} GB {} ({:.1} GB {}) — {}: {:.2} GB  |  {}: {:.2} GB  ({:.0}%)",
                    snap.ram.installed_gb,
                    tr.t("Takılı", "Installed"),
                    snap.ram.total_gb,
                    tr.t("Kullanılabilir", "Usable"),
                    tr.t("Kullanılan", "Used"),
                    snap.ram.used_gb,
                    tr.t("Boş", "Free"),
                    snap.ram.free_gb,
                    snap.ram.usage_percent
                ))
                .size(11.0)
                .color(Color32::from_rgb(203, 213, 225)),
            );
        });

        ui.add_space(6.0);

        glass_card(ui, secondary, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} — {:.0} GB ({}: {:.0} GB)",
                    tr.t("DEPOLAMA BİRİMLERİ (M.2 / NVMe / SATA)", "STORAGE DRIVES (M.2 / NVMe / SATA)"),
                    snap.storage.total_gb,
                    tr.t("Boş", "Free"),
                    snap.storage.free_gb
                ))
                .strong()
                .size(11.0)
                .color(secondary),
            );
            ui.add_space(4.0);
            if snap.storage.disks.is_empty() {
                ui.label(RichText::new(tr.t("Fiziksel disk bilgisi okunamadı.", "Physical disk details unavailable.")).size(11.0));
            } else {
                for d in &snap.storage.disks {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("• {}", d.model)).strong().size(11.5).color(Color32::from_rgb(255, 241, 235)));
                        egui::Frame::none()
                            .fill(Color32::from_rgb(70, 28, 42))
                            .stroke(Stroke::new(1.0_f32, primary))
                            .rounding(Rounding::same(5.0))
                            .inner_margin(egui::Margin::symmetric(6.0, 1.0))
                            .show(ui, |ui| {
                                ui.label(RichText::new(&d.interface_label).strong().size(10.0).color(Color32::WHITE));
                            });
                        ui.label(
                            RichText::new(format!(
                                "{}  •  {:.0} GB  •  {}",
                                d.media_type,
                                d.total_gb,
                                d.temp_text()
                            ))
                            .size(11.0)
                            .color(Color32::from_rgb(203, 213, 225)),
                        );
                    });
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "OS: {}  •  {}: {}",
                    snap.os_name,
                    tr.t("Yetki", "Privilege"),
                    if snap.is_admin { tr.t("Yönetici", "Administrator") } else { tr.t("Standart", "Standard") }
                ))
                .size(10.0)
                .color(Color32::GRAY),
            );
        });
    }

    /// Fixed-width 4-column Processes list with RIGHT-CLICK context menu (Open Location, Copy Path, Terminate).
    fn render_tab_processes(
        &mut self,
        ui: &mut egui::Ui,
        snap: &HardwareSnapshot,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) {
        ui.horizontal(|ui| {
            ui.label(tr.t("Ara:", "Search:"));
            ui.add(egui::TextEdit::singleline(&mut self.proc_search).desired_width(150.0));
            ui.checkbox(&mut self.proc_group, tr.t("Grupla", "Group"));
            egui::ComboBox::from_id_salt("proc_sort")
                .selected_text(match self.proc_sort {
                    ProcSort::Cpu => "CPU %",
                    ProcSort::Ram => "RAM",
                    ProcSort::Disk => "Disk",
                    ProcSort::Name => tr.t("Ad", "Name"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.proc_sort, ProcSort::Cpu, "CPU %");
                    ui.selectable_value(&mut self.proc_sort, ProcSort::Ram, "RAM");
                    ui.selectable_value(&mut self.proc_sort, ProcSort::Disk, "Disk");
                    ui.selectable_value(&mut self.proc_sort, ProcSort::Name, tr.t("Ad", "Name"));
                });
        });

        ui.label(
            RichText::new(tr.t(
                "İpucu: Bir uygulamaya sağ tıklayarak dosya konumunu açabilir, yolunu kopyalayabilir veya görevi sonlandırabilirsiniz.",
                "Tip: Right-click any process to open file location, copy path, or terminate.",
            ))
            .size(10.0)
            .color(Color32::from_rgb(155, 168, 185)),
        );
        ui.add_space(4.0);

        let q = self.proc_search.trim().to_lowercase();
        let mut list: Vec<_> = snap
            .processes
            .iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
            .collect();

        match self.proc_sort {
            ProcSort::Cpu => list.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent)),
            ProcSort::Ram => list.sort_by(|a, b| b.ram_mb.total_cmp(&a.ram_mb)),
            ProcSort::Disk => list.sort_by(|a, b| b.disk_mbps.total_cmp(&a.disk_mbps)),
            ProcSort::Name => list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        }

        // Reserve room for Frame horizontal margins (16px) + ScrollArea bar (14px) + 3 column gaps (18px)
        let avail_w = (ui.available_width() - 14.0).max(340.0);
        let inner_w = (avail_w - 16.0).max(320.0);
        let col_cpu = 58.0;
        let col_ram = 76.0;
        let col_disk = 74.0;
        let col_name = (inner_w - col_cpu - col_ram - col_disk - 18.0).max(110.0);

        // Table Header
        egui::Frame::none()
            .fill(Color32::from_rgb(46, 26, 38))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.set_width(inner_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add_sized([col_name, 16.0], egui::Label::new(RichText::new(tr.t("Uygulama", "Process")).strong().size(11.0).color(secondary)).truncate());
                    ui.add_sized([col_cpu, 16.0], egui::Label::new(RichText::new("CPU").strong().size(11.0).color(secondary)));
                    ui.add_sized([col_ram, 16.0], egui::Label::new(RichText::new("RAM").strong().size(11.0).color(secondary)));
                    ui.add_sized([col_disk, 16.0], egui::Label::new(RichText::new("Disk").strong().size(11.0).color(secondary)));
                });
            });

        ui.add_space(2.0);

        for (idx, p) in list.into_iter().take(90).enumerate() {
            let name = if p.instance_count > 1 {
                format!("{} ({})", p.name, p.instance_count)
            } else {
                p.name.clone()
            };
            let row_bg = if idx % 2 == 0 {
                Color32::from_rgb(30, 24, 34)
            } else {
                Color32::from_rgb(24, 19, 28)
            };

            let row_resp = egui::Frame::none()
                .fill(row_bg)
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                .show(ui, |ui| {
                    ui.set_width(inner_w);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_sized(
                            [col_name, 16.0],
                            egui::Label::new(RichText::new(&name).size(11.0).color(Color32::from_rgb(240, 232, 230)))
                                .truncate(),
                        );
                        ui.add_sized(
                            [col_cpu, 16.0],
                            egui::Label::new(RichText::new(format!("{:.1}%", p.cpu_percent)).size(11.0)),
                        );
                        ui.add_sized(
                            [col_ram, 16.0],
                            egui::Label::new(RichText::new(p.ram_text()).size(11.0)),
                        );
                        ui.add_sized(
                            [col_disk, 16.0],
                            egui::Label::new(RichText::new(format!("{:.1} MB/s", p.disk_mbps)).size(11.0)),
                        );
                    });
                })
                .response;

            // Make entire row right-clickable for context menu
            let click_resp = ui.interact(
                row_resp.rect,
                egui::Id::new(("proc_row", p.pid, idx)),
                egui::Sense::click(),
            );
            let pid = p.pid;
            let exe_path = p.exe_path.clone();
            let tr_open = tr.t("Dosya Konumunu Aç", "Open File Location").to_string();
            let tr_copy = tr.t("Dosya Yolunu Kopyala", "Copy File Path").to_string();
            let tr_kill = tr.t("Görevi Sonlandır", "Terminate Process").to_string();

            click_resp.context_menu(|ui| {
                ui.set_min_width(170.0);
                ui.label(RichText::new(&name).strong().color(secondary));
                ui.separator();
                if ui
                    .add_enabled(!exe_path.is_empty(), egui::Button::new(tr_open))
                    .clicked()
                {
                    open_file_location(&exe_path);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(!exe_path.is_empty(), egui::Button::new(tr_copy))
                    .clicked()
                {
                    ui.output_mut(|o| o.copied_text = exe_path.clone());
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .button(RichText::new(tr_kill).strong().color(primary))
                    .clicked()
                {
                    let _ = kill_process_by_pid(pid);
                    ui.close_menu();
                }
            });
        }
    }

    fn render_tab_settings(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) {
        glass_card(ui, secondary, |ui| {
            ui.label(
                RichText::new(tr.t("KURULUM & TAŞINABİLİR (PORTABLE) MOD", "SYSTEM INSTALLATION & PORTABLE MODE"))
                    .strong()
                    .size(11.0)
                    .color(secondary),
            );
            let mode_txt = if self.is_installed {
                tr.t("Durum: Sisteme Kurulu (Uygulamalar & Başlat Menüsünde görünür)", "Status: Installed (Registered in OS Apps & Start Menu)")
            } else if self.is_portable {
                tr.t("Durum: Taşınabilir (Portable) Mod", "Status: Portable Mode")
            } else {
                tr.t("Durum: Taşınabilir Çalışıyor (Henüz Sisteme Kurulmadı)", "Status: Running Portable (Not Installed to OS Apps)")
            };
            ui.label(RichText::new(mode_txt).size(11.0));
            ui.checkbox(
                &mut self.install_desktop_shortcut,
                tr.t("Masaüstü kısayolu oluştur", "Create Desktop shortcut"),
            );
            ui.horizontal(|ui| {
                if ui
                    .button(tr.t("Sisteme Kur (Uygulamalara Ekle)", "Install to OS Apps"))
                    .clicked()
                {
                    match install_current_to_system(self.install_desktop_shortcut, tr.en) {
                        Ok(msg) => {
                            self.install_status_msg = msg;
                            self.is_installed = true;
                        }
                        Err(e) => self.install_status_msg = e,
                    }
                }
                if self.is_installed
                    && ui
                        .button(RichText::new(tr.t("Sistemden Kaldır", "Uninstall")).color(primary))
                        .clicked()
                {
                    match uninstall_from_system(tr.en) {
                        Ok(msg) => {
                            self.install_status_msg = msg;
                            self.is_installed = false;
                        }
                        Err(e) => self.install_status_msg = e,
                    }
                }
            });
            if !self.install_status_msg.is_empty() {
                ui.label(RichText::new(&self.install_status_msg).size(10.5).color(secondary));
            }
        });

        ui.add_space(6.0);

        glass_card(ui, secondary, |ui| {
            ui.label(RichText::new(tr.t("GÖRÜNÜM & BAĞIMSIZ ŞEFFAFLIK", "APPEARANCE & INDEPENDENT OPACITY")).strong().size(11.0).color(secondary));
            ui.horizontal(|ui| {
                ui.label(tr.t("Dil / Language:", "Language / Dil:"));
                if ui.selectable_label(self.settings.language == "tr", "Türkçe").clicked() {
                    self.settings.language = "tr".to_string();
                }
                if ui.selectable_label(self.settings.language == "en", "English").clicked() {
                    self.settings.language = "en".to_string();
                }
            });

            ui.add(
                egui::Slider::new(&mut self.settings.widget_opacity, 0.25..=1.0)
                    .text(tr.t("Widget Opaklığı", "Widget Opacity")),
            );
            ui.add(
                egui::Slider::new(&mut self.settings.window_opacity, 0.35..=1.0)
                    .text(tr.t("Pencere Opaklığı", "Window Opacity")),
            );
            ui.label(
                RichText::new(tr.t(
                    "Not: Özet modu (Ctrl+Enter) her zaman tam şeffaftır ve diğer opaklık ayarlarını etkilemez.",
                    "Note: Summary OSD (Ctrl+Enter) is always fully transparent and independent of Widget/Window opacity.",
                ))
                .size(10.0)
                .color(Color32::GRAY),
            );

            if ui
                .add(egui::Slider::new(&mut self.settings.ui_scale, 0.85..=1.35).text(tr.t("Ölçek", "UI Scale")))
                .changed()
            {
                self.apply_window_geometry(ctx);
            }

            if ui
                .add(egui::Slider::new(&mut self.settings.refresh_ms, 500..=5000).text(tr.t("Yenileme (ms)", "Refresh (ms)")))
                .changed()
            {
                self.telemetry
                    .refresh_ms
                    .store(self.settings.refresh_ms, Ordering::Relaxed);
            }

            ui.horizontal_wrapped(|ui| {
                ui.label(tr.t("Temalar:", "Themes:"));
                let presets = [
                    ("Nar Çiçeği", "#FF4D6D", "#FFB085", "#140E16"),
                    ("Cyber Cyan", "#00E5FF", "#7C4DFF", "#0B101B"),
                    ("Emerald", "#10B981", "#6EE7B7", "#0B1714"),
                    ("Amber Gold", "#F59E0B", "#FDE68A", "#1A140B"),
                    ("Monochrome", "#F8FAFC", "#94A3B8", "#0F1117"),
                ];
                for (name, p, s, b) in presets {
                    if ui.small_button(name).clicked() {
                        self.settings.primary_hex = p.to_string();
                        self.settings.secondary_hex = s.to_string();
                        self.settings.bg_hex = b.to_string();
                    }
                }
            });

            if ui
                .checkbox(&mut self.autostart_enabled, tr.t("Sistem açılışında otomatik başlat", "Launch automatically on startup"))
                .changed()
            {
                let _ = set_autostart(self.autostart_enabled);
            }
        });

        ui.add_space(6.0);

        glass_card(ui, secondary, |ui| {
            ui.label(
                RichText::new(tr.t("ÖZET MODU (CTRL+ENTER) DEĞERLERİ", "SUMMARY OSD (CTRL+ENTER) METRICS"))
                    .strong()
                    .size(11.0)
                    .color(secondary),
            );
            ui.columns(2, |cols| {
                cols[0].checkbox(&mut self.settings.sum_cpu_pct, "CPU %");
                cols[0].checkbox(&mut self.settings.sum_cpu_temp, "CPU °C");
                cols[0].checkbox(&mut self.settings.sum_gpu_pct, "GPU %");
                cols[0].checkbox(&mut self.settings.sum_gpu_temp, "GPU °C");
                cols[0].checkbox(&mut self.settings.sum_igpu_pct, "iGPU %");
                cols[1].checkbox(&mut self.settings.sum_ram_pct, "RAM %");
                cols[1].checkbox(&mut self.settings.sum_ssd_temp, "SSD °C");
                cols[1].checkbox(&mut self.settings.sum_gpu_watts, tr.t("GPU Watt (W)", "GPU Watts (W)"));
                cols[1].checkbox(&mut self.settings.sum_device_watts, tr.t("Cihaz Watt (W)", "Device Watts (W)"));
                cols[1].checkbox(&mut self.settings.sum_fps, "FPS (DXGI / RTSS)");
            });
        });

        ui.add_space(6.0);

        glass_card(ui, secondary, |ui| {
            ui.label(RichText::new(tr.t("UYARI EŞİKLERİ", "ALERT THRESHOLDS")).strong().size(11.0).color(secondary));
            let mut changed = false;
            changed |= ui.add(egui::Slider::new(&mut self.settings.cpu_temp_thresh, 60.0..=105.0).text("CPU °C")).changed();
            changed |= ui.add(egui::Slider::new(&mut self.settings.gpu_temp_thresh, 60.0..=100.0).text("GPU °C")).changed();
            changed |= ui.add(egui::Slider::new(&mut self.settings.cpu_use_thresh, 50.0..=100.0).text("CPU %")).changed();
            changed |= ui.add(egui::Slider::new(&mut self.settings.ram_use_thresh, 50.0..=100.0).text("RAM %")).changed();
            if changed {
                if let Ok(mut g) = self.telemetry.thresholds.write() {
                    *g = (
                        self.settings.cpu_temp_thresh,
                        self.settings.gpu_temp_thresh,
                        self.settings.cpu_use_thresh,
                        self.settings.ram_use_thresh,
                    );
                }
            }
            if ui.button(tr.t("Ayarları Kaydet", "Save Settings")).clicked() {
                save_settings(&self.settings);
            }
        });
    }

    fn render_tab_updates(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        tr: &I18n,
        primary: Color32,
        secondary: Color32,
    ) {
        let is_checking = self.checking_updates.load(Ordering::Relaxed);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!is_checking, egui::Button::new(tr.t("Şimdi Denetle", "Check Now")))
                .clicked()
            {
                self.trigger_update_check(ctx);
            }
            ui.label(
                RichText::new(tr.t(
                    "Açılışta ve 6 saatte bir otomatik denetlenir.",
                    "Checked automatically at startup and every 6 hours.",
                ))
                .size(10.5)
                .color(Color32::GRAY),
            );
        });

        ui.add_space(6.0);
        let res_opt = self.update_result.read().ok().and_then(|g| g.clone());
        let action_msg = self
            .update_action_msg
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        if !action_msg.is_empty() {
            ui.label(RichText::new(action_msg).color(secondary).strong());
            ui.add_space(4.0);
        }

        if let Some(res) = res_opt {
            glass_card(ui, secondary, |ui| {
                ui.label(
                    RichText::new(format!("PulseGlass v{} ({})", APP_VERSION, res.checked_at))
                        .strong()
                        .color(secondary),
                );
                ui.label(&res.app_status_msg);
                if res.app_update_available && !res.app_download_url.is_empty() {
                    if ui
                        .button(RichText::new(tr.t("Onayla ve PulseGlass Güncellemesini Kur", "Confirm & Install PulseGlass Update")).color(primary))
                        .clicked()
                    {
                        let url = res.app_download_url.clone();
                        let en = tr.en;
                        let msg_arc = Arc::clone(&self.update_action_msg);
                        let ctx_bg = ctx.clone();
                        std::thread::spawn(move || {
                            let res_msg = match download_and_apply_app_update(&url, en) {
                                Ok(m) => {
                                    ctx_bg.send_viewport_cmd(ViewportCommand::Close);
                                    m
                                }
                                Err(e) => e,
                            };
                            if let Ok(mut g) = msg_arc.write() {
                                *g = res_msg;
                            }
                            ctx_bg.request_repaint();
                        });
                    }
                }
            });

            ui.add_space(6.0);

            glass_card(ui, secondary, |ui| {
                ui.label(
                    RichText::new(tr.t("SİSTEM GÜNCELLEMELERİ (ÖNEMLİ & İSTEĞE BAĞLI)", "SYSTEM UPDATES (IMPORTANT & OPTIONAL)"))
                        .strong()
                        .size(11.0)
                        .color(secondary),
                );
                ui.label(&res.os_status_msg);

                if !res.important_updates.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr.t("Önemli Güncellemeler:", "Important Updates:")).strong().color(primary));
                    for u in &res.important_updates {
                        ui.label(RichText::new(format!("• {} ({:.1} MB)", u.title, u.size_mb)).size(11.0));
                    }
                    if ui
                        .button(tr.t("Onayla ve Önemli Güncellemeleri Kur", "Confirm & Install Important Updates"))
                        .clicked()
                    {
                        let en = tr.en;
                        let msg_arc = Arc::clone(&self.update_action_msg);
                        let ctx_bg = ctx.clone();
                        std::thread::spawn(move || {
                            let m = install_windows_updates(false, en);
                            if let Ok(mut g) = msg_arc.write() {
                                *g = m;
                            }
                            ctx_bg.request_repaint();
                        });
                    }
                }

                if !res.optional_updates.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr.t("İsteğe Bağlı Güncellemeler:", "Optional Updates:")).strong().color(secondary));
                    for u in &res.optional_updates {
                        ui.label(RichText::new(format!("• {} ({:.1} MB)", u.title, u.size_mb)).size(11.0));
                    }
                    if ui
                        .button(tr.t("Onayla ve İsteğe Bağlı Güncellemeleri de Kur", "Confirm & Install Optional Updates Too"))
                        .clicked()
                    {
                        let en = tr.en;
                        let msg_arc = Arc::clone(&self.update_action_msg);
                        let ctx_bg = ctx.clone();
                        std::thread::spawn(move || {
                            let m = install_windows_updates(true, en);
                            if let Ok(mut g) = msg_arc.write() {
                                *g = m;
                            }
                            ctx_bg.request_repaint();
                        });
                    }
                }
            });
        } else {
            ui.label(tr.t("Güncellemeler denetleniyor...", "Checking for updates..."));
        }
    }
}

pub fn load_system_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates: &[&str] = if cfg!(windows) {
        &[
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\arial.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ]
    };

    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("system_ui".to_owned(), egui::FontData::from_owned(bytes));
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                fam.insert(0, "system_ui".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

fn configure_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = Color32::from_rgb(24, 18, 28);
    style.visuals.panel_fill = Color32::TRANSPARENT;
    style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    style.visuals.override_text_color = Some(Color32::from_rgb(245, 232, 230));
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 32, 44);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(76, 32, 48);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(108, 38, 60);
    style.visuals.widgets.inactive.rounding = Rounding::same(6.0);
    style.visuals.widgets.hovered.rounding = Rounding::same(6.0);
    style.visuals.widgets.active.rounding = Rounding::same(6.0);
    ctx.set_style(style);
}
