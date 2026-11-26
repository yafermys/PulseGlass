use egui::{Color32, Pos2, Rect, Rounding, TextureHandle};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub struct ThemePreset {
    pub name_tr: &'static str,
    pub name_en: &'static str,
    pub primary: &'static str,
    pub secondary: &'static str,
    pub bg: &'static str,
    pub card: &'static str,
    pub text: &'static str,
}

pub const THEME_PRESETS: &[ThemePreset] = &[
    ThemePreset {
        name_tr: "Nar Çiçeği",
        name_en: "Pomegranate",
        primary: "#FF4D6D",
        secondary: "#FFB085",
        bg: "#140E16",
        card: "#221B26",
        text: "#FFF1EB",
    },
    ThemePreset {
        name_tr: "Cyber Cyan",
        name_en: "Cyber Cyan",
        primary: "#00E5FF",
        secondary: "#7C4DFF",
        bg: "#0B101B",
        card: "#141D2E",
        text: "#E6F7FF",
    },
    ThemePreset {
        name_tr: "Zümrüt Matrix",
        name_en: "Emerald Matrix",
        primary: "#10B981",
        secondary: "#6EE7B7",
        bg: "#091512",
        card: "#132620",
        text: "#ECFDF5",
    },
    ThemePreset {
        name_tr: "Kehribar Altın",
        name_en: "Amber Gold",
        primary: "#F59E0B",
        secondary: "#FDE68A",
        bg: "#17120A",
        card: "#261E12",
        text: "#FFFBEB",
    },
    ThemePreset {
        name_tr: "Gece Ametist",
        name_en: "Midnight Amethyst",
        primary: "#C084FC",
        secondary: "#F472B6",
        bg: "#130D1E",
        card: "#211733",
        text: "#FAF5FF",
    },
    ThemePreset {
        name_tr: "Sakura Neon",
        name_en: "Sakura Neon",
        primary: "#FF66B2",
        secondary: "#FFC2E2",
        bg: "#1A0E17",
        card: "#2B1827",
        text: "#FFF0F7",
    },
    ThemePreset {
        name_tr: "Arktik Buz",
        name_en: "Arctic Frost",
        primary: "#38BDF8",
        secondary: "#BAE6FD",
        bg: "#0B131E",
        card: "#152334",
        text: "#F0F9FF",
    },
    ThemePreset {
        name_tr: "Kızıl Obsidyen",
        name_en: "Crimson Obsidian",
        primary: "#EF4444",
        secondary: "#FCA5A5",
        bg: "#140A0C",
        card: "#241316",
        text: "#FEF2F2",
    },
    ThemePreset {
        name_tr: "Gün Batımı",
        name_en: "Solar Sunset",
        primary: "#FB7185",
        secondary: "#FDBA74",
        bg: "#1A0F12",
        card: "#2B191E",
        text: "#FFF5F0",
    },
    ThemePreset {
        name_tr: "Derin Okyanus",
        name_en: "Deep Ocean",
        primary: "#2DD4BF",
        secondary: "#60A5FA",
        bg: "#08141C",
        card: "#102330",
        text: "#ECFEFF",
    },
    ThemePreset {
        name_tr: "Orman Yosunu",
        name_en: "Forest Moss",
        primary: "#84CC16",
        secondary: "#BEF264",
        bg: "#0F160B",
        card: "#1B2614",
        text: "#F7FEE7",
    },
    ThemePreset {
        name_tr: "Monokrom",
        name_en: "Monochrome",
        primary: "#F8FAFC",
        secondary: "#94A3B8",
        bg: "#0F1117",
        card: "#1B1F2A",
        text: "#F8FAFC",
    },
];

#[derive(Clone, Debug)]
pub struct ExtractedPalette {
    pub primary_hex: String,
    pub secondary_hex: String,
    pub bg_hex: String,
    pub card_hex: String,
    pub text_hex: String,
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) * 0.5;
    if (max - min).abs() < 1e-5 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let mut h = if (max - rf).abs() < 1e-5 {
        (gf - bf) / d + if gf < bf { 6.0 } else { 0.0 }
    } else if (max - gf).abs() < 1e-5 {
        (bf - rf) / d + 2.0
    } else {
        (rf - gf) / d + 4.0
    };
    h /= 6.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(1.0);
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    if s < 1e-5 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue2rgb = |mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            return p + (q - p) * 6.0 * t;
        }
        if t < 0.5 {
            return q;
        }
        if t < 2.0 / 3.0 {
            return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
        }
        p
    };
    (
        (hue2rgb(h + 1.0 / 3.0) * 255.0).round() as u8,
        (hue2rgb(h) * 255.0).round() as u8,
        (hue2rgb(h - 1.0 / 3.0) * 255.0).round() as u8,
    )
}

