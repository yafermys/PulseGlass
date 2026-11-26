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
    CONTROLTRACE_HANDLE, ENABLE_TRACE_PARAMETERS, ENABLE_TRACE_PARAMETERS_VERSION_2,
    EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0,
    EVENT_FILTER_DESCRIPTOR, EVENT_FILTER_TYPE_EVENT_ID, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP,
    EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
    PROCESSTRACE_HANDLE, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_RAW_TIMESTAMP,
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

fn query_storage_temp_prop10(h: HANDLE) -> Option<f32> {
    // StorageDeviceTemperatureProperty = 10 -> STORAGE_TEMPERATURE_DATA_DESCRIPTOR
    // Header (16B): Version(4), Size(4), CriticalTemp(2), WarningTemp(2), InfoCount(2), Reserved0(2)
    // Reserved1(8B), then STORAGE_TEMPERATURE_INFO[0] at offset 24:
    // Index(2), Temperature(i16 at offset 26, in Celsius), OverThreshold(2), UnderThreshold(2), ...
    let mut in_buf = [0u8; 12];
    in_buf[0..4].copy_from_slice(&10u32.to_le_bytes());
    in_buf[4..8].copy_from_slice(&0u32.to_le_bytes());
    let mut out_buf = [0u8; 128];
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
    if ok != 0 && ret >= 28 {
        let info_count = u16::from_le_bytes([out_buf[12], out_buf[13]]);
        if info_count > 0 {
            let temp_c = i16::from_le_bytes([out_buf[26], out_buf[27]]) as f32;
            if temp_c > 5.0 && temp_c < 110.0 {
                return Some(temp_c);
            }
        }
    }
    None
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

        let nvme_temp = query_storage_temp_prop10(h).or_else(|| query_nvme_temp_prop50(h));
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
        let get_power_limit = load_sym!("nvmlDeviceGetEnforcedPowerLimit", FnNvmlDeviceGetPowerLimit)
            .or_else(|| load_sym!("nvmlDeviceGetPowerManagementLimit", FnNvmlDeviceGetPowerLimit));
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

    /// Polls live NVIDIA GPU metrics via NVML. Returns `true` if at least one metric succeeded.
    pub fn poll(&self, gpu: &mut GpuInfo) -> bool {
        gpu.name = self.name.clone();
        gpu.driver_version = self.driver.clone();
        gpu.is_available = true;
        gpu.is_integrated = false;

        let mut any_ok = false;
        if let Some(f) = self.get_util {
            let mut u = NvmlUtilization { gpu: 0, memory: 0 };
            if unsafe { f(self.device, &mut u) } == 0 {
                gpu.usage_percent = u.gpu as f32;
                any_ok = true;
            }
        }
        if let Some(f) = self.get_temp {
            let mut t = 0u32;
            if unsafe { f(self.device, 0, &mut t) } == 0 {
                gpu.temperature_c = Some(t as f32);
                any_ok = true;
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
                any_ok = true;
            }
        }
        if let Some(f) = self.get_clock {
            let mut clk = 0u32;
            if unsafe { f(self.device, 0, &mut clk) } == 0 {
                gpu.clock_mhz = Some(clk);
                any_ok = true;
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
                any_ok = true;
            }
        }
        if let Some(f) = self.get_power_limit {
            let mut mw = 0u32;
            if unsafe { f(self.device, &mut mw) } == 0 && mw > 0 {
                gpu.power_limit_watts = Some((mw as f32) / 1000.0);
            }
        }
        any_ok
    }
}

// ============================================================================
// 2B. UNIVERSAL WINDOWS DXGI + PDH GPU MONITOR (ALL GPUs: NVIDIA, AMD, Intel iGPU!)
// ============================================================================

#[repr(C)]
struct DxgiAdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    luid_low: u32,
    luid_high: i32,
    flags: u32,
}

#[derive(Clone, Debug)]
pub struct DxgiAdapterEntry {
    pub name: String,
    pub luid_tag: String,
    pub is_integrated: bool,
    pub total_vram_gb: f32,
    pub shared_vram_gb: f32,
}

