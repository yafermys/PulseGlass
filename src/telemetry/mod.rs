#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

use crate::installer::is_pawnio_installed;
use crate::models::{HardwareSnapshot, ProcessItem};
use crate::settings::AppSettings;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use sysinfo::{Disks, Networks, ProcessesToUpdate, System};

pub struct TelemetryHandle {
    pub snapshot: Arc<RwLock<HardwareSnapshot>>,
    pub refresh_ms: Arc<AtomicU64>,
    pub include_processes: Arc<AtomicBool>,
    pub group_processes: Arc<AtomicBool>,
    pub thresholds: Arc<RwLock<(f32, f32, f32, f32)>>,
    pub stop: Arc<AtomicBool>,
}

impl TelemetryHandle {
    pub fn spawn(initial: &AppSettings, ctx: egui::Context) -> Self {
        let snapshot = Arc::new(RwLock::new(HardwareSnapshot::default()));
        let refresh_ms = Arc::new(AtomicU64::new(initial.refresh_ms.clamp(500, 10_000)));
        let include_processes = Arc::new(AtomicBool::new(false));
        let group_processes = Arc::new(AtomicBool::new(true));
        let thresholds = Arc::new(RwLock::new((
            initial.cpu_temp_thresh,
            initial.gpu_temp_thresh,
            initial.cpu_use_thresh,
            initial.ram_use_thresh,
        )));
        let stop = Arc::new(AtomicBool::new(false));

        let snap_bg = Arc::clone(&snapshot);
        let ref_bg = Arc::clone(&refresh_ms);
        let inc_bg = Arc::clone(&include_processes);
        let grp_bg = Arc::clone(&group_processes);
        let thr_bg = Arc::clone(&thresholds);
        let stop_bg = Arc::clone(&stop);

        std::thread::Builder::new()
            .name("pulseglass-telemetry".to_string())
            .spawn(move || {
                run_worker(snap_bg, ref_bg, inc_bg, grp_bg, thr_bg, stop_bg, ctx);
            })
            .ok();

        Self {
            snapshot,
            refresh_ms,
            include_processes,
            group_processes,
            thresholds,
            stop,
        }
    }
}