fn to_hex(rgb: (u8, u8, u8)) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2)
}

/// Extracts a harmonious, high-contrast PulseGlass theme palette from raw RGBA image pixels.
pub fn extract_theme_from_rgba(rgba: &[u8]) -> ExtractedPalette {
    // 24 hue buckets weighted by saturation & mid-luminance
    let mut bucket_w = [0.0f64; 24];
    let mut bucket_h = [0.0f64; 24];
    let mut bucket_s = [0.0f64; 24];
    let mut avg_r = 0.0f64;
    let mut avg_g = 0.0f64;
    let mut avg_b = 0.0f64;
    let mut total_n = 0.0f64;

    let step = (rgba.len() / (4 * 4000)).max(1);
    for px in rgba.chunks_exact(4).step_by(step) {
        if px[3] < 40 {
            continue;
        }
        let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
        avg_r += px[0] as f64;
        avg_g += px[1] as f64;
        avg_b += px[2] as f64;
        total_n += 1.0;

        if s > 0.16 && l > 0.12 && l < 0.90 {
            let b = ((h * 24.0) as usize).min(23);
            let w = (s as f64).powi(2) * (1.0 - ((l - 0.55).abs() as f64));
            bucket_w[b] += w;
            bucket_h[b] += (h as f64) * w;
            bucket_s[b] += (s as f64) * w;
        }
    }

    let (best_b, &best_w) = bucket_w
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap_or((0, &0.0));

    let (primary_h, primary_s) = if best_w > 0.01 {
        (
            (bucket_h[best_b] / best_w) as f32,
            ((bucket_s[best_b] / best_w) as f32).clamp(0.55, 0.92),
        )
    } else if total_n > 0.0 {
        let (h, s, _) = rgb_to_hsl(
            (avg_r / total_n) as u8,
            (avg_g / total_n) as u8,
            (avg_b / total_n) as u8,
        );
        (h, s.clamp(0.15, 0.75))
    } else {
        (0.96, 0.85)
    };

    // Secondary hue: pick second distinct peak if vibrant, else analogous +32°
    let mut second_b = None;
    let mut second_w = 0.0f64;
    for (i, &w) in bucket_w.iter().enumerate() {
        let dist = (i as i32 - best_b as i32).abs();
        let wrap_dist = dist.min(24 - dist);
        if wrap_dist >= 2 && w > second_w {
            second_w = w;
            second_b = Some(i);
        }
    }
    let secondary_h = if let Some(sb) = second_b {
        if second_w > best_w * 0.25 {
            (bucket_h[sb] / second_w) as f32
        } else {
            (primary_h + 0.09).rem_euclid(1.0)
        }
    } else {
        (primary_h + 0.09).rem_euclid(1.0)
    };

    let primary_rgb = hsl_to_rgb(primary_h, primary_s.max(0.60), 0.60);
    let secondary_rgb = hsl_to_rgb(secondary_h, (primary_s * 0.85).clamp(0.45, 0.85), 0.75);
    let bg_rgb = hsl_to_rgb(primary_h, (primary_s * 0.35).min(0.32), 0.075);
    let card_rgb = hsl_to_rgb(primary_h, (primary_s * 0.32).min(0.28), 0.13);
    let text_rgb = hsl_to_rgb(secondary_h, 0.22, 0.96);

    ExtractedPalette {
        primary_hex: to_hex(primary_rgb),
        secondary_hex: to_hex(secondary_rgb),
        bg_hex: to_hex(bg_rgb),
        card_hex: to_hex(card_rgb),
        text_hex: to_hex(text_rgb),
    }
}

struct MediaFrame {
    tex: TextureHandle,
    delay: Duration,
}

pub struct BackgroundMediaPlayer {
    pub loaded_path: String,
    frames: Vec<MediaFrame>,
    current_idx: usize,
    last_advance: Instant,
    pub aspect: f32,
    pub first_frame_rgba: Option<Vec<u8>>,
    pub status_msg: String,
}

impl Default for BackgroundMediaPlayer {
    fn default() -> Self {
        Self {
            loaded_path: String::new(),
            frames: Vec::new(),
            current_idx: 0,
            last_advance: Instant::now(),
            aspect: 16.0 / 9.0,
            first_frame_rgba: None,
            status_msg: String::new(),
        }
    }
}