/// Enumerates real hardware GPUs via `dxgi.dll` (`CreateDXGIFactory1` -> `EnumAdapters1`)
/// to map exact Windows Adapter LUIDs (`luid_0xHHHHHHHH_0xLLLLLLLL`) to dGPU & iGPU.
pub fn enumerate_dxgi_adapters() -> Vec<DxgiAdapterEntry> {
    type FnCreateFactory1 = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> i32;
    type FnEnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
    type FnGetDesc1 = unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32;
    type FnRelease = unsafe extern "system" fn(*mut c_void) -> u32;

    const IID_IDXGIFACTORY1: GUID = GUID {
        data1: 0x770A_AE78,
        data2: 0xF26F,
        data3: 0x4DBA,
        data4: [0xA8, 0x29, 0x25, 0x3C, 0x83, 0xD1, 0xB3, 0x87],
    };

    let mut out = Vec::new();
    let w = to_wide("dxgi.dll");
    unsafe {
        let m = LoadLibraryW(w.as_ptr());
        if m.is_null() {
            return out;
        }
        let Some(p_create) = GetProcAddress(m, b"CreateDXGIFactory1\0".as_ptr()) else {
            return out;
        };
        let create_factory: FnCreateFactory1 = std::mem::transmute(p_create);
        let mut factory: *mut c_void = std::ptr::null_mut();
        if create_factory(&IID_IDXGIFACTORY1, &mut factory) != 0 || factory.is_null() {
            return out;
        }

        let fvtable = *(factory as *const *const usize);
        let enum_adapters1: FnEnumAdapters1 = std::mem::transmute(*fvtable.add(12));
        let release_factory: FnRelease = std::mem::transmute(*fvtable.add(2));

        let mut idx = 0u32;
        loop {
            let mut adapter: *mut c_void = std::ptr::null_mut();
            if enum_adapters1(factory, idx, &mut adapter) != 0 || adapter.is_null() {
                break;
            }
            idx += 1;

            let avtable = *(adapter as *const *const usize);
            let get_desc1: FnGetDesc1 = std::mem::transmute(*avtable.add(10));
            let release_adapter: FnRelease = std::mem::transmute(*avtable.add(2));

            let mut desc: DxgiAdapterDesc1 = std::mem::zeroed();
            if get_desc1(adapter, &mut desc) == 0 {
                // Skip software adapters (DXGI_ADAPTER_FLAG_SOFTWARE = 2)
                if (desc.flags & 2) == 0 {
                    let end = desc
                        .description
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.description.len());
                    let name = String::from_utf16_lossy(&desc.description[..end])
                        .trim()
                        .to_string();
                    let nl = name.to_ascii_lowercase();
                    let ded_gb = (desc.dedicated_video_memory as f64) / (1024.0 * 1024.0 * 1024.0);
                    let shr_gb = (desc.shared_system_memory as f64) / (1024.0 * 1024.0 * 1024.0);
                    let is_integrated = (desc.vendor_id == 0x8086 && !nl.contains("arc"))
                        || nl.contains("uhd")
                        || nl.contains("iris")
                        || nl.contains("hd graphics")
                        || (nl.contains("radeon(tm) graphics") && ded_gb < 1.5)
                        || ded_gb < 0.6;
                    let luid_tag = format!(
                        "luid_0x{:08x}_0x{:08x}",
                        desc.luid_high as u32, desc.luid_low
                    );
                    out.push(DxgiAdapterEntry {
                        name,
                        luid_tag,
                        is_integrated,
                        total_vram_gb: ded_gb as f32,
                        shared_vram_gb: shr_gb as f32,
                    });
                }
            }
            release_adapter(adapter);
        }
        release_factory(factory);
    }
    out
}

#[repr(C)]
struct PdhFmtCounterValueItemW {
    sz_name: *mut u16,
    c_status: u32,
    _pad: u32,
    double_value: f64,
}

type FnPdhGetFormattedCounterArrayW = unsafe extern "system" fn(
    *mut c_void,
    u32,
    *mut u32,
    *mut u32,
    *mut PdhFmtCounterValueItemW,
) -> i32;

/// Native Windows PDH GPU Engine & Adapter Memory reader (works for ALL GPUs: NVIDIA, AMD, Intel iGPU!).
/// Reads exact Windows Task Manager GPU utilization & VRAM usage with zero subprocesses.
pub struct PdhGpuReader {
    query: *mut c_void,
    engine_counter: *mut c_void,
    ded_mem_counter: *mut c_void,
    shr_mem_counter: *mut c_void,
    collect_fn: FnPdhCollectQueryData,
    get_array_fn: FnPdhGetFormattedCounterArrayW,
    close_fn: FnPdhCloseQuery,
    pub dgpu_adapter: Option<DxgiAdapterEntry>,
    pub igpu_adapter: Option<DxgiAdapterEntry>,
}

unsafe impl Send for PdhGpuReader {}

impl Drop for PdhGpuReader {
    fn drop(&mut self) {
        if !self.query.is_null() {
            unsafe { (self.close_fn)(self.query) };
        }
    }
}

impl PdhGpuReader {
    pub fn try_new() -> Option<Self> {
        let adapters = enumerate_dxgi_adapters();
        let dgpu_adapter = adapters.iter().find(|a| !a.is_integrated).cloned();
        let igpu_adapter = adapters.iter().find(|a| a.is_integrated).cloned();

        let w = to_wide("pdh.dll");
        unsafe {
            let m = LoadLibraryW(w.as_ptr());
            if m.is_null() {
                return None;
            }
            let open_fn: FnPdhOpenQueryW =
                std::mem::transmute(GetProcAddress(m, b"PdhOpenQueryW\0".as_ptr())?);
            let add_fn: FnPdhAddEnglishCounterW =
                std::mem::transmute(GetProcAddress(m, b"PdhAddEnglishCounterW\0".as_ptr())?);
            let collect_fn: FnPdhCollectQueryData =
                std::mem::transmute(GetProcAddress(m, b"PdhCollectQueryData\0".as_ptr())?);
            let get_array_fn: FnPdhGetFormattedCounterArrayW = std::mem::transmute(
                GetProcAddress(m, b"PdhGetFormattedCounterArrayW\0".as_ptr())?,
            );
            let close_fn: FnPdhCloseQuery =
                std::mem::transmute(GetProcAddress(m, b"PdhCloseQuery\0".as_ptr())?);

            let mut query: *mut c_void = std::ptr::null_mut();
            if open_fn(std::ptr::null(), 0, &mut query) != 0 || query.is_null() {
                return None;
            }

            let mut engine_counter: *mut c_void = std::ptr::null_mut();
            let mut ded_mem_counter: *mut c_void = std::ptr::null_mut();
            let mut shr_mem_counter: *mut c_void = std::ptr::null_mut();

            let p_eng = to_wide(r"\GPU Engine(*)\Utilization Percentage");
            let _ = add_fn(query, p_eng.as_ptr(), 0, &mut engine_counter);
            let p_ded = to_wide(r"\GPU Adapter Memory(*)\Dedicated Usage");
            let _ = add_fn(query, p_ded.as_ptr(), 0, &mut ded_mem_counter);
            let p_shr = to_wide(r"\GPU Adapter Memory(*)\Shared Usage");
            let _ = add_fn(query, p_shr.as_ptr(), 0, &mut shr_mem_counter);

            if engine_counter.is_null() && ded_mem_counter.is_null() {
                close_fn(query);
                return None;
            }
            let _ = collect_fn(query);
            Some(Self {
                query,
                engine_counter,
                ded_mem_counter,
                shr_mem_counter,
                collect_fn,
                get_array_fn,
                close_fn,
                dgpu_adapter,
                igpu_adapter,
            })
        }
    }

