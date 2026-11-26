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
    // Brief warmup so tick 1 immediately reports accurate CPU% instead of 0%
    std::thread::sleep(Duration::from_millis(200));
    let mut disks_sys = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();

    let mut pawnio = is_pawnio_installed();
    let os_name = System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string());
    let cpu_brand_init = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    #[cfg(windows)]
    let phys_ram_gb = windows::get_installed_ram_gb();

    #[cfg(windows)]
    let is_admin = windows::is_elevated();
    #[cfg(not(windows))]
    let is_admin = false;

    #[cfg(windows)]
    let mut nvml = windows::NvmlReader::try_new();
    #[cfg(windows)]
    let mut pdh_gpu = windows::PdhGpuReader::try_new();
    #[cfg(windows)]
    let mut pending_pdh_gpu: Option<windows::PdhGpuReader> = None;
    #[cfg(windows)]
    let mut intel_igcl = windows::IntelIgclReader::try_new();
    #[cfg(windows)]
    let (reg_dgpu, reg_igpu) = windows::discover_gpus_from_registry();
    #[cfg(windows)]
    let mut emi = windows::EmiPowerReader::new();
    #[cfg(windows)]
    let mut pawnio_reader = windows::PawnIoReader::try_new(&cpu_brand_init);
    #[cfg(windows)]
    let etw_fps = windows::EtwFpsMonitor::start();
    #[cfg(windows)]
    let dwm_pid: Option<u32> = sys
        .processes()
        .iter()
        .find(|(_, p)| p.name().to_string_lossy().eq_ignore_ascii_case("dwm.exe"))
        .map(|(pid, _)| pid.as_u32());
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
        let (pawnio_temp, pawnio_pkg_w, pawnio_psys_w) = {
            if pawnio_reader.is_none() && tick % 8 == 0 {
                pawnio = is_pawnio_installed();
                if pawnio {
                    pawnio_reader = windows::PawnIoReader::try_new(&snap.cpu.name);
                }
            }
            if let Some(ref mut pr) = pawnio_reader {
                pawnio = true;
                pr.poll()
            } else {
                (None, None, None)
            }
        };

        #[cfg(windows)]
        {
            if let Some(ref r) = pdh_thermal {
                let (t, perf_pct) = r.poll();
                snap.cpu.temperature_c = pawnio_temp.or(t);
                if let Some(pct) = perf_pct {
                    if snap.cpu.base_clock_ghz > 0.1 {
                        snap.cpu.current_clock_ghz = snap.cpu.base_clock_ghz * (pct / 100.0);
                    }
                }
            } else {
                snap.cpu.temperature_c = pawnio_temp;
            }
            snap.pawnio_installed = pawnio;
        }
        #[cfg(target_os = "linux")]
        {
            snap.cpu.temperature_c = linux::read_cpu_temp();
        }

        // 2. RAM (always formatted cleanly as GB with exact hardware installed size)
        let total_mem = sys.total_memory() as f64;
        let used_mem = sys.used_memory() as f64;
        snap.ram.total_gb = (total_mem / (1024.0 * 1024.0 * 1024.0)) as f32;
        #[cfg(windows)]
        {
            snap.ram.installed_gb = phys_ram_gb.unwrap_or_else(|| snap.ram.total_gb.ceil());
        }
        #[cfg(not(windows))]
        {
            snap.ram.installed_gb = snap.ram.total_gb.ceil();
        }
        snap.ram.used_gb = (used_mem / (1024.0 * 1024.0 * 1024.0)) as f32;
        snap.ram.free_gb = (snap.ram.total_gb - snap.ram.used_gb).max(0.0);
        snap.ram.usage_percent = if total_mem > 0.0 {
            ((used_mem / total_mem) * 100.0) as f32
        } else {
            0.0
        };

        // 3. GPU + iGPU (Universal DXGI + PDH GPU Engine/Memory + Auto-retrying NVIDIA NVML)
        #[cfg(windows)]
        {
            if let Some(ref ig) = reg_igpu {
                snap.igpu = ig.clone();
            }
            if let Some(ref dg) = reg_dgpu {
                snap.gpu = dg.clone();
            }

            // Double-buffered PDH wildcard refresh every 15 ticks so newly launched games/apps
            // are always captured by \GPU Engine(*) with ZERO 0%-flicker between samples.
            if let Some(fresh) = pending_pdh_gpu.take() {
                pdh_gpu = Some(fresh);
            } else if tick % 15 == 0 {
                pending_pdh_gpu = windows::PdhGpuReader::try_new();
            }

            // Populate exact DXGI adapter info + live PDH GPU utilization & VRAM for BOTH dGPU and iGPU!
            let mut dgpu_luid: Option<(u32, i32)> = None;
            let mut igpu_luid: Option<(u32, i32)> = None;
            let pdh_sample = if let Some(ref pg) = pdh_gpu {
                if let Some(ref da) = pg.dgpu_adapter {
                    dgpu_luid = Some((da.luid_low, da.luid_high));
                    if snap.gpu.name.is_empty() {
                        snap.gpu.name = da.name.clone();
                    }
                    snap.gpu.is_available = true;
                    snap.gpu.is_integrated = false;
                    snap.gpu.dedicated_vram_gb = da.total_vram_gb;
                    snap.gpu.shared_total_gb = da.shared_vram_gb;
                    if da.total_vram_gb > 0.1 {
                        snap.gpu.total_vram_gb = da.total_vram_gb;
                    }
                }
                if let Some(ref ia) = pg.igpu_adapter {
                    igpu_luid = Some((ia.luid_low, ia.luid_high));
                    if snap.igpu.name.is_empty() {
                        snap.igpu.name = ia.name.clone();
                    }
                    snap.igpu.is_available = true;
                    snap.igpu.is_integrated = true;
                    // Real DXGI Dedicated VRAM (e.g. 128 MB on Intel UHD, NOT bogus Registry 2.0 GB!)
                    snap.igpu.dedicated_vram_gb = ia.total_vram_gb;
                    snap.igpu.shared_total_gb = ia.shared_vram_gb;
                    snap.igpu.total_vram_gb = if ia.total_vram_gb >= 0.5 {
                        ia.total_vram_gb
                    } else {
                        ia.shared_vram_gb
                    };
                }
                pg.poll()
            } else {
                windows::PdhGpuSample::default()
            };

            if let Some(iu) = pdh_sample.igpu_util {
                snap.igpu.usage_percent = iu;
            }
            if let Some(shr) = pdh_sample.igpu_shr_gb {
                snap.igpu.shared_used_gb = shr;
            }
            // For UMA iGPUs (<0.5 GB dedicated VRAM), active GPU memory is Shared System Memory (matches Task Manager!)
            if snap.igpu.dedicated_vram_gb >= 0.5 {
                if let Some(ded) = pdh_sample.igpu_ded_gb {
                    snap.igpu.used_vram_gb = ded;
                }
            } else if let Some(shr) = pdh_sample.igpu_shr_gb {
                snap.igpu.used_vram_gb = shr;
            }

            // Live iGPU telemetry: D3DKMT WDDM sensor -> Intel IGCL (clock, power, GT temp) -> SoC/Package die temp!
            if snap.igpu.is_available {
                if let Some((lo, hi)) = igpu_luid {
                    snap.igpu.temperature_c = windows::query_d3dkmt_adapter_temp(lo, hi);
                }
                if let Some(ref mut igcl) = intel_igcl {
                    let s = igcl.poll();
                    if snap.igpu.temperature_c.is_none() {
                        snap.igpu.temperature_c = s.temp_c;
                    }
                    snap.igpu.clock_mhz = s.clock_mhz;
                    snap.igpu.power_watts = s.power_watts;
                }
                // On monolithic Intel/AMD CPUs where the iGPU sits on the exact same silicon die as the CPU
                // and exposes no independent WDDM/IGCL thermistor, report the shared CPU/SoC Package Die temp!
                if snap.igpu.temperature_c.is_none() {
                    if let Some(soc_t) = snap.cpu.temperature_c {
                        snap.igpu.temperature_c = Some(soc_t);
                        snap.igpu.sensor_note = Some("SoC / Paket".to_string());
                    }
                }
            }

            // Retry NVML initialization periodically if dGPU woke up from Optimus sleep
            if nvml.is_none() && tick % 10 == 0 {
                nvml = windows::NvmlReader::try_new();
            }
            let mut nvml_ok = false;
            if let Some(ref reader) = nvml {
                nvml_ok = reader.poll(&mut snap.gpu);
            }
            if !nvml_ok && nvml.is_some() && tick % 10 == 0 {
                // Re-acquire NVML handle if GPU lost/slept
                nvml = windows::NvmlReader::try_new();
            }

            // Universal D3DKMT temperature fallback for AMD/Intel dGPUs (or if NVML unavailable)
            if snap.gpu.temperature_c.is_none() {
                if let Some((lo, hi)) = dgpu_luid {
                    snap.gpu.temperature_c = windows::query_d3dkmt_adapter_temp(lo, hi);
                }
            }

            // Combine PDH dGPU usage & VRAM seamlessly (works for AMD/Intel dGPUs and NVIDIA Optimus!)
            if let Some(du) = pdh_sample.dgpu_util {
                snap.gpu.usage_percent = snap.gpu.usage_percent.max(du);
            }
            if let Some(dm) = pdh_sample.dgpu_ded_gb {
                if snap.gpu.used_vram_gb < 0.01 && dm > 0.01 {
                    snap.gpu.used_vram_gb = dm;
                }
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

        if tick == 1 || tick % 2 == 0 {
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

        // 5. Power / Wattage (PawnIO RAPL/PSys + EMI + GPU W + Device Total W)
        #[cfg(windows)]
        {
            snap.power = windows::poll_power_info(
                &mut emi,
                pawnio_pkg_w,
                pawnio_psys_w,
                snap.gpu.power_watts,
                snap.gpu.power_limit_watts,
                snap.cpu.usage_percent,
                snap.cpu.current_clock_ghz,
                snap.cpu.base_clock_ghz,
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

        // 6. Network (skip loopback / virtual / filter adapters to prevent double-counting)
        let mut rx_bytes: u64 = 0;
        let mut tx_bytes: u64 = 0;
        let mut best_nic = String::new();
        let mut best_traffic = 0u64;
        for (name, data) in networks.list() {
            let nl = name.to_lowercase();
            if nl.contains("loopback")
                || nl.contains("vethernet")
                || nl.contains("teredo")
                || nl.contains("isatap")
                || nl.contains("pseudo")
                || nl.contains("wfp")
                || nl.contains("qos")
                || nl == "lo"
            {
                continue;
            }
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

        // 7. FPS (RTSS -> Multi-Provider ETW: DXGI / D3D9 / DxgKrnl)
        #[cfg(windows)]
        {
            if let Some(f) = windows::read_rtss_fps() {
                snap.fps = Some(f);
                snap.fps_source = "RTSS".to_string();
            } else if let Some(f) = etw_fps.get_fps(dwm_pid) {
                snap.fps = Some(f);
                snap.fps_source = "ETW".to_string();
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

                let mut exe_path = proc_
                    .exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                #[cfg(windows)]
                if exe_path.is_empty() {
                    if let Some(wp) = windows::query_process_exe_path(pid_u) {
                        exe_path = wp;
                    }
                }

                raw.push(ProcessItem {
                    pid: pid_u,
                    pids: vec![pid_u],
                    name: proc_.name().to_string_lossy().to_string(),
                    exe_path,
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
                            e.pids.push(item.pid);
                            e.cpu_percent = (e.cpu_percent + item.cpu_percent).min(100.0);
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

pub fn kill_processes_by_pids(pids: &[u32]) -> bool {
    if pids.is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        if windows::terminate_processes_win32(pids) {
            return true;
        }
    }
    let mut sys = System::new();
    let spids: Vec<sysinfo::Pid> = pids.iter().map(|&p| sysinfo::Pid::from_u32(p)).collect();
    sys.refresh_processes(ProcessesToUpdate::Some(&spids), true);
    let mut any = false;
    for sp in spids {
        if let Some(p) = sys.process(sp) {
            if p.kill() {
                any = true;
            }
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_live_telemetry_sanity() {
        let mut sys = System::new_all();
        std::thread::sleep(Duration::from_millis(250));
        sys.refresh_cpu_specifics(sysinfo::CpuRefreshKind::everything());
        sys.refresh_memory();
        let cpu_pct = sys.global_cpu_usage();
        let freq_mhz = sys.cpus().first().map(|c| c.frequency()).unwrap_or(0);
        let tot_gb = sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
        let used_gb = sys.used_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
        assert!(tot_gb > 1.0 && used_gb > 0.1 && used_gb <= tot_gb + 0.1);
        assert!((0.0..=100.0).contains(&cpu_pct));
        eprintln!(
            "LIVE_CHECK: CPU={:.1}% @ {} MHz | RAM={:.2}/{:.2} GB",
            cpu_pct, freq_mhz, used_gb, tot_gb
        );

        #[cfg(windows)]
        {
            let inst_gb = windows::get_installed_ram_gb();
            let (dgpu, igpu) = windows::discover_gpus_from_registry();
            let nvml = windows::NvmlReader::try_new();
            let mut gpu = crate::models::GpuInfo::default();
            if let Some(ref r) = nvml {
                let _ = r.poll(&mut gpu);
            }
            let pdh_gpu = windows::PdhGpuReader::try_new();
            let mut igcl = windows::IntelIgclReader::try_new();
            std::thread::sleep(Duration::from_millis(250));
            let pdh_gpu_vals = pdh_gpu.as_ref().map(|p| p.poll());
            let igcl_vals = igcl.as_mut().map(|r| r.poll());
            eprintln!("PDH_GPU_CHECK: {:?} | IGCL_CHECK: {:?}", pdh_gpu_vals, igcl_vals);
            let mut emi = windows::EmiPowerReader::new();
            std::thread::sleep(Duration::from_millis(300));
            let pwr = windows::poll_power_info(
                &mut emi,
                None,
                None,
                gpu.power_watts,
                gpu.power_limit_watts,
                cpu_pct,
                freq_mhz as f32 / 1000.0,
                freq_mhz as f32 / 1000.0,
            );
            let pdh = windows::PdhThermalReader::try_new();
            std::thread::sleep(Duration::from_millis(250));
            let (cpu_t, perf_pct) = pdh.as_ref().map(|r| r.poll()).unwrap_or((None, None));
            let turbo_ghz = perf_pct.map(|p| (freq_mhz as f32 / 1000.0) * (p / 100.0));
            eprintln!(
                "WIN_CHECK: PhysRAM={:?} | CPU_T={:?} | TurboGHz={:?} | NVML={} temp={:?} util={:.0}% vram={:.2}/{:.2}GB W={:?}/{:?} | RegDGPU={:?} | RegIGPU={:?} | Power: cpuW={:?} gpuW={:?} batW={:?} devW={:?} src='{}'",
                inst_gb,
                cpu_t,
                turbo_ghz,
                gpu.name,
                gpu.temperature_c,
                gpu.usage_percent,
                gpu.used_vram_gb,
                gpu.total_vram_gb,
                gpu.power_watts,
                gpu.power_limit_watts,
                dgpu.map(|g| (g.name, g.total_vram_gb)),
                igpu.map(|g| (g.name, g.total_vram_gb)),
                pwr.cpu_watts,
                pwr.gpu_watts,
                pwr.battery_watts,
                pwr.device_total_watts,
                pwr.source_note
            );
        }
    }
}