impl BackgroundMediaPlayer {
    pub fn clear(&mut self) {
        self.loaded_path.clear();
        self.frames.clear();
        self.current_idx = 0;
        self.first_frame_rgba = None;
        self.status_msg.clear();
    }

    pub fn is_loaded(&self) -> bool {
        !self.frames.is_empty()
    }

    #[allow(dead_code)]
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Loads static image (PNG/JPG/WEBP/BMP), animated GIF, or Video (MP4/WEBM/MKV/MOV via ffmpeg) into GPU textures.
    pub fn load_if_changed(&mut self, ctx: &egui::Context, path: &str, tag: &str, en: bool) -> bool {
        let trimmed = path.trim();
        if trimmed == self.loaded_path {
            return false;
        }
        self.clear();
        self.loaded_path = trimmed.to_string();
        if trimmed.is_empty() {
            return false;
        }

        let p = Path::new(trimmed);
        if !p.exists() {
            self.status_msg = if en {
                "Media file not found.".to_string()
            } else {
                "Medya dosyası bulunamadı.".to_string()
            };
            return false;
        }

        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if ext == "gif" {
            return self.load_gif(ctx, p, tag, en);
        }
        if matches!(ext.as_str(), "mp4" | "webm" | "mkv" | "mov" | "avi" | "wmv") {
            return self.load_video_ffmpeg(ctx, p, tag, en);
        }
        self.load_static_image(ctx, p, tag, en)
    }

