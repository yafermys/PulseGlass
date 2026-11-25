#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod installer;

use egui::{Color32, RichText, Rounding, Stroke};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

#[cfg(feature = "embed-app")]
const EMBEDDED_APP: &[u8] = include_bytes!("../target/release/PulseGlass.exe");

fn load_icon() -> Option<Arc<egui::IconData>> {
    let bytes = include_bytes!("../Assets/app.png");
    let img = image::load_from_memory(bytes)
        .ok()?
        .resize(64, 64, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let (width, height) = img.dimensions();
    Some(Arc::new(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }))
}

fn load_icon_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let bytes = include_bytes!("../Assets/app.png");
    let img = image::load_from_memory(bytes)
        .ok()?
        .resize(64, 64, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let (w, h) = img.dimensions();
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img.into_raw());
    Some(ctx.load_texture("setup_app_icon", color_image, egui::TextureOptions::LINEAR))
}

#[cfg(windows)]
fn apply_win32_window_icon(window_title: &str) -> bool {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, LoadImageW, SendMessageW, SetClassLongPtrW, GCLP_HICON, GCLP_HICONSM,
        ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, WM_SETICON,
    };
    let title_w: Vec<u16> = window_title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title_w.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        let hinst = GetModuleHandleW(std::ptr::null());
        let icon_id = 1usize as *const u16;
        let h_big = LoadImageW(hinst, icon_id, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED);
        let h_small = LoadImageW(hinst, icon_id, IMAGE_ICON, 16, 16, LR_SHARED);
        if !h_big.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, h_big as isize);
            SetClassLongPtrW(hwnd, GCLP_HICON, h_big as isize);
        }
        if !h_small.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, h_small as isize);
            SetClassLongPtrW(hwnd, GCLP_HICONSM, h_small as isize);
        }
        !h_big.is_null() || !h_small.is_null()
    }
}

fn locate_or_extract_payload() -> Result<PathBuf, String> {
    #[cfg(feature = "embed-app")]
    {
        let out = std::env::temp_dir().join("PulseGlass_Setup_Payload.exe");
        fs::write(&out, EMBEDDED_APP).map_err(|e| format!("Payload extract error: {}", e))?;
        return Ok(out);
    }
    #[cfg(not(feature = "embed-app"))]
    {
        let cur = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = cur.parent().ok_or("No parent dir")?;
        let sibling = dir.join(if cfg!(windows) {
            "PulseGlass.exe"
        } else {
            "PulseGlass"
        });
        if sibling.exists() {
            Ok(sibling)
        } else {
            Err("PulseGlass.exe not found next to Setup.".to_string())
        }
    }
}

struct SetupApp {
    en: bool,
    desktop_shortcut: bool,
    autostart: bool,
    launch_after: bool,
    status: String,
    done: bool,
    icon_tex: Option<egui::TextureHandle>,
    win_icon_applied: bool,
    frame_count: u32,
}

impl SetupApp {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            en: false,
            desktop_shortcut: true,
            autostart: false,
            launch_after: true,
            status: String::new(),
            done: false,
            icon_tex: load_icon_texture(ctx),
            win_icon_applied: false,
            frame_count: 0,
        }
    }
}

