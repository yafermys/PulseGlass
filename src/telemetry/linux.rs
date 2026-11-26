#![cfg(target_os = "linux")]

use crate::models::{GpuInfo, PhysicalDiskInfo, PowerInfo};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

pub struct LinuxRaplReader {
    last_uj: Option<u64>,
    last_time: Instant,
}

impl LinuxRaplReader {
    pub fn new() -> Self {
        let uj = read_rapl_uj();
        Self {
            last_uj: uj,
            last_time: Instant::now(),
        }
    }

    pub fn poll_cpu_watts(&mut self) -> Option<f32> {
        let cur = read_rapl_uj()?;
        let now = Instant::now();
        let dt = now.duration_since(self.last_time).as_secs_f64();
        let prev = self.last_uj;
        self.last_uj = Some(cur);
        self.last_time = now;

        if let Some(p) = prev {
            if cur >= p && dt > 0.05 {
                let watts = ((cur - p) as f64 / 1_000_000.0) / dt;
                if watts > 0.2 && watts < 400.0 {
                    return Some(watts as f32);
                }
            }
        }
        None
    }
}

fn read_rapl_uj() -> Option<u64> {
    let candidates = [
        "/sys/class/powercap/intel-rapl:0/energy_uj",
        "/sys/class/powercap/intel-rapl/intel-rapl:0/energy_uj",
    ];
    for c in &candidates {
        if let Ok(s) = fs::read_to_string(c) {
            if let Ok(v) = s.trim().parse::<u64>() {
                return Some(v);
            }
        }
    }
    None
}