    fn push_rgba_frame(
        &mut self,
        ctx: &egui::Context,
        img: image::DynamicImage,
        delay: Duration,
        name: String,
    ) {
        let resized = if img.width() > 1280 || img.height() > 720 {
            img.resize(1280, 720, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgba_img = resized.into_rgba8();
        let (w, h) = rgba_img.dimensions();
        if w == 0 || h == 0 {
            return;
        }
        let raw = rgba_img.into_raw();
        if self.first_frame_rgba.is_none() {
            self.aspect = w as f32 / h as f32;
            self.first_frame_rgba = Some(raw.clone());
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &raw);
        let tex = ctx.load_texture(name, ci, egui::TextureOptions::LINEAR);
        self.frames.push(MediaFrame { tex, delay });
    }

    fn load_static_image(&mut self, ctx: &egui::Context, path: &Path, tag: &str, en: bool) -> bool {
        match image::open(path) {
            Ok(img) => {
                self.push_rgba_frame(
                    ctx,
                    img,
                    Duration::from_secs(3600),
                    format!("pg_bg_{}_0", tag),
                );
                self.status_msg = if en {
                    "Image loaded.".to_string()
                } else {
                    "Görsel yüklendi.".to_string()
                };
                true
            }
            Err(e) => {
                self.status_msg = format!("Görsel okunamadı: {}", e);
                false
            }
        }
    }

    fn load_gif(&mut self, ctx: &egui::Context, path: &Path, tag: &str, en: bool) -> bool {
        use image::AnimationDecoder;
        let Ok(file) = File::open(path) else {
            self.status_msg = "GIF açılamadı.".to_string();
            return false;
        };
        let Ok(decoder) = image::codecs::gif::GifDecoder::new(BufReader::new(file)) else {
            return self.load_static_image(ctx, path, tag, en);
        };
        let Ok(frames) = decoder.into_frames().collect_frames() else {
            return self.load_static_image(ctx, path, tag, en);
        };
        for (i, frame) in frames.into_iter().take(90).enumerate() {
            let (num, den) = frame.delay().numer_denom_ms();
            let ms = if den > 0 { (num / den).max(33) } else { 66 };
            let dyn_img = image::DynamicImage::ImageRgba8(frame.into_buffer());
            self.push_rgba_frame(
                ctx,
                dyn_img,
                Duration::from_millis(ms as u64),
                format!("pg_bg_gif_{}_{}", tag, i),
            );
        }
        if !self.frames.is_empty() {
            self.status_msg = if en {
                format!("Animated GIF loaded ({} frames).", self.frames.len())
            } else {
                format!("Animasyonlu GIF yüklendi ({} kare).", self.frames.len())
            };
            true
        } else {
            false
        }
    }

    fn load_video_ffmpeg(&mut self, ctx: &egui::Context, path: &Path, tag: &str, en: bool) -> bool {
        let out_dir = std::env::temp_dir().join(format!("pulseglass_vid_{}", tag));
        let _ = std::fs::remove_dir_all(&out_dir);
        let _ = std::fs::create_dir_all(&out_dir);
        let pattern = out_dir.join("f_%03d.jpg");

        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args([
            "-y",
            "-t",
            "8",
            "-i",
            &path.display().to_string(),
            "-vf",
            "fps=12,scale='min(960,iw)':-2",
            "-q:v",
            "4",
            &pattern.display().to_string(),
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }

        match cmd.output() {
            Ok(out) if out.status.success() => {
                let mut entries: Vec<_> = std::fs::read_dir(&out_dir)
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.ok().map(|x| x.path()))
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jpg"))
                    .collect();
                entries.sort();
                for (i, fp) in entries.into_iter().take(96).enumerate() {
                    if let Ok(img) = image::open(&fp) {
                        self.push_rgba_frame(
                            ctx,
                            img,
                            Duration::from_millis(83), // ~12 FPS smooth loop
                            format!("pg_bg_vid_{}_{}", tag, i),
                        );
                    }
                }
                let _ = std::fs::remove_dir_all(&out_dir);
                if !self.frames.is_empty() {
                    self.status_msg = if en {
                        format!("Video background loaded ({} frames).", self.frames.len())
                    } else {
                        format!("Video arka plan yüklendi ({} kare).", self.frames.len())
                    };
                    return true;
                }
            }
            _ => {
                let _ = std::fs::remove_dir_all(&out_dir);
            }
        }
        self.status_msg = if en {
            "Video playback requires ffmpeg in PATH (GIF/PNG/JPG/WEBP work built-in).".to_string()
        } else {
            "Video arka plan için sistemde ffmpeg gerekir (GIF/PNG/JPG/WEBP yerleşik çalışır)."
                .to_string()
        };
        false
    }

    pub fn current_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if self.frames.is_empty() {
            return None;
        }
        if self.frames.len() > 1 {
            let delay = self.frames[self.current_idx].delay;
            if self.last_advance.elapsed() >= delay {
                self.current_idx = (self.current_idx + 1) % self.frames.len();
                self.last_advance = Instant::now();
            }
            ctx.request_repaint_after(delay.min(Duration::from_millis(66)));
        }
        Some(&self.frames[self.current_idx].tex)
    }

    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: Rect,
        fit: &str,
        dim_alpha: f32,
        opacity: f32,
        rounding: Rounding,
    ) -> bool {
        let aspect = self.aspect.max(0.1);
        let Some(tex) = self.current_texture(ctx) else {
            return false;
        };
        let tex_id = tex.id();
        let rect_aspect = (rect.width() / rect.height().max(1.0)).max(0.1);

        let (draw_rect, uv) = match fit {
            "stretch" => (rect, Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0))),
            "contain" => {
                if rect_aspect > aspect {
                    let w = rect.height() * aspect;
                    let x0 = rect.center().x - w * 0.5;
                    (
                        Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x0 + w, rect.bottom())),
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    )
                } else {
                    let h = rect.width() / aspect;
                    let y0 = rect.center().y - h * 0.5;
                    (
                        Rect::from_min_max(Pos2::new(rect.left(), y0), Pos2::new(rect.right(), y0 + h)),
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    )
                }
            }
            _ => {
                // Cover: crop UVs cleanly to fill entire rounded frame with zero distortion
                if rect_aspect > aspect {
                    let scale = aspect / rect_aspect;
                    let pad = (1.0 - scale) * 0.5;
                    (
                        rect,
                        Rect::from_min_max(Pos2::new(0.0, pad), Pos2::new(1.0, 1.0 - pad)),
                    )
                } else {
                    let scale = rect_aspect / aspect;
                    let pad = (1.0 - scale) * 0.5;
                    (
                        rect,
                        Rect::from_min_max(Pos2::new(pad, 0.0), Pos2::new(1.0 - pad, 1.0)),
                    )
                }
            }
        };

        let img_alpha = (opacity.clamp(0.25, 1.0) * 255.0) as u8;
        let tint = Color32::from_rgba_unmultiplied(255, 255, 255, img_alpha);
        let mut mesh = egui::Mesh::with_texture(tex_id);
        mesh.add_rect_with_uv(draw_rect, uv, tint);
        // Clip cleanly to rounded window frame
        let clipped = painter.with_clip_rect(rect);
        clipped.add(egui::Shape::mesh(mesh));

        // Optional dark veil for crisp text readability over bright images/GIFs
        let veil_a = (dim_alpha.clamp(0.0, 0.92) * opacity.clamp(0.25, 1.0) * 255.0) as u8;
        if veil_a > 0 {
            clipped.rect_filled(rect, rounding, Color32::from_rgba_unmultiplied(12, 10, 18, veil_a));
        }
        true
    }
}

