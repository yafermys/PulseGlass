#![allow(dead_code)]
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default)]
pub struct CpuInfo {
    pub name: String,
    pub physical_cores: usize,
    pub logical_processors: usize,
    pub usage_percent: f32,
    pub current_clock_ghz: f32,
    pub base_clock_ghz: f32,
    pub temperature_c: Option<f32>,
    pub package_watts: Option<f32>,
    pub per_core_usage: Vec<f32>,
    pub temperature_note: Option<String>,
}

impl CpuInfo {
    pub fn temp_text(&self) -> String {
        match self.temperature_c {
            Some(t) => format!("{:.0}°C", t),
            None => "--°C".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GpuInfo {
    pub name: String,
    pub is_integrated: bool,
    pub is_available: bool,
    pub usage_percent: f32,
    pub temperature_c: Option<f32>,
    pub used_vram_gb: f32,
    pub total_vram_gb: f32,
    pub clock_mhz: Option<u32>,
    pub fan_percent: Option<u32>,
    pub power_watts: Option<f32>,
    pub power_limit_watts: Option<f32>,
    pub driver_version: String,
    pub sensor_note: Option<String>,
}

impl GpuInfo {
    pub fn temp_text(&self) -> String {
        match self.temperature_c {
            Some(t) => format!("{:.0}°C", t),
            None => "--°C".to_string(),
        }
    }

    pub fn vram_text(&self) -> String {
        if self.total_vram_gb > 0.0 {
            format!("{:.1} / {:.1} GB", self.used_vram_gb, self.total_vram_gb)
        } else if self.used_vram_gb > 0.0 {
            format!("{:.2} GB", self.used_vram_gb)
        } else {
            "N/A".to_string()
        }
    }

    pub fn power_text(&self) -> String {
        match (self.power_watts, self.power_limit_watts) {
            (Some(w), Some(lim)) if lim > 0.0 => format!("{:.1} W / {:.0} W", w, lim),
            (Some(w), _) => format!("{:.1} W", w),
            _ => "-- W".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RamInfo {
    pub installed_gb: f32,
    pub total_gb: f32,
    pub used_gb: f32,
    pub free_gb: f32,
    pub usage_percent: f32,
    pub speed_mhz: u32,
    pub memory_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct PhysicalDiskInfo {
    pub index: u32,
    pub model: String,
    pub total_gb: f32,
    /// e.g. "SSD", "HDD", "NVMe SSD"
    pub media_type: String,
    /// e.g. "NVMe M.2", "NVMe M.2 (Intel VMD RAID)", "SATA", "USB", "SAS"
    pub interface_label: String,
    pub bus_type_raw: String,
    pub firmware: String,
    pub temperature_c: Option<f32>,
}

impl PhysicalDiskInfo {
    pub fn temp_text(&self) -> String {
        match self.temperature_c {
            Some(t) => format!("{:.0}°C", t),
            None => "--°C".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StorageInfo {
    pub total_gb: f32,
    pub free_gb: f32,
    pub usage_percent: f32,
    pub read_mbps: f32,
    pub write_mbps: f32,
    pub temperature_c: Option<f32>,
    pub primary_interface: String,
    pub disks: Vec<PhysicalDiskInfo>,
}

impl StorageInfo {
    pub fn temp_text(&self) -> String {
        match self.temperature_c {
            Some(t) => format!("{:.0}°C", t),
            None => "--°C".to_string(),
        }
    }

    pub fn io_text(&self) -> String {
        format!("R {:.1} / W {:.1} MB/s", self.read_mbps, self.write_mbps)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PowerInfo {
    pub cpu_watts: Option<f32>,
    pub gpu_watts: Option<f32>,
    pub gpu_limit_watts: Option<f32>,
    pub battery_watts: Option<f32>,
    pub device_total_watts: Option<f32>,
    pub on_battery: bool,
    pub battery_percent: Option<u8>,
    pub source_note: String,
}

impl PowerInfo {
    pub fn device_text(&self) -> String {
        match self.device_total_watts {
            Some(w) => format!("{:.1} W", w),
            None => "-- W".to_string(),
        }
    }

    pub fn gpu_text(&self) -> String {
        match self.gpu_watts {
            Some(w) => format!("{:.1} W", w),
            None => "-- W".to_string(),
        }
    }

    pub fn cpu_text(&self) -> String {
        match self.cpu_watts {
            Some(w) => format!("{:.1} W", w),
            None => "-- W".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NetworkInfo {
    pub adapter_name: String,
    pub download_mbps: f32,
    pub upload_mbps: f32,
}

#[derive(Debug, Clone)]
pub struct ProcessItem {
    pub pid: u32,
    pub name: String,
    pub exe_path: String,
    pub instance_count: usize,
    pub cpu_percent: f32,
    pub ram_mb: f32,
    pub disk_mbps: f32,
}

impl ProcessItem {
    pub fn ram_text(&self) -> String {
        if self.ram_mb >= 1024.0 {
            format!("{:.2} GB", self.ram_mb / 1024.0)
        } else {
            format!("{:.0} MB", self.ram_mb)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HardwareSnapshot {
    pub cpu: CpuInfo,
    pub gpu: GpuInfo,
    pub igpu: GpuInfo,
    pub ram: RamInfo,
    pub storage: StorageInfo,
    pub power: PowerInfo,
    pub network: NetworkInfo,
    pub processes: Vec<ProcessItem>,
    pub fps: Option<u32>,
    pub fps_source: String,
    pub alerts: Vec<String>,
    pub is_admin: bool,
    pub pawnio_installed: bool,
    pub os_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsUpdateEntry {
    pub id: String,
    pub title: String,
    pub size_mb: f64,
    pub is_optional: bool,
    pub category: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateCheckResult {
    pub checked_at: String,
    pub app_update_available: bool,
    pub app_latest_version: String,
    pub app_download_url: String,
    pub app_release_notes: String,
    pub app_status_msg: String,
    pub important_updates: Vec<OsUpdateEntry>,
    pub optional_updates: Vec<OsUpdateEntry>,
    pub os_status_msg: String,
}