fn run_worker(
    snapshot: Arc<RwLock<HardwareSnapshot>>,
    refresh_ms: Arc<AtomicU64>,
    include_processes: Arc<AtomicBool>,
    group_processes: Arc<AtomicBool>,
    thresholds: Arc<RwLock<(f32, f32, f32, f32)>>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let mut sys = System::new_all();
    let mut disks_sys = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();

    let pawnio = is_pawnio_installed();
    let os_name = System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string());

    #[cfg(windows)]
    let is_admin = windows::is_elevated();
    #[cfg(not(windows))]
    let is_admin = false;

    #[cfg(windows)]
    let nvml = windows::NvmlReader::try_new();
    #[cfg(windows)]
    let (reg_dgpu, reg_igpu) = windows::discover_gpus_from_registry();
    #[cfg(windows)]
    let mut emi = windows::EmiPowerReader::new();
    #[cfg(windows)]
    let etw_fps = windows::EtwFpsMonitor::start();
    #[cfg(windows)]
    let pdh_thermal = windows::PdhThermalReader::try_new();

    #[cfg(target_os = "linux")]
    let mut rapl = linux::LinuxRaplReader::new();

    let mut tick: u64 = 0;
    let mut last_time = Instant::now();
    let mut cached_disks = Vec::new();
    let mut prev_proc_io: HashMap<u32, u64> = HashMap::new();

    while !stop.load(Ordering::Relaxed) {
        let loop_start = Instant::now();
        let elapsed = loop_start.duration_since(last_time).as_secs_f32().max(0.2);
        last_time = loop_start;
        tick = tick.wrapping_add(1);

        sys.refresh_cpu_specifics(sysinfo::CpuRefreshKind::everything());
        sys.refresh_memory();
        networks.refresh();
        if tick == 1 || tick % 10 == 0 {
            disks_sys.refresh_list();
        } else {
            disks_sys.refresh();
        }

        let mut snap = HardwareSnapshot {
            is_admin,
            pawnio_installed: pawnio,
            os_name: os_name.clone(),
            ..Default::default()
        };

        // 1. CPU
        let cpus = sys.cpus();
        snap.cpu.logical_processors = cpus.len();
        snap.cpu.physical_cores = sys.physical_core_count().unwrap_or(cpus.len() / 2).max(1);
        if let Some(first) = cpus.first() {
            snap.cpu.name = first.brand().trim().to_string();
            snap.cpu.current_clock_ghz = first.frequency() as f32 / 1000.0;
            snap.cpu.base_clock_ghz = snap.cpu.current_clock_ghz;
        }
        snap.cpu.usage_percent = sys.global_cpu_usage();
        snap.cpu.per_core_usage = cpus.iter().map(|c| c.cpu_usage()).collect();

        #[cfg(windows)]
        {
            snap.cpu.temperature_c = pdh_thermal.as_ref().and_then(|r| r.read_temp_c());
        }
        #[cfg(target_os = "linux")]
        {
            snap.cpu.temperature_c = linux::read_cpu_temp();
        }

        // 2. RAM (always formatted cleanly as GB with decimal precision)
        let total_mem = sys.total_memory() as f64;
        let used_mem = sys.used_memory() as f64;
        snap.ram.total_gb = (total_mem / (1024.0 * 1024.0 * 1024.0)) as f32;
        snap.ram.installed_gb = snap.ram.total_gb.ceil();
        snap.ram.used_gb = (used_mem / (1024.0 * 1024.0 * 1024.0)) as f32;
        snap.ram.free_gb = (snap.ram.total_gb - snap.ram.used_gb).max(0.0);
        snap.ram.usage_percent = if total_mem > 0.0 {
            ((used_mem / total_mem) * 100.0) as f32
        } else {
            0.0
        };

        // 3. GPU + iGPU
        #[cfg(windows)]
        {
            if let Some(ref ig) = reg_igpu {
                snap.igpu = ig.clone();
            }
            if let Some(ref reader) = nvml {
                reader.poll(&mut snap.gpu);
            } else if let Some(ref dg) = reg_dgpu {
                snap.gpu = dg.clone();
            }
        }
        #[cfg(target_os = "linux")]
        {
            linux::poll_gpu_linux(&mut snap.gpu, &mut snap.igpu);
        }

        // 4. Storage (Logical total/free + Physical M.2/NVMe/SATA/RAID + live SSD Temp!)
        let mut tot_b: u64 = 0;
        let mut avail_b: u64 = 0;
        for d in disks_sys.list() {
            tot_b = tot_b.saturating_add(d.total_space());
            avail_b = avail_b.saturating_add(d.available_space());
        }
        snap.storage.total_gb = (tot_b as f64 / (1024.0 * 1024.0 * 1024.0)) as f32;
        snap.storage.free_gb = (avail_b as f64 / (1024.0 * 1024.0 * 1024.0)) as f32;
        if tot_b > 0 {
            snap.storage.usage_percent = (((tot_b - avail_b) as f64 / tot_b as f64) * 100.0) as f32;
        }

        if tick == 1 || tick % 5 == 0 {
            #[cfg(windows)]
            {
                cached_disks = windows::probe_physical_disks();
            }
            #[cfg(target_os = "linux")]
            {
                cached_disks = linux::probe_physical_disks();
            }
        }
        snap.storage.disks = cached_disks.clone();
        snap.storage.temperature_c = cached_disks.iter().find_map(|d| d.temperature_c);
        snap.storage.primary_interface = cached_disks
            .first()
            .map(|d| d.interface_label.clone())
            .unwrap_or_else(|| "SSD".to_string());

        // 5. Power / Wattage (CPU W + GPU W + Device Total W)
        #[cfg(windows)]
        {
            snap.power = windows::poll_power_info(
                &mut emi,
                snap.gpu.power_watts,
                snap.gpu.power_limit_watts,
            );
            snap.cpu.package_watts = snap.power.cpu_watts;
        }
        #[cfg(target_os = "linux")]
        {
            snap.power = linux::poll_power_linux(
                &mut rapl,
                snap.gpu.power_watts,
                snap.gpu.power_limit_watts,
            );
            snap.cpu.package_watts = snap.power.cpu_watts;
        }

        // 6. Network
        let mut rx_bytes: u64 = 0;
        let mut tx_bytes: u64 = 0;
        let mut best_nic = String::new();
        let mut best_traffic = 0u64;
        for (name, data) in networks.list() {
            let r = data.received();
            let t = data.transmitted();
            rx_bytes = rx_bytes.saturating_add(r);
            tx_bytes = tx_bytes.saturating_add(t);
            if r + t >= best_traffic {
                best_traffic = r + t;
                best_nic = name.clone();
            }
        }
        snap.network.adapter_name = if best_nic.is_empty() {
            "Network".to_string()
        } else {
            best_nic
        };
        snap.network.download_mbps = (rx_bytes as f32) / (elapsed * 1024.0 * 1024.0);
        snap.network.upload_mbps = (tx_bytes as f32) / (elapsed * 1024.0 * 1024.0);

        // 7. FPS (RTSS -> DXGI ETW)
        #[cfg(windows)]
        {
            if let Some(f) = windows::read_rtss_fps() {
                snap.fps = Some(f);
                snap.fps_source = "RTSS".to_string();
            } else if let Some(f) = etw_fps.get_fps() {
                snap.fps = Some(f);
                snap.fps_source = "DXGI".to_string();
            }
        }

        // 8. Processes (only when Detailed / Processes view is active to keep idle CPU tiny!)
        if include_processes.load(Ordering::Relaxed) {
            sys.refresh_processes(ProcessesToUpdate::All, true);
            let ncpu = snap.cpu.logical_processors.max(1) as f32;
            let mut raw: Vec<ProcessItem> = Vec::with_capacity(256);
            let mut total_read = 0.0f32;
            let mut total_write = 0.0f32;
            for (&pid, proc_) in sys.processes() {
                let pid_u = pid.as_u32();
                let du = proc_.disk_usage();
                let cur_io = du.total_read_bytes.saturating_add(du.total_written_bytes);
                let prev_io = prev_proc_io.insert(pid_u, cur_io).unwrap_or(cur_io);
                let delta_io = cur_io.saturating_sub(prev_io);
                let disk_mbps = (delta_io as f32) / (elapsed * 1024.0 * 1024.0);
                total_read += (du.read_bytes as f32) / (elapsed * 1024.0 * 1024.0);
                total_write += (du.written_bytes as f32) / (elapsed * 1024.0 * 1024.0);

                raw.push(ProcessItem {
                    pid: pid_u,
                    name: proc_.name().to_string_lossy().to_string(),
                    exe_path: proc_
                        .exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    instance_count: 1,
                    cpu_percent: (proc_.cpu_usage() / ncpu).clamp(0.0, 100.0),
                    ram_mb: proc_.memory() as f32 / (1024.0 * 1024.0),
                    disk_mbps,
                });
            }
            snap.storage.read_mbps = total_read;
            snap.storage.write_mbps = total_write;

            if group_processes.load(Ordering::Relaxed) {
                let mut grouped: HashMap<String, ProcessItem> = HashMap::new();
                for item in raw {
                    let key = item.name.to_lowercase();
                    grouped
                        .entry(key)
                        .and_modify(|e| {
                            e.instance_count += 1;
                            e.cpu_percent += item.cpu_percent;
                            e.ram_mb += item.ram_mb;
                            e.disk_mbps += item.disk_mbps;
                            if e.exe_path.is_empty() && !item.exe_path.is_empty() {
                                e.exe_path = item.exe_path.clone();
                            }
                        })
                        .or_insert(item);
                }
                snap.processes = grouped.into_values().collect();
            } else {
                snap.processes = raw;
            }
        }

        // 9. Alerts
        if let Ok((cpu_t, gpu_t, cpu_u, ram_u)) = thresholds.read().map(|g| *g) {
            if let Some(t) = snap.cpu.temperature_c {
                if t >= cpu_t {
                    snap.alerts.push(format!("CPU {:.0}°C!", t));
                }
            }
            if let Some(t) = snap.gpu.temperature_c {
                if t >= gpu_t {
                    snap.alerts.push(format!("GPU {:.0}°C!", t));
                }
            }
            if snap.cpu.usage_percent >= cpu_u {
                snap.alerts.push(format!("CPU %{:.0}!", snap.cpu.usage_percent));
            }
            if snap.ram.usage_percent >= ram_u {
                snap.alerts.push(format!("RAM %{:.0}!", snap.ram.usage_percent));
            }
        }

        if let Ok(mut guard) = snapshot.write() {
            *guard = snap;
        }
        ctx.request_repaint();

        let target_ms = refresh_ms.load(Ordering::Relaxed).clamp(500, 10_000);
        let spent = loop_start.elapsed();
        let wait = Duration::from_millis(target_ms).saturating_sub(spent);
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

pub fn kill_process_by_pid(pid: u32) -> bool {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]), true);
    if let Some(p) = sys.process(sysinfo::Pid::from_u32(pid)) {
        p.kill()
    } else {
        false
    }
}
