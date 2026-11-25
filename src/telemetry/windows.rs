#![cfg(windows)]

use crate::models::{GpuInfo, PhysicalDiskInfo, PowerInfo};
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW,
    CONTROLTRACE_HANDLE, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD,
    EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES,
    EVENT_TRACE_REAL_TIME_MODE, PROCESSTRACE_HANDLE, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, WNODE_FLAG_TRACED_GUID,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
};
use windows_sys::Win32::System::Power::{
    CallNtPowerInformation, SystemBatteryState, SYSTEM_BATTERY_STATE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId,
};
use winreg::enums::HKEY_LOCAL_MACHINE;
use winreg::RegKey;

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

pub fn is_elevated() -> bool {
    use winreg::enums::KEY_WRITE;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", KEY_WRITE)
        .is_ok()
}

// ============================================================================
// 1. STORAGE: M.2 NVMe / SATA / Intel VMD RAID / USB detection + NVMe prop=50 Temp
// ============================================================================

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
const GENERIC_READ_FLAG: u32 = 0x8000_0000;
const GENERIC_WRITE_FLAG: u32 = 0x4000_0000;

fn bus_type_name(bus: u8) -> (&'static str, &'static str) {
    match bus {
        17 => ("NVMe M.2", "NVMe"),
        11 => ("SATA", "SATA"),
        8 => ("RAID", "RAID"),
        7 => ("USB", "USB"),
        3 => ("ATA", "ATA"),
        10 => ("SAS", "SAS"),
        1 => ("SCSI", "SCSI"),
        12 => ("SD", "SD"),
        13 => ("MMC", "MMC"),
        _ => ("Disk", "Unknown"),
    }
}

fn read_cstring(buf: &[u8], offset: usize) -> String {
    if offset == 0 || offset >= buf.len() {
        return String::new();
    }
    let slice = &buf[offset..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).trim().to_string()
}

fn query_nvme_temp_prop50(h: HANDLE) -> Option<f32> {
    let mut buf = [0u8; 48 + 512];
    buf[0..4].copy_from_slice(&50u32.to_le_bytes()); // StorageDeviceProtocolSpecificProperty
    buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // PropertyStandardQuery
    buf[8..12].copy_from_slice(&3u32.to_le_bytes()); // ProtocolType = Nvme
    buf[12..16].copy_from_slice(&2u32.to_le_bytes()); // DataType = NVMeDataTypeLogPage
    buf[16..20].copy_from_slice(&2u32.to_le_bytes()); // LogPage 0x02 (SMART/Health)
    buf[20..24].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // NSID
    buf[24..28].copy_from_slice(&40u32.to_le_bytes()); // ProtocolDataOffset
    buf[28..32].copy_from_slice(&512u32.to_le_bytes()); // ProtocolDataLength

    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            buf.as_ptr() as *const c_void,
            buf.len() as u32,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 && ret >= 52 {
        let kelvin = (buf[49] as u16) | ((buf[50] as u16) << 8);
        let temp_c = (kelvin as f32) - 273.15;
        if temp_c > 5.0 && temp_c < 110.0 {
            return Some(temp_c.round());
        }
    }
    None
}

fn query_seek_penalty_is_ssd(h: HANDLE) -> Option<bool> {
    let mut in_buf = [0u8; 12];
    in_buf[0..4].copy_from_slice(&7u32.to_le_bytes()); // StorageDeviceSeekPenaltyProperty
    in_buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // PropertyStandardQuery
    let mut out_buf = [0u8; 16];
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            in_buf.as_ptr() as *const c_void,
            in_buf.len() as u32,
            out_buf.as_mut_ptr() as *mut c_void,
            out_buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 && ret >= 12 {
        // DEVICE_SEEK_PENALTY_DESCRIPTOR: Version(4), Size(4), IncursSeekPenalty(1)
        return Some(out_buf[8] == 0);
    }
    None
}

fn query_disk_length_gb(h: HANDLE) -> f32 {
    let mut out_buf = [0u8; 8];
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_GET_LENGTH_INFO,
            std::ptr::null(),
            0,
            out_buf.as_mut_ptr() as *mut c_void,
            out_buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 && ret >= 8 {
        let bytes = i64::from_le_bytes(out_buf);
        if bytes > 0 {
            return (bytes as f64 / (1024.0 * 1024.0 * 1024.0)) as f32;
        }
    }
    0.0
}