/// Native OS File Dialog for choosing background Image / GIF / Video (zero external crates).
pub fn pick_media_file(en: bool) -> Option<String> {
    #[cfg(windows)]
    {
        use std::ffi::{c_void, OsStr};
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

        #[repr(C)]
        struct OpenFileNameW {
            l_struct_size: u32,
            hwnd_owner: isize,
            h_instance: isize,
            lpstr_filter: *const u16,
            lpstr_custom_filter: *mut u16,
            n_max_cust_filter: u32,
            n_filter_index: u32,
            lpstr_file: *mut u16,
            n_max_file: u32,
            lpstr_file_title: *mut u16,
            n_max_file_title: u32,
            lpstr_initial_dir: *const u16,
            lpstr_title: *const u16,
            flags: u32,
            n_file_offset: u16,
            n_file_extension: u16,
            lpstr_def_ext: *const u16,
            l_cust_data: isize,
            lpfn_hook: *const c_void,
            lp_template_name: *const u16,
            pv_reserved: *const c_void,
            dw_reserved: u32,
            flags_ex: u32,
        }
        type FnGetOpenFileNameW = unsafe extern "system" fn(*mut OpenFileNameW) -> i32;

        let dll_w: Vec<u16> = OsStr::new("comdlg32.dll").encode_wide().chain(Some(0)).collect();
        let filter_str = if en {
            "Media (Image / GIF / Video)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.mp4;*.webm;*.mkv;*.mov\0All Files (*.*)\0*.*\0\0"
        } else {
            "Medya (Görsel / GIF / Video)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.mp4;*.webm;*.mkv;*.mov\0Tüm Dosyalar (*.*)\0*.*\0\0"
        };
        let filter_w: Vec<u16> = filter_str.encode_utf16().collect();
        let title_w: Vec<u16> = (if en {
            "Select Background Image / GIF / Video\0"
        } else {
            "Arka Plan Görseli / GIF / Video Seç\0"
        })
        .encode_utf16()
        .collect();
        let mut file_buf = vec![0u16; 1024];

        unsafe {
            let m = LoadLibraryW(dll_w.as_ptr());
            if m.is_null() {
                return None;
            }
            let proc_addr = GetProcAddress(m, b"GetOpenFileNameW\0".as_ptr())?;
            let get_open_fn: FnGetOpenFileNameW = std::mem::transmute(proc_addr);

            let mut ofn: OpenFileNameW = std::mem::zeroed();
            ofn.l_struct_size = std::mem::size_of::<OpenFileNameW>() as u32;
            ofn.lpstr_filter = filter_w.as_ptr();
            ofn.n_filter_index = 1;
            ofn.lpstr_file = file_buf.as_mut_ptr();
            ofn.n_max_file = file_buf.len() as u32;
            ofn.lpstr_title = title_w.as_ptr();
            // OFN_FILEMUSTEXIST (0x1000) | OFN_PATHMUSTEXIST (0x0800) | OFN_NOCHANGEDIR (0x0008)
            ofn.flags = 0x0000_1808;

            if get_open_fn(&mut ofn) != 0 {
                let end = file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len());
                let s = String::from_utf16_lossy(&file_buf[..end]);
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let title = if en {
            "Select Background Media"
        } else {
            "Arka Plan Medyası Seç"
        };
        if let Ok(out) = std::process::Command::new("zenity")
            .args([
                "--file-selection",
                &format!("--title={}", title),
                "--file-filter=Media | *.png *.jpg *.jpeg *.webp *.bmp *.gif *.mp4 *.webm *.mkv",
            ])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = en;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_theme_extracts_vibrant_palette_from_image() {
        // Synthetic cyan/blue image
        let mut rgba = Vec::with_capacity(64 * 64 * 4);
        for _ in 0..(64 * 64) {
            rgba.extend_from_slice(&[20, 210, 245, 255]);
        }
        let pal = extract_theme_from_rgba(&rgba);
        assert!(pal.primary_hex.starts_with('#') && pal.primary_hex.len() == 7);
        assert!(pal.secondary_hex.starts_with('#') && pal.secondary_hex.len() == 7);
        assert!(pal.bg_hex.starts_with('#') && pal.bg_hex.len() == 7);
        assert!(pal.card_hex.starts_with('#') && pal.card_hex.len() == 7);
        assert!(pal.text_hex.starts_with('#') && pal.text_hex.len() == 7);
    }
}