impl eframe::App for SetupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame_count = self.frame_count.saturating_add(1);
        #[cfg(windows)]
        if !self.win_icon_applied && self.frame_count <= 20 {
            if apply_win32_window_icon("PulseGlass Setup") {
                self.win_icon_applied = true;
            }
        }

        let primary = Color32::from_rgb(255, 77, 109);
        let secondary = Color32::from_rgb(255, 176, 133);
        let bg = Color32::from_rgb(18, 15, 24);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg).inner_margin(egui::Margin::same(18.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(ref tex) = self.icon_tex {
                        ui.add(
                            egui::Image::new((tex.id(), egui::vec2(24.0, 24.0)))
                                .rounding(Rounding::same(5.0)),
                        );
                    }
                    ui.label(
                        RichText::new(format!("PulseGlass v{} Kurulum / Setup", installer::APP_VERSION))
                            .strong()
                            .size(18.0)
                            .color(primary),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.selectable_label(self.en, "EN").clicked() {
                            self.en = true;
                        }
                        if ui.selectable_label(!self.en, "TR").clicked() {
                            self.en = false;
                        }
                    });
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(if self.en {
                        "Choose how you want to use PulseGlass: install it into Windows Installed Apps & Start Menu, or extract/run in Portable mode."
                    } else {
                        "PulseGlass'ı nasıl kullanmak istediğinizi seçin: Windows Uygulamalar listesine & Başlat Menüsüne kurabilir veya Taşınabilir (Portable) olarak çalıştırabilirsiniz."
                    })
                    .size(12.0)
                    .color(Color32::from_rgb(220, 225, 235)),
                );

                ui.add_space(12.0);
                egui::Frame::none()
                    .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 14))
                    .stroke(Stroke::new(1.0_f32, secondary))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.checkbox(
                            &mut self.desktop_shortcut,
                            if self.en { "Create Desktop shortcut" } else { "Masaüstü kısayolu oluştur" },
                        );
                        ui.checkbox(
                            &mut self.autostart,
                            if self.en { "Launch automatically at startup" } else { "Sistem açılışında otomatik başlat" },
                        );
                        ui.checkbox(
                            &mut self.launch_after,
                            if self.en { "Launch PulseGlass after installation" } else { "Kurulumdan sonra PulseGlass'ı başlat" },
                        );
                    });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [215.0, 36.0],
                            egui::Button::new(
                                RichText::new(if self.en {
                                    "Install to System Apps"
                                } else {
                                    "Sisteme Kur (Uygulamalar)"
                                })
                                .strong()
                                .color(Color32::WHITE),
                            )
                            .fill(primary),
                        )
                        .clicked()
                    {
                        match locate_or_extract_payload()
                            .and_then(|p| installer::install_binary_to_system(&p, self.desktop_shortcut, self.en))
                        {
                            Ok(msg) => {
                                if self.autostart {
                                    let _ = installer::set_autostart(true);
                                }
                                self.status = msg;
                                self.done = true;
                                if self.launch_after {
                                    #[cfg(windows)]
                                    {
                                        let pf = PathBuf::from(r"C:\Program Files\PulseGlass\PulseGlass.exe");
                                        let local = dirs::data_local_dir()
                                            .unwrap_or_default()
                                            .join(r"Programs\PulseGlass\PulseGlass.exe");
                                        let target = if pf.exists() { pf } else { local };
                                        let _ = Command::new(target).spawn();
                                    }
                                }
                            }
                            Err(e) => self.status = e,
                        }
                    }

                    if ui
                        .add_sized(
                            [195.0, 36.0],
                            egui::Button::new(
                                RichText::new(if self.en {
                                    "Extract Portable Here"
                                } else {
                                    "Buraya Taşınabilir Çıkar"
                                })
                                .strong(),
                            ),
                        )
                        .clicked()
                    {
                        match locate_or_extract_payload() {
                            Ok(src) => {
                                let dest_dir = std::env::current_exe()
                                    .ok()
                                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                                    .unwrap_or_else(|| PathBuf::from("."));
                                let dest = dest_dir.join(if cfg!(windows) {
                                    "PulseGlass.exe"
                                } else {
                                    "pulseglass"
                                });
                                let res = if src.canonicalize().ok() != dest.canonicalize().ok() {
                                    fs::copy(&src, &dest).map(|_| ())
                                } else {
                                    Ok(())
                                };
                                match res {
                                    Ok(_) => {
                                        let _ = fs::write(dest_dir.join("portable.flag"), "1");
                                        self.status = if self.en {
                                            format!("Portable ready: {}", dest.display())
                                        } else {
                                            format!("Taşınabilir sürüm hazır: {}", dest.display())
                                        };
                                        self.done = true;
                                        if self.launch_after {
                                            let _ = Command::new(&dest).arg("--portable").spawn();
                                        }
                                    }
                                    Err(e) => self.status = e.to_string(),
                                }
                            }
                            Err(e) => self.status = e,
                        }
                    }
                });

                if !self.status.is_empty() {
                    ui.add_space(10.0);
                    ui.label(RichText::new(&self.status).color(secondary).size(11.5));
                }

                if self.done {
                    ui.add_space(8.0);
                    if ui.button(if self.en { "Close Setup" } else { "Kurulumu Kapat" }).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });
    }
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PulseGlass Setup")
        .with_inner_size([460.0, 290.0])
        .with_resizable(false);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "PulseGlass Setup",
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            for path in [
                r"C:\Windows\Fonts\segoeui.ttf",
                r"C:\Windows\Fonts\arial.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ] {
                if let Ok(bytes) = std::fs::read(path) {
                    fonts
                        .font_data
                        .insert("sys_ui".to_owned(), egui::FontData::from_owned(bytes));
                    if let Some(f) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                        f.insert(0, "sys_ui".to_owned());
                    }
                    break;
                }
            }
            cc.egui_ctx.set_fonts(fonts);
            Ok(Box::new(SetupApp::new(&cc.egui_ctx)))
        }),
    )
}