pub fn probe_physical_disks() -> Vec<PhysicalDiskInfo> {
    let mut disks = Vec::new();
    for idx in 0..8u32 {
        let path = format!(r"\\.\PhysicalDrive{}", idx);
        let wide = to_wide(&path);
        let h = unsafe {
            let mut h = CreateFileW(
                wide.as_ptr(),
                GENERIC_READ_FLAG | GENERIC_WRITE_FLAG,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                h = CreateFileW(
                    wide.as_ptr(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
            }
            h
        };
        if h == INVALID_HANDLE_VALUE {
            continue;
        }

        let mut in_buf = [0u8; 12];
        in_buf[0..4].copy_from_slice(&0u32.to_le_bytes()); // StorageDeviceProperty
        in_buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // PropertyStandardQuery
        let mut out_buf = [0u8; 1024];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                in_buf.as_ptr() as *const c_void,
                in_buf.len() as u32,
                out_buf.as_mut_ptr() as *mut c_void,
                out_buf.len() as u32,
                &mut ret,
                std::ptr::null_mut(),
            )
        };

        let mut info = PhysicalDiskInfo {
            index: idx,
            total_gb: query_disk_length_gb(h),
            ..Default::default()
        };

        let mut bus_code = 0u8;
        if ok != 0 && ret >= 36 {
            let vendor_off = u32::from_le_bytes(out_buf[12..16].try_into().unwrap()) as usize;
            let prod_off = u32::from_le_bytes(out_buf[16..20].try_into().unwrap()) as usize;
            let rev_off = u32::from_le_bytes(out_buf[20..24].try_into().unwrap()) as usize;
            bus_code = out_buf[28];

            let vendor = read_cstring(&out_buf, vendor_off);
            let prod = read_cstring(&out_buf, prod_off);
            info.firmware = read_cstring(&out_buf, rev_off);
            info.model = if !vendor.is_empty() && !prod.to_lowercase().contains(&vendor.to_lowercase()) {
                format!("{} {}", vendor, prod)
            } else if !prod.is_empty() {
                prod
            } else {
                format!("PhysicalDrive{}", idx)
            };
        } else {
            info.model = format!("PhysicalDrive{}", idx);
        }

        let nvme_temp = query_nvme_temp_prop50(h);
        info.temperature_c = nvme_temp;

        let (iface, raw) = bus_type_name(bus_code);
        info.bus_type_raw = raw.to_string();

        let is_ssd = query_seek_penalty_is_ssd(h).unwrap_or(bus_code == 17 || nvme_temp.is_some());

        // Classify clearly as NVMe M.2 vs SATA vs Intel VMD RAID vs USB
        info.interface_label = if bus_code == 17 {
            "NVMe M.2".to_string()
        } else if bus_code == 8 && nvme_temp.is_some() {
            "NVMe M.2 (Intel VMD RAID)".to_string()
        } else if bus_code == 8 {
            "RAID (Intel VMD/RST)".to_string()
        } else if bus_code == 11 {
            if is_ssd {
                "SATA SSD".to_string()
            } else {
                "SATA HDD".to_string()
            }
        } else {
            iface.to_string()
        };

        info.media_type = if bus_code == 17 || (bus_code == 8 && nvme_temp.is_some()) {
            "NVMe M.2 SSD".to_string()
        } else if is_ssd {
            "SSD".to_string()
        } else {
            "HDD".to_string()
        };

        unsafe { CloseHandle(h) };
        disks.push(info);
    }
    disks
}

// ============================================================================
// 2. NVIDIA GPU DIRECT FFI (nvml.dll — ZERO subprocesses, live GPU Wattage!)
// ============================================================================

#[repr(C)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

type FnNvmlInit = unsafe extern "C" fn() -> i32;
type FnNvmlShutdown = unsafe extern "C" fn() -> i32;
type FnNvmlDeviceGetHandleByIndex = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
type FnNvmlDeviceGetName = unsafe extern "C" fn(*mut c_void, *mut u8, u32) -> i32;
type FnNvmlDeviceGetTemp = unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32;
type FnNvmlDeviceGetUtil = unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> i32;
type FnNvmlDeviceGetMem = unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> i32;
type FnNvmlDeviceGetClock = unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32;
type FnNvmlDeviceGetFan = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type FnNvmlDeviceGetPower = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type FnNvmlDeviceGetPowerLimit = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type FnNvmlSystemGetDriver = unsafe extern "C" fn(*mut u8, u32) -> i32;

pub struct NvmlReader {
    device: *mut c_void,
    shutdown: Option<FnNvmlShutdown>,
    get_temp: Option<FnNvmlDeviceGetTemp>,
    get_util: Option<FnNvmlDeviceGetUtil>,
    get_mem: Option<FnNvmlDeviceGetMem>,
    get_clock: Option<FnNvmlDeviceGetClock>,
    get_fan: Option<FnNvmlDeviceGetFan>,
    get_power: Option<FnNvmlDeviceGetPower>,
    get_power_limit: Option<FnNvmlDeviceGetPowerLimit>,
    pub name: String,
    pub driver: String,
}

unsafe impl Send for NvmlReader {}