    fn read_counter_array(&self, counter: *mut c_void) -> Vec<(String, f64)> {
        if counter.is_null() {
            return Vec::new();
        }
        const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
        let mut buf_size = 0u32;
        let mut count = 0u32;
        unsafe {
            let _ = (self.get_array_fn)(
                counter,
                PDH_FMT_DOUBLE,
                &mut buf_size,
                &mut count,
                std::ptr::null_mut(),
            );
            if buf_size == 0 || count == 0 || buf_size > 8 * 1024 * 1024 {
                return Vec::new();
            }
            let mut raw = vec![0u8; buf_size as usize];
            let items = raw.as_mut_ptr() as *mut PdhFmtCounterValueItemW;
            let st = (self.get_array_fn)(counter, PDH_FMT_DOUBLE, &mut buf_size, &mut count, items);
            if st != 0 {
                return Vec::new();
            }
            let mut res = Vec::with_capacity(count as usize);
            for i in 0..(count as usize) {
                let it = &*items.add(i);
                if it.c_status == 0 && !it.sz_name.is_null() && it.double_value.is_finite() {
                    let mut len = 0usize;
                    while *it.sz_name.add(len) != 0 && len < 512 {
                        len += 1;
                    }
                    let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.sz_name, len))
                        .to_ascii_lowercase();
                    res.push((name, it.double_value));
                }
            }
            res
        }
    }

    /// Returns `(dgpu_util_pct, dgpu_used_vram_gb, igpu_util_pct, igpu_used_vram_gb)`
    pub fn poll(&self) -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>) {
        if unsafe { (self.collect_fn)(self.query) } != 0 {
            return (None, None, None, None);
        }

        let d_tag = self.dgpu_adapter.as_ref().map(|a| a.luid_tag.as_str());
        let i_tag = self.igpu_adapter.as_ref().map(|a| a.luid_tag.as_str());

        // Sum utilization per (adapter_is_igpu, engine_type) then take max engine (Task Manager formula)
        let mut d_eng: HashMap<String, f64> = HashMap::new();
        let mut i_eng: HashMap<String, f64> = HashMap::new();
        for (name, val) in self.read_counter_array(self.engine_counter) {
            if val <= 0.0 || val > 100.0 {
                continue;
            }
            let eng_key = name
                .rsplit("engtype_")
                .next()
                .unwrap_or("3d")
                .to_string();
            if let Some(dt) = d_tag {
                if name.contains(dt) {
                    *d_eng.entry(eng_key).or_insert(0.0) += val;
                    continue;
                }
            }
            if let Some(it) = i_tag {
                if name.contains(it) {
                    *i_eng.entry(eng_key).or_insert(0.0) += val;
                }
            }
        }
        let d_util = if d_tag.is_some() {
            Some(
                d_eng
                    .values()
                    .copied()
                    .fold(0.0f64, f64::max)
                    .clamp(0.0, 100.0) as f32,
            )
        } else {
            None
        };
        let i_util = if i_tag.is_some() {
            Some(
                i_eng
                    .values()
                    .copied()
                    .fold(0.0f64, f64::max)
                    .clamp(0.0, 100.0) as f32,
            )
        } else {
            None
        };

        let mut d_mem_gb = None;
        let mut i_mem_gb = None;
        for (name, val) in self.read_counter_array(self.ded_mem_counter) {
            let gb = (val / (1024.0 * 1024.0 * 1024.0)) as f32;
            if let Some(dt) = d_tag {
                if name.contains(dt) && gb >= 0.0 {
                    d_mem_gb = Some(gb);
                }
            }
            if let Some(it) = i_tag {
                if name.contains(it) && gb > 0.01 {
                    i_mem_gb = Some(gb);
                }
            }
        }
        // Integrated GPUs mainly allocate from Shared System Memory -> read Shared Usage for iGPU!
        for (name, val) in self.read_counter_array(self.shr_mem_counter) {
            let gb = (val / (1024.0 * 1024.0 * 1024.0)) as f32;
            if let Some(it) = i_tag {
                if name.contains(it) && gb >= 0.0 {
                    i_mem_gb = Some(i_mem_gb.unwrap_or(0.0) + gb);
                }
            }
        }
        (d_util, d_mem_gb, i_util, i_mem_gb)
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

// ============================================================================
// 4B. INTEGRATED PAWNIO SIGNED MSR DRIVER READER (Zero WinRing0!)
// ============================================================================

const PAWNIO_INTEL_MSR_BIN: &[u8] = include_bytes!("../../Assets/pawnio/IntelMSR.bin");
const PAWNIO_AMD_MSR_BIN: &[u8] = include_bytes!("../../Assets/pawnio/AMDFamily17.bin");
const IOCTL_PIO_LOAD_BINARY: u32 = (41394u32 << 16) | (0x821u32 << 2);
const IOCTL_PIO_EXECUTE_FN: u32 = (41394u32 << 16) | (0x841u32 << 2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum CpuVendor {
    Intel,
    Amd,
}

pub struct PawnIoReader {
    handle: HANDLE,
    vendor: CpuVendor,
    energy_unit: f64,
    tj_max: f32,
    last_pkg_energy: Option<(u32, Instant)>,
    last_psys_energy: Option<(u32, Instant)>,
}

unsafe impl Send for PawnIoReader {}

impl Drop for PawnIoReader {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE && !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

impl PawnIoReader {
    pub fn try_new(cpu_brand: &str) -> Option<Self> {
        let vendor = if cpu_brand.to_ascii_lowercase().contains("amd") {
            CpuVendor::Amd
        } else {
            CpuVendor::Intel
        };
        let dev_path = to_wide(r"\\?\GLOBALROOT\Device\PawnIO");
        let h = unsafe {
            CreateFileW(
                dev_path.as_ptr(),
                GENERIC_READ_FLAG | GENERIC_WRITE_FLAG,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return None;
        }

        let module_bytes = match vendor {
            CpuVendor::Intel => PAWNIO_INTEL_MSR_BIN,
            CpuVendor::Amd => PAWNIO_AMD_MSR_BIN,
        };
        let mut ret = 0u32;
        let loaded = unsafe {
            DeviceIoControl(
                h,
                IOCTL_PIO_LOAD_BINARY,
                module_bytes.as_ptr() as *const c_void,
                module_bytes.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut ret,
                std::ptr::null_mut(),
            )
        };
        if loaded == 0 {
            unsafe { CloseHandle(h) };
            return None;
        }

        let mut reader = Self {
            handle: h,
            vendor,
            energy_unit: 0.000_061_035_156_25,
            tj_max: 100.0,
            last_pkg_energy: None,
            last_psys_energy: None,
        };

        match vendor {
            CpuVendor::Intel => {
                if let Some(unit_msr) = reader.read_msr(0x606) {
                    let esu = ((unit_msr >> 8) & 0x1F) as u32;
                    if esu > 0 && esu < 31 {
                        reader.energy_unit = 0.5_f64.powi(esu as i32);
                    }
                }
                if let Some(target_msr) = reader.read_msr(0x1A2) {
                    let tj = ((target_msr >> 16) & 0xFF) as f32;
                    if (70.0..=115.0).contains(&tj) {
                        reader.tj_max = tj;
                    }
                }
            }
            CpuVendor::Amd => {
                if let Some(unit_msr) = reader.read_msr(0xC001_0299) {
                    let esu = ((unit_msr >> 8) & 0x1F) as u32;
                    if esu > 0 && esu < 31 {
                        reader.energy_unit = 0.5_f64.powi(esu as i32);
                    }
                }
            }
        }
        let _ = reader.poll();
        Some(reader)
    }

    fn read_msr(&self, msr: u32) -> Option<u64> {
        let mut in_buf = [0u8; 40];
        let fn_name = b"ioctl_read_msr";
        in_buf[..fn_name.len()].copy_from_slice(fn_name);
        in_buf[32..40].copy_from_slice(&(msr as u64).to_le_bytes());

        let mut out_val = 0u64;
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_PIO_EXECUTE_FN,
                in_buf.as_ptr() as *const c_void,
                in_buf.len() as u32,
                &mut out_val as *mut u64 as *mut c_void,
                8,
                &mut ret,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 && ret >= 8 {
            Some(out_val)
        } else {
            None
        }
    }

    /// Returns `(cpu_temp_c, cpu_pkg_watts, platform_psys_watts)`
    pub fn poll(&mut self) -> (Option<f32>, Option<f32>, Option<f32>) {
        let now = Instant::now();
        let mut temp_c = None;
        let mut pkg_w = None;
        let mut psys_w = None;

        match self.vendor {
            CpuVendor::Intel => {
                for msr in [0x1B1u32, 0x19Cu32] {
                    if let Some(v) = self.read_msr(msr) {
                        let valid = (v >> 31) & 1 != 0 || msr == 0x1B1;
                        if valid {
                            let delta = ((v >> 16) & 0x7F) as f32;
                            let t = self.tj_max - delta;
                            if (10.0..=115.0).contains(&t) {
                                temp_c = Some(t);
                                break;
                            }
                        }
                    }
                }
                if let Some(v) = self.read_msr(0x611) {
                    let cur = (v & 0xFFFF_FFFF) as u32;
                    if let Some((prev, pt)) = self.last_pkg_energy {
                        let dt = now.duration_since(pt).as_secs_f64();
                        if dt > 0.1 {
                            let w = (cur.wrapping_sub(prev) as f64) * self.energy_unit / dt;
                            if w > 0.5 && w < 350.0 {
                                pkg_w = Some(w as f32);
                            }
                        }
                    }
                    self.last_pkg_energy = Some((cur, now));
                }
                if let Some(v) = self.read_msr(0x64D) {
                    let cur = (v & 0xFFFF_FFFF) as u32;
                    if cur != 0 {
                        if let Some((prev, pt)) = self.last_psys_energy {
                            let dt = now.duration_since(pt).as_secs_f64();
                            if dt > 0.1 {
                                let w = (cur.wrapping_sub(prev) as f64) * self.energy_unit / dt;
                                if w > 2.0 && w < 450.0 {
                                    psys_w = Some(w as f32);
                                }
                            }
                        }
                        self.last_psys_energy = Some((cur, now));
                    }
                }
            }
            CpuVendor::Amd => {
                if let Some(v) = self.read_msr(0xC001_029B) {
                    let cur = (v & 0xFFFF_FFFF) as u32;
                    if let Some((prev, pt)) = self.last_pkg_energy {
                        let dt = now.duration_since(pt).as_secs_f64();
                        if dt > 0.1 {
                            let w = (cur.wrapping_sub(prev) as f64) * self.energy_unit / dt;
                            if w > 0.5 && w < 350.0 {
                                pkg_w = Some(w as f32);
                            }
                        }
                    }
                    self.last_pkg_energy = Some((cur, now));
                }
            }
        }
        (temp_c, pkg_w, psys_w)
    }
}

/// Native Win32 process image path lookup fallback when sysinfo returns None.
pub fn query_process_exe_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok != 0 && len > 0 {
            let s = String::from_utf16_lossy(&buf[..len as usize]);
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Native Win32 multi-PID termination (handles grouped apps + stubborn child trees cleanly).
pub fn terminate_processes_win32(pids: &[u32]) -> bool {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut any_killed = false;
    let mut failed_pids = Vec::new();
    for &pid in pids {
        if pid == 0 || pid == std::process::id() {
            continue;
        }
        let mut ok = false;
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                if TerminateProcess(h, 1) != 0 {
                    ok = true;
                }
                CloseHandle(h);
            }
        }
        if ok {
            any_killed = true;
        } else {
            failed_pids.push(pid);
        }
    }
    if !failed_pids.is_empty() {
        let mut cmd = std::process::Command::new("taskkill.exe");
        cmd.arg("/F");
        for pid in &failed_pids {
            cmd.arg("/PID").arg(pid.to_string());
        }
        if let Ok(st) = cmd.creation_flags(CREATE_NO_WINDOW).status() {
            if st.success() {
                any_killed = true;
            }
        }
    }
    any_killed
}

pub fn poll_power_info(
    emi: &mut EmiPowerReader,
    pawnio_pkg_w: Option<f32>,
    pawnio_psys_w: Option<f32>,
    gpu_watts: Option<f32>,
    gpu_limit: Option<f32>,
    cpu_usage_pct: f32,
    cpu_clock_ghz: f32,
    cpu_base_ghz: f32,
) -> PowerInfo {
    let cpu_hw_w = pawnio_pkg_w.or_else(|| emi.poll_cpu_watts());
    let mut p = PowerInfo {
        cpu_watts: cpu_hw_w,
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
            p.is_estimated = false;
            p.source_note = "Pil / Toplam Cihaz".to_string();
            return p;
        }
    }

    if let Some(psys) = pawnio_psys_w {
        let total = match p.gpu_watts {
            Some(gw) if psys < gw + 5.0 => psys + gw,
            _ => psys,
        };
        p.device_total_watts = Some(total);
        p.is_estimated = false;
        p.source_note = "PawnIO Donanım (PSys)".to_string();
        return p;
    }

    if let Some(cw) = p.cpu_watts {
        let src_tag = if pawnio_pkg_w.is_some() {
            "PawnIO RAPL"
        } else {
            "EMI"
        };
        let base_sys = 8.0;
        if let Some(gw) = p.gpu_watts {
            p.device_total_watts = Some(cw + gw + base_sys);
            p.is_estimated = false;
            p.source_note = format!("{} + GPU + Sistem", src_tag);
        } else {
            p.device_total_watts = Some(cw + base_sys);
            p.is_estimated = false;
            p.source_note = format!("{} + Sistem", src_tag);
        }
        return p;
    }

    let ratio = if cpu_base_ghz > 0.2 {
        (cpu_clock_ghz / cpu_base_ghz).clamp(0.4, 2.2)
    } else {
        1.0
    };
    let est_cpu_w = 5.0 + (cpu_usage_pct.clamp(0.0, 100.0) / 100.0) * 38.0 * ratio.powf(1.2);
    let base_platform_w = 9.0;
    let total_est = p.gpu_watts.unwrap_or(0.0) + est_cpu_w + base_platform_w;
    p.device_total_watts = Some(total_est);
    p.is_estimated = true;
    p.source_note = if p.gpu_watts.is_some() {
        "Tahmini (GPU + CPU/Sistem) — Kesin ölçüm için PawnIO".to_string()
    } else {
        "Tahmini (CPU/Sistem) — Kesin ölçüm için PawnIO".to_string()
    };
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

// Microsoft-Windows-D3D9 = {783ACA0A-790E-4D7F-8451-AA850511C6B9}
const D3D9_PROVIDER_GUID: GUID = GUID {
    data1: 0x783A_CA0A,
    data2: 0x790E,
    data3: 0x4D7F,
    data4: [0x84, 0x51, 0xAA, 0x85, 0x05, 0x11, 0xC6, 0xB9],
};

// Microsoft-Windows-DxgKrnl = {802EC45A-1E99-4B83-9920-87C98277BA9D} (Verified from Intel PresentMon)
const DXGKRNL_PROVIDER_GUID: GUID = GUID {
    data1: 0x802E_C45A,
    data2: 0x1E99,
    data3: 0x4B83,
    data4: [0x99, 0x20, 0x87, 0xC9, 0x82, 0x77, 0xBA, 0x9D],
};

const ETW_SESSION_NAME: &str = "PulseGlass_FPS_Session";

#[repr(C)]
struct TracePropertiesBuf {
    props: EVENT_TRACE_PROPERTIES,
    session_name: [u16; 256],
}

#[repr(C)]
struct EventFilterEventIdBuf {
    filter_in: u8,
    reserved: u8,
    count: u16,
    events: [u16; 4],
}

#[derive(Clone, Copy)]
struct FrameSample {
    qpc: i64,
    arrived: Instant,
}

struct EtwFpsState {
    self_pid: u32,
    qpc_freq: i64,
    /// Tier 1: DXGI (DX10/11/12) + D3D9 Present_Start per PID
    api_frames: HashMap<u32, VecDeque<FrameSample>>,
    /// Tier 2: DxgKrnl Present_Info (184) per PID (OpenGL / Vulkan / DXGI fallback)
    krnl_present_frames: HashMap<u32, VecDeque<FrameSample>>,
    /// Tier 3: DxgKrnl PresentHistory_Start (171) per PID (FlipModel / windowed fallback)
    krnl_history_frames: HashMap<u32, VecDeque<FrameSample>>,
}

static ETW_STATE: std::sync::OnceLock<Arc<Mutex<EtwFpsState>>> = std::sync::OnceLock::new();

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn query_qpc_freq() -> i64 {
    type FnQpf = unsafe extern "system" fn(*mut i64) -> i32;
    let w = to_wide("kernel32.dll");
    unsafe {
        let m = LoadLibraryW(w.as_ptr());
        if !m.is_null() {
            if let Some(p) = GetProcAddress(m, b"QueryPerformanceFrequency\0".as_ptr()) {
                let f: FnQpf = std::mem::transmute(p);
                let mut freq = 0i64;
                if f(&mut freq) != 0 && freq > 0 {
                    return freq;
                }
            }
        }
    }
    10_000_000
}

unsafe fn enable_provider_filtered(
    session_handle: CONTROLTRACE_HANDLE,
    session_guid: GUID,
    provider_guid: &GUID,
    match_any_keyword: u64,
    event_ids: &[u16],
) -> u32 {
    if !event_ids.is_empty() && event_ids.len() <= 4 {
        let mut filter_id = EventFilterEventIdBuf {
            filter_in: 1, // TRUE: keep only listed Event IDs in kernel
            reserved: 0,
            count: event_ids.len() as u16,
            events: [0u16; 4],
        };
        for (i, &id) in event_ids.iter().enumerate() {
            filter_id.events[i] = id;
        }
        let filter_bytes = 4 + event_ids.len() * 2;
        let mut filter_desc = EVENT_FILTER_DESCRIPTOR {
            Ptr: (&filter_id as *const EventFilterEventIdBuf) as u64,
            Size: filter_bytes as u32,
            Type: EVENT_FILTER_TYPE_EVENT_ID,
        };
        let params = ENABLE_TRACE_PARAMETERS {
            Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
            EnableProperty: EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0,
            ControlFlags: 0,
            SourceId: session_guid,
            EnableFilterDesc: &mut filter_desc,
            FilterDescCount: 1,
        };
        let st = EnableTraceEx2(
            session_handle,
            provider_guid,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER,
            5, // TRACE_LEVEL_VERBOSE (accepts win_LogAlways=0 .. Verbose=5)
            match_any_keyword,
            0,
            0,
            &params,
        );
        if st == 0 {
            return 0;
        }
    }

    // Fallback without Event ID filter if OS rejected filter descriptor
    EnableTraceEx2(
        session_handle,
        provider_guid,
        EVENT_CONTROL_CODE_ENABLE_PROVIDER,
        5,
        match_any_keyword,
        0,
        0,
        std::ptr::null(),
    )
}

unsafe extern "system" fn etw_record_callback(record: *mut EVENT_RECORD) {
    if record.is_null() {
        return;
    }
    let hdr = &(*record).EventHeader;
    let pid = hdr.ProcessId;
    if pid == 0 {
        return;
    }
    let id = hdr.EventDescriptor.Id;
    let opcode = hdr.EventDescriptor.Opcode;
    let prov = &hdr.ProviderId;

    // Exact PresentMon (Provider, EventId, Opcode) classification
    let tier: u8 = if (guid_eq(prov, &DXGI_PROVIDER_GUID) && (id == 42 || id == 55) && opcode == 1)
        || (guid_eq(prov, &D3D9_PROVIDER_GUID) && id == 1 && opcode == 1)
    {
        1
    } else if guid_eq(prov, &DXGKRNL_PROVIDER_GUID) && id == 184 && opcode == 0 {
        2
    } else if guid_eq(prov, &DXGKRNL_PROVIDER_GUID) && id == 171 && opcode == 1 {
        3
    } else {
        0
    };

    if tier == 0 {
        return;
    }

    if let Some(state_arc) = ETW_STATE.get() {
        if let Ok(mut st) = state_arc.try_lock() {
            if pid == st.self_pid {
                return;
            }
            let sample = FrameSample {
                qpc: hdr.TimeStamp,
                arrived: Instant::now(),
            };
            let map = match tier {
                1 => &mut st.api_frames,
                2 => &mut st.krnl_present_frames,
                _ => &mut st.krnl_history_frames,
            };
            let q = map.entry(pid).or_default();
            q.push_back(sample);
            if q.len() > 1200 {
                q.pop_front();
            }
        }
    }
}

pub struct EtwFpsMonitor {
    trace_handle: u64,
    session_handle: u64,
    _session_w: Vec<u16>,
    running: bool,
}

impl EtwFpsMonitor {
    pub fn start() -> Self {
        let _ = ETW_STATE.set(Arc::new(Mutex::new(EtwFpsState {
            self_pid: std::process::id(),
            qpc_freq: query_qpc_freq(),
            api_frames: HashMap::new(),
            krnl_present_frames: HashMap::new(),
            krnl_history_frames: HashMap::new(),
        })));
        Self::stop_existing_session();

        let session_w = to_wide(ETW_SESSION_NAME);
        let mut session_handle = CONTROLTRACE_HANDLE { Value: 0 };

        unsafe {
            let init_props = |buf: &mut TracePropertiesBuf| {
                std::ptr::write_bytes(buf as *mut TracePropertiesBuf, 0, 1);
                buf.props.Wnode.BufferSize = std::mem::size_of::<TracePropertiesBuf>() as u32;
                buf.props.Wnode.ClientContext = 1; // QPC
                buf.props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
                buf.props.BufferSize = 64;
                buf.props.MinimumBuffers = 32;
                buf.props.MaximumBuffers = 128;
                buf.props.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
                buf.props.FlushTimer = 1;
                buf.props.LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
            };

            let mut props_buf: TracePropertiesBuf = std::mem::zeroed();
            init_props(&mut props_buf);
            let mut st = StartTraceW(&mut session_handle, session_w.as_ptr(), &mut props_buf.props);
            if st == 183 {
                // ERROR_ALREADY_EXISTS: stop stale session and retry once
                Self::stop_existing_session();
                init_props(&mut props_buf);
                st = StartTraceW(&mut session_handle, session_w.as_ptr(), &mut props_buf.props);
            }
            if st != 0 {
                return Self {
                    trace_handle: u64::MAX,
                    session_handle: 0,
                    _session_w: session_w,
                    running: false,
                };
            }

            let session_guid = props_buf.props.Wnode.Guid;

            // Enable DXGI (keyword Events=0x2, IDs 42/55), D3D9 (keyword Events=0x2, ID 1),
            // and DxgKrnl (keyword Present|Base=0x0800_0001, IDs 184/171; never Performance kw!)
            let st_dxgi = enable_provider_filtered(
                session_handle,
                session_guid,
                &DXGI_PROVIDER_GUID,
                0x2,
                &[42, 55],
            );
            let _ = enable_provider_filtered(
                session_handle,
                session_guid,
                &D3D9_PROVIDER_GUID,
                0x2,
                &[1],
            );
            let st_krnl = enable_provider_filtered(
                session_handle,
                session_guid,
                &DXGKRNL_PROVIDER_GUID,
                0x0800_0001,
                &[184, 171],
            );

            if st_dxgi != 0 && st_krnl != 0 {
                Self::stop_existing_session();
                return Self {
                    trace_handle: u64::MAX,
                    session_handle: 0,
                    _session_w: session_w,
                    running: false,
                };
            }

            let mut logfile: EVENT_TRACE_LOGFILEW = std::mem::zeroed();
            logfile.LoggerName = session_w.as_ptr() as *mut u16;
            logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME
                | PROCESS_TRACE_MODE_EVENT_RECORD
                | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
            logfile.Anonymous2.EventRecordCallback = Some(etw_record_callback);

            let trace_handle = OpenTraceW(&mut logfile);
            if trace_handle.Value == u64::MAX {
                Self::stop_existing_session();
                return Self {
                    trace_handle: u64::MAX,
                    session_handle: 0,
                    _session_w: session_w,
                    running: false,
                };
            }

            // Use exact PerfFreq from ETW logfile header (matches Intel PresentMon)
            if logfile.LogfileHeader.PerfFreq > 0 {
                if let Some(state_arc) = ETW_STATE.get() {
                    if let Ok(mut st) = state_arc.lock() {
                        st.qpc_freq = logfile.LogfileHeader.PerfFreq;
                    }
                }
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
                session_handle: session_handle.Value,
                _session_w: session_w,
                running: true,
            }
        }
    }

    fn calc_fps_from_queue(q: &mut VecDeque<FrameSample>, now: Instant, qpc_freq: i64) -> u32 {
        // Keep a 2.4s arrival window so ETW's 1s flush latency never drops active FPS to 0/--
        let arrival_cutoff = now.checked_sub(Duration::from_millis(2400)).unwrap_or(now);
        while let Some(&front) = q.front() {
            if front.arrived < arrival_cutoff {
                q.pop_front();
            } else {
                break;
            }
        }
        let Some(&last) = q.back() else {
            return 0;
        };
        // Compute exact FPS from hardware QPC timestamps within the last 1.0 second of the stream
        let qpc_cutoff = last.qpc.saturating_sub(qpc_freq);
        let mut first_in_win: Option<i64> = None;
        let mut count_in_win: u32 = 0;
        for s in q.iter() {
            if s.qpc >= qpc_cutoff {
                if first_in_win.is_none() {
                    first_in_win = Some(s.qpc);
                }
                count_in_win += 1;
            }
        }
        if count_in_win >= 2 {
            if let Some(first_qpc) = first_in_win {
                let dt_sec = (last.qpc - first_qpc) as f64 / (qpc_freq as f64);
                if dt_sec > 0.05 {
                    let fps = ((count_in_win - 1) as f64 / dt_sec).round() as u32;
                    return fps.min(2000);
                }
            }
        }
        count_in_win
    }

    /// Returns `(foreground_app_fps, best_other_app_fps, dwm_fps)`
    fn pick_fps(
        map: &mut HashMap<u32, VecDeque<FrameSample>>,
        fg_pid: u32,
        self_pid: u32,
        dwm_pid: Option<u32>,
        now: Instant,
        qpc_freq: i64,
    ) -> (u32, u32, u32) {
        let mut fg_fps = 0u32;
        let mut max_app_fps = 0u32;
        let mut dwm_fps = 0u32;
        map.retain(|&pid, q| {
            let fps = Self::calc_fps_from_queue(q, now, qpc_freq);
            if pid != self_pid && fps > 0 {
                if Some(pid) == dwm_pid {
                    dwm_fps = fps;
                } else {
                    if pid == fg_pid {
                        fg_fps = fps;
                    }
                    if fps > max_app_fps {
                        max_app_fps = fps;
                    }
                }
            }
            !q.is_empty()
        });
        (fg_fps, max_app_fps, dwm_fps)
    }

    pub fn get_fps(&self, dwm_pid: Option<u32>) -> Option<u32> {
        if !self.running {
            return None;
        }
        let state_arc = ETW_STATE.get()?;
        let now = Instant::now();

        let mut fg_pid = 0u32;
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                GetWindowThreadProcessId(hwnd, &mut fg_pid);
            }
        }

        let mut st = state_arc.lock().ok()?;
        let self_pid = st.self_pid;
        let qpc_freq = st.qpc_freq;

        let (fg1, max1, dwm1) =
            Self::pick_fps(&mut st.api_frames, fg_pid, self_pid, dwm_pid, now, qpc_freq);
        let (fg2, max2, dwm2) =
            Self::pick_fps(&mut st.krnl_present_frames, fg_pid, self_pid, dwm_pid, now, qpc_freq);
        let (fg3, max3, dwm3) =
            Self::pick_fps(&mut st.krnl_history_frames, fg_pid, self_pid, dwm_pid, now, qpc_freq);

        // 1. Prefer active foreground app/game (DXGI/D3D9 -> DxgKrnl Present -> PresentHistory)
        for f in [fg1, fg2, fg3] {
            if f > 0 {
                return Some(f);
            }
        }
        // 2. Otherwise active background game/app (e.g. when user clicks PulseGlass widget)
        for f in [max1, max2, max3] {
            if f > 0 {
                return Some(f);
            }
        }
        // 3. Only fall back to DWM desktop compositor when actively animating/composing (>= 15 FPS)
        for f in [dwm1, dwm2, dwm3] {
            if f >= 15 {
                return Some(f);
            }
        }
        None
    }

    fn stop_existing_session() {
        let session_w = to_wide(ETW_SESSION_NAME);
        unsafe {
            let mut props_buf: TracePropertiesBuf = std::mem::zeroed();
            props_buf.props.Wnode.BufferSize = std::mem::size_of::<TracePropertiesBuf>() as u32;
            props_buf.props.LoggerNameOffset =
                std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
            let _ = ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                session_w.as_ptr(),
                &mut props_buf.props,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
    }
}

impl Drop for EtwFpsMonitor {
    fn drop(&mut self) {
        if self.session_handle != 0 {
            unsafe {
                let mut props_buf: TracePropertiesBuf = std::mem::zeroed();
                props_buf.props.Wnode.BufferSize = std::mem::size_of::<TracePropertiesBuf>() as u32;
                props_buf.props.LoggerNameOffset =
                    std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
                let _ = ControlTraceW(
                    CONTROLTRACE_HANDLE {
                        Value: self.session_handle,
                    },
                    std::ptr::null(),
                    &mut props_buf.props,
                    EVENT_TRACE_CONTROL_STOP,
                );
            }
            self.session_handle = 0;
        } else {
            Self::stop_existing_session();
        }
        if self.trace_handle != u64::MAX {
            unsafe {
                let _ = CloseTrace(PROCESSTRACE_HANDLE {
                    Value: self.trace_handle,
                });
            }
            self.trace_handle = u64::MAX;
        }
    }
}

#[cfg(test)]
mod etw_tests {
    use super::*;

    #[test]
    fn test_etw_qpc_fps_calculation_and_priority() {
        let freq: i64 = 10_000_000;
        let now = Instant::now();

        // Simulate 60 FPS frames delivered in a batch 1.1s ago (ETW 1s flush lag must NOT drop FPS to 0!)
        let mut q60 = VecDeque::new();
        let arrived_batch = now - Duration::from_millis(1100);
        for i in 0..61i64 {
            q60.push_back(FrameSample {
                qpc: 1_000_000_000 + (i * freq) / 60,
                arrived: arrived_batch,
            });
        }
        let fps60 = EtwFpsMonitor::calc_fps_from_queue(&mut q60, now, freq);
        assert!((59..=61).contains(&fps60), "Expected ~60 FPS, got {}", fps60);

        // Simulate 144 FPS game
        let mut q144 = VecDeque::new();
        for i in 0..145i64 {
            q144.push_back(FrameSample {
                qpc: 2_000_000_000 + (i * freq) / 144,
                arrived: now - Duration::from_millis(200),
            });
        }
        let fps144 = EtwFpsMonitor::calc_fps_from_queue(&mut q144, now, freq);
        assert!((142..=146).contains(&fps144), "Expected ~144 FPS, got {}", fps144);

        // Verify priority: real game PID beats DWM even when PulseGlass (self_pid) is foreground
        let mut map = HashMap::new();
        map.insert(100u32, q144); // game
        let mut q_dwm = VecDeque::new();
        for i in 0..61i64 {
            q_dwm.push_back(FrameSample {
                qpc: 3_000_000_000 + (i * freq) / 60,
                arrived: now,
            });
        }
        map.insert(200u32, q_dwm); // dwm.exe
        let (fg, max_app, dwm) = EtwFpsMonitor::pick_fps(&mut map, 999, 999, Some(200), now, freq);
        assert_eq!(fg, 0);
        assert!((142..=146).contains(&max_app));
        assert!((59..=61).contains(&dwm));
    }
}