pub fn read_cpu_temp() -> Option<f32> {
    let mut acpi_fallback = None;
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = fs::read_to_string(p.join("name"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            // AMD Ryzen k10temp/zenpower may expose Tdie on temp2_input and Tctl on temp1_input
            if name.contains("k10temp") || name.contains("zenpower") {
                for file in ["temp2_input", "temp1_input", "temp3_input"] {
                    if let Ok(raw) = fs::read_to_string(p.join(file)) {
                        if let Ok(mc) = raw.trim().parse::<f32>() {
                            let c = mc / 1000.0;
                            if c > 10.0 && c < 115.0 {
                                return Some(c.round());
                            }
                        }
                    }
                }
            }
            if let Ok(raw) = fs::read_to_string(p.join("temp1_input")) {
                if let Ok(mc) = raw.trim().parse::<f32>() {
                    let c = mc / 1000.0;
                    if c > 10.0 && c < 115.0 {
                        if name.contains("coretemp") || name.contains("x86_pkg_temp") {
                            return Some(c.round());
                        }
                        if (name.contains("acpitz")
                            || name.contains("thinkpad")
                            || name.contains("asus")
                            || name.contains("dell_smm"))
                            && acpi_fallback.is_none()
                        {
                            acpi_fallback = Some(c.round());
                        }
                    }
                }
            }
        }
    }
    if acpi_fallback.is_some() {
        return acpi_fallback;
    }
    if let Ok(zones) = fs::read_dir("/sys/class/thermal") {
        for z in zones.flatten() {
            let zp = z.path();
            let ztype = fs::read_to_string(zp.join("type"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            if ztype.contains("x86_pkg_temp") || ztype.contains("cpu") || ztype.contains("acpi") {
                if let Ok(raw) = fs::read_to_string(zp.join("temp")) {
                    if let Ok(mc) = raw.trim().parse::<f32>() {
                        let c = mc / 1000.0;
                        if c > 10.0 && c < 115.0 {
                            return Some(c.round());
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn probe_physical_disks() -> Vec<PhysicalDiskInfo> {
    let mut disks = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return disks;
    };
    let mut idx = 0u32;
    for entry in entries.flatten() {
        let dev = entry.file_name().to_string_lossy().to_string();
        if !(dev.starts_with("nvme") || dev.starts_with("sd") || dev.starts_with("mmcblk")) {
            continue;
        }
        let base = entry.path();
        let sectors: u64 = fs::read_to_string(base.join("size"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let total_gb = (sectors as f64 * 512.0 / (1024.0 * 1024.0 * 1024.0)) as f32;

        let model = fs::read_to_string(base.join("device/model"))
            .unwrap_or_else(|_| dev.clone())
            .trim()
            .to_string();
        let rotational: u8 = fs::read_to_string(base.join("queue/rotational"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);

        let (interface_label, media_type, bus_raw) = if dev.starts_with("nvme") {
            ("NVMe M.2".to_string(), "NVMe M.2 SSD".to_string(), "NVMe".to_string())
        } else {
            let link = fs::read_link(base.join("device"))
                .map(|p| p.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if link.contains("usb") {
                ("USB".to_string(), if rotational == 0 { "USB SSD".to_string() } else { "USB HDD".to_string() }, "USB".to_string())
            } else if rotational == 0 {
                ("SATA SSD".to_string(), "SSD".to_string(), "SATA".to_string())
            } else {
                ("SATA HDD".to_string(), "HDD".to_string(), "SATA".to_string())
            }
        };

        let temp = find_block_hwmon_temp(&base.join("device/hwmon"));

        disks.push(PhysicalDiskInfo {
            index: idx,
            model,
            total_gb,
            media_type,
            interface_label,
            bus_type_raw: bus_raw,
            firmware: String::new(),
            temperature_c: temp,
        });
        idx += 1;
    }
    disks
}

fn find_block_hwmon_temp(hwmon_dir: &Path) -> Option<f32> {
    let entries = fs::read_dir(hwmon_dir).ok()?;
    for entry in entries.flatten() {
        if let Ok(raw) = fs::read_to_string(entry.path().join("temp1_input")) {
            if let Ok(mc) = raw.trim().parse::<f32>() {
                let c = mc / 1000.0;
                if c > 5.0 && c < 110.0 {
                    return Some(c.round());
                }
            }
        }
    }
    None
}

pub fn poll_gpu_linux(dgpu: &mut GpuInfo, igpu: &mut GpuInfo) {
    // 1. Try NVIDIA via nvidia-smi
    if let Ok(out) = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total,clocks.gr,fan.speed,power.draw,power.limit,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        if out.status.success() {
            let line = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = line.lines().next() {
                let parts: Vec<&str> = first.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 10 {
                    dgpu.name = parts[0].to_string();
                    dgpu.is_available = true;
                    dgpu.is_integrated = false;
                    dgpu.usage_percent = parts[1].parse().unwrap_or(0.0);
                    dgpu.temperature_c = parts[2].parse().ok();
                    dgpu.used_vram_gb = parts[3].parse::<f32>().unwrap_or(0.0) / 1024.0;
                    dgpu.total_vram_gb = parts[4].parse::<f32>().unwrap_or(0.0) / 1024.0;
                    dgpu.clock_mhz = parts[5].parse().ok();
                    dgpu.fan_percent = parts[6].parse().ok();
                    dgpu.power_watts = parts[7].parse().ok();
                    dgpu.power_limit_watts = parts[8].parse().ok();
                    dgpu.driver_version = parts[9].to_string();
                }
            }
        }
    }

    // 2. Try AMD / Intel via /sys/class/drm/card*
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let dev = entry.path().join("device");
            let vendor = fs::read_to_string(dev.join("vendor"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            if vendor == "0x1002" {
                // AMD GPU
                let target = if !dgpu.is_available { &mut *dgpu } else { &mut *igpu };
                target.is_available = true;
                if target.name.is_empty() {
                    target.name = "AMD Radeon GPU".to_string();
                }
                if let Ok(u) = fs::read_to_string(dev.join("gpu_busy_percent")) {
                    target.usage_percent = u.trim().parse().unwrap_or(0.0);
                }
                if let Ok(used) = fs::read_to_string(dev.join("mem_info_vram_used")) {
                    target.used_vram_gb =
                        (used.trim().parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0 * 1024.0)) as f32;
                }
                if let Ok(tot) = fs::read_to_string(dev.join("mem_info_vram_total")) {
                    target.total_vram_gb =
                        (tot.trim().parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0 * 1024.0)) as f32;
                }
                if let Ok(hwmons) = fs::read_dir(dev.join("hwmon")) {
                    for hm in hwmons.flatten() {
                        if let Ok(t) = fs::read_to_string(hm.path().join("temp1_input")) {
                            if let Ok(mc) = t.trim().parse::<f32>() {
                                target.temperature_c = Some((mc / 1000.0).round());
                            }
                        }
                        if let Ok(pw) = fs::read_to_string(hm.path().join("power1_average")) {
                            if let Ok(uw) = pw.trim().parse::<f32>() {
                                target.power_watts = Some(uw / 1_000_000.0);
                            }
                        }
                    }
                }
            } else if vendor == "0x8086" && !igpu.is_available {
                igpu.is_available = true;
                igpu.is_integrated = true;
                igpu.name = "Intel Integrated Graphics".to_string();
            }
        }
    }
}

pub fn poll_power_linux(rapl: &mut LinuxRaplReader, gpu_watts: Option<f32>, gpu_limit: Option<f32>) -> PowerInfo {
    let mut p = PowerInfo {
        cpu_watts: rapl.poll_cpu_watts(),
        gpu_watts,
        gpu_limit_watts: gpu_limit,
        ..Default::default()
    };

    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("BAT") {
                continue;
            }
            let base = entry.path();
            let status = fs::read_to_string(base.join("status"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            p.on_battery = status == "discharging";
            if let Ok(cap) = fs::read_to_string(base.join("capacity")) {
                p.battery_percent = cap.trim().parse().ok();
            }
            if let Ok(pw) = fs::read_to_string(base.join("power_now")) {
                if let Ok(uw) = pw.trim().parse::<f32>() {
                    if uw > 0.0 {
                        p.battery_watts = Some(uw / 1_000_000.0);
                    }
                }
            } else if let (Ok(cur), Ok(volt)) = (
                fs::read_to_string(base.join("current_now")),
                fs::read_to_string(base.join("voltage_now")),
            ) {
                if let (Ok(ua), Ok(uv)) = (cur.trim().parse::<f64>(), volt.trim().parse::<f64>()) {
                    let w = (ua * uv) / 1e12;
                    if w > 0.5 && w < 300.0 {
                        p.battery_watts = Some(w as f32);
                    }
                }
            }
        }
    }

    if p.on_battery {
        if let Some(bw) = p.battery_watts {
            p.device_total_watts = Some(bw);
            p.source_note = "Battery / Device".to_string();
            return p;
        }
    }

    match (p.cpu_watts, p.gpu_watts) {
        (Some(cw), Some(gw)) => {
            p.device_total_watts = Some(cw + gw + 8.0);
            p.source_note = "CPU (RAPL) + GPU + Platform".to_string();
        }
        (Some(cw), None) => {
            p.device_total_watts = Some(cw + 8.0);
            p.source_note = "CPU (RAPL) + Platform".to_string();
        }
        (None, Some(gw)) => {
            p.device_total_watts = Some(gw);
            p.source_note = "GPU".to_string();
        }
        (None, None) => {}
    }

    p
}