impl NvmlReader {
    pub fn try_new() -> Option<Self> {
        let candidates = [
            "nvml.dll",
            r"C:\Windows\System32\nvml.dll",
            r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll",
        ];
        let mut module = std::ptr::null_mut();
        for c in &candidates {
            let w = to_wide(c);
            module = unsafe { LoadLibraryW(w.as_ptr()) };
            if !module.is_null() {
                break;
            }
        }
        if module.is_null() {
            return None;
        }

        macro_rules! load_sym {
            ($name:literal, $ty:ty) => {{
                let p = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
                p.map(|f| unsafe { std::mem::transmute::<_, $ty>(f) })
            }};
        }

        let init: FnNvmlInit = load_sym!("nvmlInit_v2", FnNvmlInit)?;
        let shutdown = load_sym!("nvmlShutdown", FnNvmlShutdown);
        let get_handle: FnNvmlDeviceGetHandleByIndex =
            load_sym!("nvmlDeviceGetHandleByIndex_v2", FnNvmlDeviceGetHandleByIndex)?;
        let get_name = load_sym!("nvmlDeviceGetName", FnNvmlDeviceGetName);
        let get_temp = load_sym!("nvmlDeviceGetTemperature", FnNvmlDeviceGetTemp);
        let get_util = load_sym!("nvmlDeviceGetUtilizationRates", FnNvmlDeviceGetUtil);
        let get_mem = load_sym!("nvmlDeviceGetMemoryInfo", FnNvmlDeviceGetMem);
        let get_clock = load_sym!("nvmlDeviceGetClockInfo", FnNvmlDeviceGetClock);
        let get_fan = load_sym!("nvmlDeviceGetFanSpeed", FnNvmlDeviceGetFan);
        let get_power = load_sym!("nvmlDeviceGetPowerUsage", FnNvmlDeviceGetPower);
        let get_power_limit =
            load_sym!("nvmlDeviceGetEnforcedPowerLimit", FnNvmlDeviceGetPowerLimit);
        let get_driver = load_sym!("nvmlSystemGetDriverVersion", FnNvmlSystemGetDriver);

        if unsafe { init() } != 0 {
            return None;
        }
        let mut dev: *mut c_void = std::ptr::null_mut();
        if unsafe { get_handle(0, &mut dev) } != 0 || dev.is_null() {
            if let Some(sh) = shutdown {
                unsafe { sh() };
            }
            return None;
        }

        let mut name = String::from("NVIDIA GPU");
        if let Some(f) = get_name {
            let mut buf = [0u8; 96];
            if unsafe { f(dev, buf.as_mut_ptr(), buf.len() as u32) } == 0 {
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
            }
        }

        let mut driver = String::new();
        if let Some(f) = get_driver {
            let mut buf = [0u8; 64];
            if unsafe { f(buf.as_mut_ptr(), buf.len() as u32) } == 0 {
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                driver = String::from_utf8_lossy(&buf[..end]).trim().to_string();
            }
        }

        Some(Self {
            device: dev,
            shutdown,
            get_temp,
            get_util,
            get_mem,
            get_clock,
            get_fan,
            get_power,
            get_power_limit,
            name,
            driver,
        })
    }

    pub fn poll(&self, gpu: &mut GpuInfo) {
        gpu.name = self.name.clone();
        gpu.driver_version = self.driver.clone();
        gpu.is_available = true;
        gpu.is_integrated = false;

        if let Some(f) = self.get_util {
            let mut u = NvmlUtilization { gpu: 0, memory: 0 };
            if unsafe { f(self.device, &mut u) } == 0 {
                gpu.usage_percent = u.gpu as f32;
            }
        }
        if let Some(f) = self.get_temp {
            let mut t = 0u32;
            if unsafe { f(self.device, 0, &mut t) } == 0 {
                gpu.temperature_c = Some(t as f32);
            }
        }
        if let Some(f) = self.get_mem {
            let mut m = NvmlMemory {
                total: 0,
                free: 0,
                used: 0,
            };
            if unsafe { f(self.device, &mut m) } == 0 {
                gpu.total_vram_gb = (m.total as f64 / (1024.0 * 1024.0 * 1024.0)) as f32;
                gpu.used_vram_gb = (m.used as f64 / (1024.0 * 1024.0 * 1024.0)) as f32;
            }
        }
        if let Some(f) = self.get_clock {
            let mut clk = 0u32;
            if unsafe { f(self.device, 0, &mut clk) } == 0 {
                gpu.clock_mhz = Some(clk);
            }
        }
        if let Some(f) = self.get_fan {
            let mut fan = 0u32;
            if unsafe { f(self.device, &mut fan) } == 0 && fan <= 100 {
                gpu.fan_percent = Some(fan);
            }
        }
        if let Some(f) = self.get_power {
            let mut mw = 0u32;
            if unsafe { f(self.device, &mut mw) } == 0 && mw > 0 {
                gpu.power_watts = Some((mw as f32) / 1000.0);
            }
        }
        if let Some(f) = self.get_power_limit {
            let mut mw = 0u32;
            if unsafe { f(self.device, &mut mw) } == 0 && mw > 0 {
                gpu.power_limit_watts = Some((mw as f32) / 1000.0);
            }
        }
    }
}

impl Drop for NvmlReader {
    fn drop(&mut self) {
        if let Some(sh) = self.shutdown {
            unsafe { sh() };
        }
    }
}

fn read_reg_u64(sub: &RegKey, name: &str) -> Option<u64> {
    if let Ok(v) = sub.get_value::<u64, _>(name) {
        return Some(v);
    }
    if let Ok(v) = sub.get_value::<u32, _>(name) {
        return Some(v as u64);
    }
    if let Ok(raw) = sub.get_raw_value(name) {
        if raw.bytes.len() == 8 {
            return Some(u64::from_le_bytes(raw.bytes[..8].try_into().ok()?));
        } else if raw.bytes.len() == 4 {
            return Some(u32::from_le_bytes(raw.bytes[..4].try_into().ok()?) as u64);
        }
    }
    None
}

pub fn get_installed_ram_gb() -> Option<f32> {
    type FnGetPhysMem = unsafe extern "system" fn(*mut u64) -> i32;
    let w = to_wide("kernel32.dll");
    let m = unsafe { LoadLibraryW(w.as_ptr()) };
    if m.is_null() {
        return None;
    }
    let p = unsafe { GetProcAddress(m, b"GetPhysicallyInstalledSystemMemory\0".as_ptr()) }?;
    let f: FnGetPhysMem = unsafe { std::mem::transmute(p) };
    let mut kb = 0u64;
    if unsafe { f(&mut kb) } != 0 && kb > 0 {
        return Some((kb as f64 / (1024.0 * 1024.0)) as f32);
    }
    None
}

