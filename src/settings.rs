use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub widget_opacity: f32,
    pub window_opacity: f32,
    pub ui_scale: f32,
    pub refresh_ms: u64,
    pub primary_hex: String,
    pub secondary_hex: String,
    pub bg_hex: String,
    pub cpu_temp_thresh: f32,
    pub gpu_temp_thresh: f32,
    pub cpu_use_thresh: f32,
    pub ram_use_thresh: f32,
    pub pos_x: Option<f32>,
    pub pos_y: Option<f32>,
    pub sum_cpu_pct: bool,
    pub sum_cpu_temp: bool,
    pub sum_gpu_pct: bool,
    pub sum_gpu_temp: bool,
    pub sum_igpu_pct: bool,
    pub sum_ram_pct: bool,
    pub sum_ssd_temp: bool,
    pub sum_gpu_watts: bool,
    pub sum_device_watts: bool,
    pub sum_fps: bool,
    pub language: String,
    pub topmost: bool,
    pub desktop_pinned: bool,
    pub hotkey_summary: String,
    pub hotkey_ghost: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            widget_opacity: 0.80,
            window_opacity: 0.94,
            ui_scale: 1.0,
            refresh_ms: 1000,
            primary_hex: "#FF4D6D".to_string(),
            secondary_hex: "#FFB085".to_string(),
            bg_hex: "#120F18".to_string(),
            cpu_temp_thresh: 85.0,
            gpu_temp_thresh: 85.0,
            cpu_use_thresh: 90.0,
            ram_use_thresh: 90.0,
            pos_x: None,
            pos_y: None,
            sum_cpu_pct: true,
            sum_cpu_temp: true,
            sum_gpu_pct: true,
            sum_gpu_temp: true,
            sum_igpu_pct: false,
            sum_ram_pct: true,
            sum_ssd_temp: true,
            sum_gpu_watts: false,
            sum_device_watts: false,
            sum_fps: true,
            language: "tr".to_string(),
            topmost: true,
            desktop_pinned: false,
            hotkey_summary: "Ctrl+Enter".to_string(),
            hotkey_ghost: "Shift+Enter".to_string(),
        }
    }
}

pub fn is_portable_mode() -> bool {
    if std::env::args().any(|a| a.eq_ignore_ascii_case("--portable")) {
        return true;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("portable.flag").exists() || dir.join("pulseglass.json").exists() {
                return true;
            }
        }
    }
    false
}

pub fn config_path() -> PathBuf {
    if is_portable_mode() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join("pulseglass.json");
            }
        }
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("PulseGlass").join("settings.json")
}

pub fn load_settings() -> AppSettings {
    let path = config_path();
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(s) = serde_json::from_str::<AppSettings>(&text) {
            return s;
        }
    }
    AppSettings::default()
}

pub fn save_settings(s: &AppSettings) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = fs::write(path, json);
    }
}

pub fn parse_hex_color(hex: &str, fallback: egui::Color32) -> egui::Color32 {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&clean[0..2], 16),
            u8::from_str_radix(&clean[2..4], 16),
            u8::from_str_radix(&clean[4..6], 16),
        ) {
            return egui::Color32::from_rgb(r, g, b);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_color_parses_correctly() {
        let c = parse_hex_color("#FF4D6D", egui::Color32::WHITE);
        assert_eq!(c, egui::Color32::from_rgb(255, 77, 109));
    }

    #[test]
    fn default_settings_roundtrip() {
        let s = AppSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.primary_hex, "#FF4D6D");
        assert!(back.sum_fps);
        assert!(back.sum_ssd_temp);
    }
}