/// Discovers Display Adapters from Registry (finds iGPU like Intel UHD/Iris + fallback dGPU).
pub fn discover_gpus_from_registry() -> (Option<GpuInfo>, Option<GpuInfo>) {
    let mut dgpu: Option<GpuInfo> = None;
    let mut igpu: Option<GpuInfo> = None;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(class_key) = hklm.open_subkey(
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
    ) else {
        return (None, None);
    };

    for sub_name in class_key.enum_keys().flatten() {
        if !sub_name.starts_with('0') {
            continue;
        }
        let Ok(sub) = class_key.open_subkey(&sub_name) else {
            continue;
        };
        let desc: String = sub.get_value("DriverDesc").unwrap_or_default();
        if desc.is_empty() || desc.contains("Basic Display") {
            continue;
        }
        let driver: String = sub.get_value("DriverVersion").unwrap_or_default();
        let vram_bytes = read_reg_u64(&sub, "HardwareInformation.qwMemorySize")
            .or_else(|| read_reg_u64(&sub, "HardwareInformation.MemorySize"))
            .unwrap_or(0);
        let vram_gb = (vram_bytes as f64 / (1024.0 * 1024.0 * 1024.0)) as f32;

        let lower = desc.to_lowercase();
        let is_integrated = (lower.contains("intel") && !lower.contains("arc"))
            || lower.contains("uhd")
            || lower.contains("iris")
            || lower.contains("radeon(tm) graphics")
            || lower.contains("vega");

        let info = GpuInfo {
            name: desc,
            is_integrated,
            is_available: true,
            total_vram_gb: vram_gb,
            driver_version: driver,
            ..Default::default()
        };

        if is_integrated {
            if igpu.is_none() {
                igpu = Some(info);
            }
        } else if dgpu.is_none() {
            dgpu = Some(info);
        }
    }

    (dgpu, igpu)
}

// ============================================================================
// 3. CPU & DEVICE WATTAGE: Native Windows EMI (RAPL without WinRing0!) + Battery
// ============================================================================

// GUID_DEVICE_ENERGY_METER = {45BD8344-7ED6-49CF-A440-C276C933B053}
const GUID_DEVICE_ENERGY_METER: GUID = GUID {
    data1: 0x45BD_8344,
    data2: 0x7ED6,
    data3: 0x49CF,
    data4: [0xA4, 0x40, 0xC2, 0x76, 0xC9, 0x33, 0xB0, 0x53],
};

const IOCTL_EMI_GET_VERSION: u32 = 0x0022_0000;
const IOCTL_EMI_GET_METADATA_SIZE: u32 = 0x0022_0004;
const IOCTL_EMI_GET_METADATA: u32 = 0x0022_0008;
const IOCTL_EMI_GET_MEASUREMENT: u32 = 0x0022_000C;

struct EmiDevice {
    handle: HANDLE,
    version: u16,
    channel_count: usize,
    pkg_channel: Option<usize>,
    last_energy: Vec<u64>,
    last_time_100ns: Vec<u64>,
}

unsafe impl Send for EmiDevice {}

impl Drop for EmiDevice {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE && !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

pub struct EmiPowerReader {
    devices: Vec<EmiDevice>,
}

impl EmiPowerReader {
    pub fn new() -> Self {
        let mut devices = Vec::new();
        unsafe {
            let hdev = SetupDiGetClassDevsW(
                &GUID_DEVICE_ENERGY_METER,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            );
            if hdev == INVALID_HANDLE_VALUE as isize {
                return Self { devices };
            }

            let mut idx = 0u32;
            loop {
                let mut if_data: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
                if_data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if SetupDiEnumDeviceInterfaces(
                    hdev,
                    std::ptr::null(),
                    &GUID_DEVICE_ENERGY_METER,
                    idx,
                    &mut if_data,
                ) == 0
                {
                    break;
                }
                idx += 1;

                let mut req_size = 0u32;
                SetupDiGetDeviceInterfaceDetailW(
                    hdev,
                    &if_data,
                    std::ptr::null_mut(),
                    0,
                    &mut req_size,
                    std::ptr::null_mut(),
                );
                if req_size == 0 || req_size > 4096 {
                    continue;
                }
                let mut detail_buf = vec![0u8; req_size as usize];
                let detail = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

                if SetupDiGetDeviceInterfaceDetailW(
                    hdev,
                    &if_data,
                    detail,
                    req_size,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) == 0
                {
                    continue;
                }

                let path_ptr = &(*detail).DevicePath as *const u16;
                let h = CreateFileW(
                    path_ptr,
                    GENERIC_READ_FLAG,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if h == INVALID_HANDLE_VALUE {
                    continue;
                }

                let mut ver = 0u16;
                let mut ret = 0u32;
                if DeviceIoControl(
                    h,
                    IOCTL_EMI_GET_VERSION,
                    std::ptr::null(),
                    0,
                    &mut ver as *mut u16 as *mut c_void,
                    2,
                    &mut ret,
                    std::ptr::null_mut(),
                ) == 0
                {
                    CloseHandle(h);
                    continue;
                }

                let mut meta_size = 0u32;
                if DeviceIoControl(
                    h,
                    IOCTL_EMI_GET_METADATA_SIZE,
                    std::ptr::null(),
                    0,
                    &mut meta_size as *mut u32 as *mut c_void,
                    4,
                    &mut ret,
                    std::ptr::null_mut(),
                ) == 0
                    || meta_size == 0
                    || meta_size > 65536
                {
                    CloseHandle(h);
                    continue;
                }

                let mut meta = vec![0u8; meta_size as usize];
                if DeviceIoControl(
                    h,
                    IOCTL_EMI_GET_METADATA,
                    std::ptr::null(),
                    0,
                    meta.as_mut_ptr() as *mut c_void,
                    meta_size,
                    &mut ret,
                    std::ptr::null_mut(),
                ) == 0
                {
                    CloseHandle(h);
                    continue;
                }

                // Parse exact Windows SDK EMI_METADATA_V2 layout:
                // HardwareOEM[32 WCHAR] (64B) + HardwareModel[32 WCHAR] (64B) + HardwareRevision(2B) + ChannelCount(2B) = 132B
                // Each EMI_CHANNEL_V2: MeasurementUnit(4B) + ChannelNameSize(2B) + ChannelName[ChannelNameSize bytes]
                let (channel_count, pkg_channel) = if ver == 2 && meta.len() >= 132 {
                    let count = u16::from_le_bytes([meta[130], meta[131]]) as usize;
                    let count = count.min(32);
                    let mut off = 132usize;
                    let mut pkg_idx = None;
                    for ch in 0..count {
                        if off + 6 > meta.len() {
                            break;
                        }
                        let name_bytes = u16::from_le_bytes([meta[off + 4], meta[off + 5]]) as usize;
                        let name_start = off + 6;
                        let name_end = (name_start + name_bytes).min(meta.len());
                        let name_u16: Vec<u16> = meta[name_start..name_end]
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .take_while(|&v| v != 0)
                            .collect();
                        let name = String::from_utf16_lossy(&name_u16).to_lowercase();
                        if name.contains("pkg") || name.contains("package") || name.contains("cpu")
                        {
                            pkg_idx = Some(ch);
                        }
                        // EMI_CHANNEL_V2_NEXT_CHANNEL advances by 6 + name_bytes (4-byte aligned)
                        let step = (6 + name_bytes + 3) & !3;
                        off += step.max(8);
                    }
                    (count.max(1), pkg_idx.or(if count > 0 { Some(0) } else { None }))
                } else {
                    (1, Some(0))
                };

                devices.push(EmiDevice {
                    handle: h,
                    version: ver,
                    channel_count: channel_count.max(1),
                    pkg_channel,
                    last_energy: vec![0; channel_count.max(1)],
                    last_time_100ns: vec![0; channel_count.max(1)],
                });
            }

            SetupDiDestroyDeviceInfoList(hdev);
        }

        let mut reader = Self { devices };
        let _ = reader.poll_cpu_watts(); // seed baseline
        reader
    }

    pub fn poll_cpu_watts(&mut self) -> Option<f32> {
        let mut best_watts: Option<f32> = None;
        for dev in &mut self.devices {
            // Each EMI_CHANNEL_MEASUREMENT_DATA is 16 bytes (AbsoluteEnergy: u64 in pWh, AbsoluteTime: u64 in 100ns)
            let buf_len = dev.channel_count * 16;
            let mut buf = vec![0u8; buf_len];
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    dev.handle,
                    IOCTL_EMI_GET_MEASUREMENT,
                    std::ptr::null(),
                    0,
                    buf.as_mut_ptr() as *mut c_void,
                    buf_len as u32,
                    &mut ret,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || (ret as usize) < 16 {
                continue;
            }
            let ch = dev.pkg_channel.unwrap_or(0).min(dev.channel_count - 1);
            let off = ch * 16;
            if off + 16 > ret as usize {
                continue;
            }
            let energy_pwh = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
            let time_100ns = u64::from_le_bytes(buf[off + 8..off + 16].try_into().unwrap());

            let prev_e = dev.last_energy[ch];
            let prev_t = dev.last_time_100ns[ch];
            dev.last_energy[ch] = energy_pwh;
            dev.last_time_100ns[ch] = time_100ns;

            if prev_t > 0 && time_100ns > prev_t && energy_pwh >= prev_e {
                let dt_sec = (time_100ns - prev_t) as f64 * 1e-7;
                if dt_sec > 0.05 {
                    // 1 pWh = 3.6e-9 Joules => W = dE_pWh * 3.6e-9 / dt_sec
                    // Note: EMI v1/v2 reports picowatt-hours (EmiMeasurementUnitPicowattHours = 0)
                    let _ = dev.version;
                    let watts = ((energy_pwh - prev_e) as f64 * 3.6e-9) / dt_sec;
                    if watts > 0.5 && watts < 350.0 {
                        best_watts = Some(watts as f32);
                    }
                }
            }
        }
        best_watts
    }
}

pub fn poll_power_info(emi: &mut EmiPowerReader, gpu_watts: Option<f32>, gpu_limit: Option<f32>) -> PowerInfo {
    let mut p = PowerInfo {
        cpu_watts: emi.poll_cpu_watts(),
        gpu_watts,
        gpu_limit_watts: gpu_limit,
        ..Default::default()
    };

    // Query SystemBatteryState for exact laptop discharge/charge wattage
    unsafe {
        let mut sbs: SYSTEM_BATTERY_STATE = std::mem::zeroed();
        let status = CallNtPowerInformation(
            SystemBatteryState,
            std::ptr::null(),
            0,
            &mut sbs as *mut SYSTEM_BATTERY_STATE as *mut c_void,
            std::mem::size_of::<SYSTEM_BATTERY_STATE>() as u32,
        );
        if status == 0 && sbs.BatteryPresent != 0 {
            p.on_battery = sbs.AcOnLine == 0;
            if sbs.MaxCapacity > 0 {
                let pct = ((sbs.RemainingCapacity as f64 / sbs.MaxCapacity as f64) * 100.0)
                    .clamp(0.0, 100.0) as u8;
                p.battery_percent = Some(pct);
            }
            // Rate is in mW (signed): negative when discharging, positive when charging
            let rate_signed = sbs.Rate as i32;
            if rate_signed != i32::MIN && rate_signed != 0 {
                let w = (rate_signed.abs() as f32) / 1000.0;
                if w > 0.5 && w < 300.0 {
                    p.battery_watts = Some(w);
                }
            }
        }
    }

    if p.on_battery {
        if let Some(bw) = p.battery_watts {
            p.device_total_watts = Some(bw);
            p.source_note = "Pil / Toplam Cihaz".to_string();
            return p;
        }
    }

    // On AC power: only report Total Device wattage when CPU EMI is actually available;
    // never misreport GPU-only wattage as total device wattage.
    match (p.cpu_watts, p.gpu_watts) {
        (Some(cw), Some(gw)) => {
            p.device_total_watts = Some(cw + gw + 10.0);
            p.source_note = "CPU (EMI) + GPU + Sistem".to_string();
        }
        (Some(cw), None) => {
            p.device_total_watts = Some(cw + 10.0);
            p.source_note = "CPU (EMI) + Sistem".to_string();
        }
        (None, _) => {
            if let Some(bw) = p.battery_watts {
                p.device_total_watts = Some(bw);
                p.source_note = "Pil Ölçümü".to_string();
            } else {
                p.device_total_watts = None;
                p.source_note = "Adaptörde (Pil sensörü boşta)".to_string();
            }
        }
    }

    p
}

// ============================================================================
// 4. CPU TEMPERATURE: Native Windows PDH (pdh.dll) — ZERO PowerShell / ZERO console flash!
// ============================================================================

#[repr(C)]
struct PdhFmtCounterValue {
    c_status: u32,
    double_value: f64,
}

type FnPdhOpenQueryW = unsafe extern "system" fn(*const u16, usize, *mut *mut c_void) -> i32;
type FnPdhAddEnglishCounterW =
    unsafe extern "system" fn(*mut c_void, *const u16, usize, *mut *mut c_void) -> i32;
type FnPdhCollectQueryData = unsafe extern "system" fn(*mut c_void) -> i32;
type FnPdhGetFormattedCounterValue =
    unsafe extern "system" fn(*mut c_void, u32, *mut u32, *mut PdhFmtCounterValue) -> i32;
type FnPdhCloseQuery = unsafe extern "system" fn(*mut c_void) -> i32;

const PDH_FMT_DOUBLE: u32 = 0x0000_0200;

pub struct PdhThermalReader {
    query: *mut c_void,
    temp_counter: *mut c_void,
    perf_counter: *mut c_void,
    collect: Option<FnPdhCollectQueryData>,
    get_val: Option<FnPdhGetFormattedCounterValue>,
    close: Option<FnPdhCloseQuery>,
}

unsafe impl Send for PdhThermalReader {}

impl PdhThermalReader {
    pub fn try_new() -> Option<Self> {
        let w = to_wide("pdh.dll");
        let module = unsafe { LoadLibraryW(w.as_ptr()) };
        if module.is_null() {
            return None;
        }
        macro_rules! load_pdh {
            ($name:literal, $ty:ty) => {{
                let p = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
                p.map(|f| unsafe { std::mem::transmute::<_, $ty>(f) })
            }};
        }
        let open: FnPdhOpenQueryW = load_pdh!("PdhOpenQueryW", FnPdhOpenQueryW)?;
        let add: FnPdhAddEnglishCounterW =
            load_pdh!("PdhAddEnglishCounterW", FnPdhAddEnglishCounterW)?;
        let collect = load_pdh!("PdhCollectQueryData", FnPdhCollectQueryData);
        let get_val = load_pdh!("PdhGetFormattedCounterValue", FnPdhGetFormattedCounterValue);
        let close = load_pdh!("PdhCloseQuery", FnPdhCloseQuery);

        let mut query: *mut c_void = std::ptr::null_mut();
        if unsafe { open(std::ptr::null(), 0, &mut query) } != 0 || query.is_null() {
            return None;
        }

        // 1. ACPI Thermal Zone counter
        let paths = [
            r"\Thermal Zone Information(\_TZ.TZ01)\Temperature",
            r"\Thermal Zone Information(\_TZ.TZ00)\Temperature",
            r"\Thermal Zone Information(\_TZ.TZ01_0)\Temperature",
            r"\Thermal Zone Information(\_TZ.THM0)\Temperature",
            r"\Thermal Zone Information(\_TZ.THRM)\Temperature",
            r"\Thermal Zone Information(\_TZ.TZ0)\Temperature",
        ];
        let mut temp_counter: *mut c_void = std::ptr::null_mut();
        for p in &paths {
            let pw = to_wide(p);
            if unsafe { add(query, pw.as_ptr(), 0, &mut temp_counter) } == 0 && !temp_counter.is_null() {
                break;
            }
            temp_counter = std::ptr::null_mut();
        }

        // 2. Live CPU % Processor Performance (exact Task Manager Turbo GHz multiplier)
        let mut perf_counter: *mut c_void = std::ptr::null_mut();
        let perf_path = to_wide(r"\Processor Information(_Total)\% Processor Performance");
        if unsafe { add(query, perf_path.as_ptr(), 0, &mut perf_counter) } != 0 {
            perf_counter = std::ptr::null_mut();
        }

        if temp_counter.is_null() && perf_counter.is_null() {
            if let Some(c) = close {
                unsafe { c(query) };
            }
            return None;
        }
        if let Some(c) = collect {
            unsafe { c(query) };
        }
        Some(Self {
            query,
            temp_counter,
            perf_counter,
            collect,
            get_val,
            close,
        })
    }

    pub fn poll(&self) -> (Option<f32>, Option<f32>) {
        let (Some(collect), Some(get_val)) = (self.collect, self.get_val) else {
            return (None, None);
        };
        if unsafe { collect(self.query) } != 0 {
            return (None, None);
        }

        let mut temp_c = None;
        if !self.temp_counter.is_null() {
            let mut val = PdhFmtCounterValue {
                c_status: 0,
                double_value: 0.0,
            };
            let mut ctype = 0u32;
            if unsafe { get_val(self.temp_counter, PDH_FMT_DOUBLE, &mut ctype, &mut val) } == 0
                && val.c_status == 0
            {
                let k = val.double_value as f32;
                let c = if k > 200.0 { k - 273.15 } else { k };
                if c > 15.0 && c < 115.0 {
                    temp_c = Some(c.round());
                }
            }
        }

        let mut perf_pct = None;
        if !self.perf_counter.is_null() {
            let mut val = PdhFmtCounterValue {
                c_status: 0,
                double_value: 0.0,
            };
            let mut ctype = 0u32;
            if unsafe { get_val(self.perf_counter, PDH_FMT_DOUBLE, &mut ctype, &mut val) } == 0
                && val.c_status == 0
            {
                let pct = val.double_value as f32;
                if pct > 5.0 && pct < 400.0 {
                    perf_pct = Some(pct);
                }
            }
        }

        (temp_c, perf_pct)
    }
}

impl Drop for PdhThermalReader {
    fn drop(&mut self) {
        if let Some(c) = self.close {
            unsafe { c(self.query) };
        }
    }
}

// ============================================================================
// 5. LIVE FPS: RTSS Shared Memory v2 + Native Windows DXGI ETW (ABI-exact!)
// ============================================================================

pub fn read_rtss_fps() -> Option<u32> {
    let name = to_wide("RTSSSharedMemoryV2");
    unsafe {
        let hmap = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
        if hmap.is_null() {
            return None;
        }
        let view = MapViewOfFile(hmap, FILE_MAP_READ, 0, 0, 0);
        if view.Value.is_null() {
            CloseHandle(hmap);
            return None;
        }

        let base = view.Value as *const u8;
        let sig = u32::from_le_bytes(*(base as *const [u8; 4]));
        let mut best = 0u32;
        if sig == 0x5254_5353 {
            // 'RTSS'
            let entry_size = u32::from_le_bytes(*(base.add(8) as *const [u8; 4])) as usize;
            let arr_off = u32::from_le_bytes(*(base.add(12) as *const [u8; 4])) as usize;
            let arr_size = u32::from_le_bytes(*(base.add(16) as *const [u8; 4])) as usize;
            if entry_size >= 280 && entry_size < 65536 {
                for i in 0..arr_size.min(64) {
                    let e = base.add(arr_off + i * entry_size);
                    let pid = u32::from_le_bytes(*(e as *const [u8; 4]));
                    if pid == 0 {
                        continue;
                    }
                    let t0 = u32::from_le_bytes(*(e.add(268) as *const [u8; 4]));
                    let t1 = u32::from_le_bytes(*(e.add(272) as *const [u8; 4]));
                    let frames = u32::from_le_bytes(*(e.add(276) as *const [u8; 4]));
                    if t1 > t0 && frames > 0 {
                        let fps = ((frames as f64 * 1000.0) / (t1 - t0) as f64).round() as u32;
                        if fps > best && fps < 2000 {
                            best = fps;
                        }
                    }
                }
            }
        }

        UnmapViewOfFile(view);
        CloseHandle(hmap);
        if best > 0 { Some(best) } else { None }
    }
}

// Microsoft-Windows-DXGI = {CA11C036-0102-4A2D-A6AD-F03CFED5D3C9}
const DXGI_PROVIDER_GUID: GUID = GUID {
    data1: 0xCA11_C036,
    data2: 0x0102,
    data3: 0x4A2D,
    data4: [0xA6, 0xAD, 0xF0, 0x3C, 0xFE, 0xD5, 0xD3, 0xC9],
};

const ETW_SESSION_NAME: &str = "PulseGlass_DXGI_FPS_V2";

static ETW_FRAMES: std::sync::OnceLock<Arc<Mutex<HashMap<u32, VecDeque<Instant>>>>> =
    std::sync::OnceLock::new();

unsafe extern "system" fn etw_record_callback(record: *mut EVENT_RECORD) {
    if record.is_null() {
        return;
    }
    let hdr = &(*record).EventHeader;
    let id = hdr.EventDescriptor.Id;
    let opcode = hdr.EventDescriptor.Opcode;
    // DXGI Present_Start: Id 42, 44, 55 with Opcode 1
    if (id == 42 || id == 44 || id == 55) && opcode == 1 {
        let pid = hdr.ProcessId;
        if pid == 0 {
            return;
        }
        if let Some(map_arc) = ETW_FRAMES.get() {
            if let Ok(mut map) = map_arc.try_lock() {
                let q = map.entry(pid).or_default();
                q.push_back(Instant::now());
                if q.len() > 1000 {
                    q.pop_front();
                }
            }
        }
    }
}

pub struct EtwFpsMonitor {
    trace_handle: u64,
    running: bool,
}

impl EtwFpsMonitor {
    pub fn start() -> Self {
        let _ = ETW_FRAMES.set(Arc::new(Mutex::new(HashMap::new())));
        Self::stop_existing_session();

        let props_size = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
        let total_size = props_size + 512;
        let mut props_buf = vec![0u8; total_size];
        let mut session_handle = CONTROLTRACE_HANDLE { Value: 0 };
        let session_w = to_wide(ETW_SESSION_NAME);

        unsafe {
            let props = props_buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
            (*props).Wnode.BufferSize = total_size as u32;
            (*props).Wnode.ClientContext = 1; // QPC
            (*props).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            (*props).BufferSize = 64;
            (*props).MinimumBuffers = 4;
            (*props).MaximumBuffers = 16;
            (*props).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
            (*props).FlushTimer = 1;
            (*props).LoggerNameOffset = props_size as u32;

            let st = StartTraceW(&mut session_handle, session_w.as_ptr(), props);
            if st != 0 {
                return Self {
                    trace_handle: u64::MAX,
                    running: false,
                };
            }

            let st2 = EnableTraceEx2(
                session_handle,
                &DXGI_PROVIDER_GUID,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                5, // TRACE_LEVEL_VERBOSE
                u64::MAX,
                0,
                0,
                std::ptr::null(),
            );
            if st2 != 0 {
                Self::stop_existing_session();
                return Self {
                    trace_handle: u64::MAX,
                    running: false,
                };
            }

            let mut logfile: EVENT_TRACE_LOGFILEW = std::mem::zeroed();
            logfile.LoggerName = session_w.as_ptr() as *mut u16;
            logfile.Anonymous1.ProcessTraceMode =
                PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
            logfile.Anonymous2.EventRecordCallback = Some(etw_record_callback);

            let trace_handle = OpenTraceW(&mut logfile);
            if trace_handle.Value == u64::MAX {
                Self::stop_existing_session();
                return Self {
                    trace_handle: u64::MAX,
                    running: false,
                };
            }

            let th_val = trace_handle.Value;
            std::thread::Builder::new()
                .name("pulseglass-etw-fps".to_string())
                .spawn(move || {
                    let handles = [PROCESSTRACE_HANDLE { Value: th_val }];
                    let _ = ProcessTrace(handles.as_ptr(), 1, std::ptr::null(), std::ptr::null());
                })
                .ok();

            Self {
                trace_handle: th_val,
                running: true,
            }
        }
    }

    pub fn get_fps(&self) -> Option<u32> {
        if !self.running {
            return None;
        }
        let map_arc = ETW_FRAMES.get()?;
        let now = Instant::now();
        let cutoff = now.checked_sub(Duration::from_secs(1))?;

        let mut fg_pid = 0u32;
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                GetWindowThreadProcessId(hwnd, &mut fg_pid);
            }
        }

        let mut map = map_arc.lock().ok()?;
        let mut fg_fps = 0u32;
        let mut max_fps = 0u32;
        for (&pid, q) in map.iter_mut() {
            while let Some(&front) = q.front() {
                if front < cutoff {
                    q.pop_front();
                } else {
                    break;
                }
            }
            let cnt = q.len() as u32;
            if pid == fg_pid && cnt > 0 {
                fg_fps = cnt;
            }
            if cnt > max_fps {
                max_fps = cnt;
            }
        }
        if fg_fps > 0 {
            Some(fg_fps)
        } else if max_fps > 0 {
            Some(max_fps)
        } else {
            None
        }
    }

    fn stop_existing_session() {
        let props_size = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
        let total_size = props_size + 512;
        let mut props_buf = vec![0u8; total_size];
        let session_w = to_wide(ETW_SESSION_NAME);
        unsafe {
            let props = props_buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
            (*props).Wnode.BufferSize = total_size as u32;
            (*props).LoggerNameOffset = props_size as u32;
            let _ = ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                session_w.as_ptr(),
                props,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
    }
}

impl Drop for EtwFpsMonitor {
    fn drop(&mut self) {
        if self.trace_handle != u64::MAX {
            unsafe {
                let _ = CloseTrace(PROCESSTRACE_HANDLE {
                    Value: self.trace_handle,
                });
            }
            self.trace_handle = u64::MAX;
        }
        Self::stop_existing_session();
    }
}
