using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Management;
using System.Net.NetworkInformation;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using Microsoft.Win32.SafeHandles;
using PulseGlassWidget.Models;

namespace PulseGlassWidget.Services
{
    public class HardwareMonitorService : IDisposable
    {
        private CpuInfo _staticCpu = new CpuInfo();
        private GpuInfo _staticGpu = new GpuInfo();
        private GpuInfo _staticIGpu = new GpuInfo { IsIntegrated = true, IsAvailable = false, Name = "Dahili GPU Bulunamadı" };
        private RamInfo _staticRam = new RamInfo();
        private StorageInfo _staticStorage = new StorageInfo();

        private PerformanceCounter _cpuTotalCounter;
        private PerformanceCounter _cpuPerfCounter;
        private readonly List<PerformanceCounter> _thermalCounters = new List<PerformanceCounter>();
        private PerformanceCounter _diskIdleCounter;
        private PerformanceCounter _diskReadCounter;
        private PerformanceCounter _diskWriteCounter;

        private string _nvidiaSmiPath;
        private bool _checkedNvidiaSmi;

        private readonly Dictionary<int, TimeSpan> _prevProcCpu = new Dictionary<int, TimeSpan>();
        private readonly Dictionary<int, ulong> _prevProcIo = new Dictionary<int, ulong>();
        private DateTime _lastPollTime = DateTime.UtcNow;

        private long _prevNetRecv;
        private long _prevNetSent;
        private DateTime _lastNetTime = DateTime.UtcNow;
        private string _activeNicName = "Ağ";
        private long _prevCpuIdle;
        private long _prevCpuKernel;
        private long _prevCpuUser;

        private int _tickCount;
        private double? _cachedDiskTemp;
        private double? _cachedCpuTemp;
        private double _cachedGpuUtil;
        private double? _cachedGpuTemp;
        private double _cachedGpuUsedVram;
        private double _cachedGpuTotalVram;
        private double? _cachedGpuClock;
        private double? _cachedGpuFan;
        private double _cachedIGpuUtil;
        private double _cachedIGpuVram;

        private string _lhmNamespace;
        private bool _hasAcpiTemp;
        private bool _hasStorageTemp;
        private bool _hasGpuAdapterMemWmi;
        private bool _hasGpuEngineWmi;
        private readonly EtwFpsMonitor _etwFps = new EtwFpsMonitor();
        private LibreHardwareMonitor.Hardware.Computer _lhmComputer;

        public double CpuTempAlertThreshold { get; set; }
        public double GpuTempAlertThreshold { get; set; }
        public double CpuUsageAlertThreshold { get; set; }
        public double RamUsageAlertThreshold { get; set; }
        public bool PawnIoInstalled { get; private set; }

        public HardwareMonitorService()
        {
            CpuTempAlertThreshold = 85.0;
            GpuTempAlertThreshold = 85.0;
            CpuUsageAlertThreshold = 90.0;
            RamUsageAlertThreshold = 90.0;
        }

        // LibreHardwareMonitorLib reads CPU sensors through the PawnIO driver and detects it by this key.
        private static bool IsPawnIoInstalled()
        {
            try
            {
                using (var key = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO"))
                {
                    return key != null;
                }
            }
            catch { return false; }
        }

        public void InitializeStaticInfo()
        {
            PawnIoInstalled = IsPawnIoInstalled();
            try
            {
                _lhmComputer = new LibreHardwareMonitor.Hardware.Computer
                {
                    IsCpuEnabled = true,
                    IsStorageEnabled = true,
                    IsGpuEnabled = true
                };
                _lhmComputer.Open();
            }
            catch { _lhmComputer = null; }
            try { _etwFps.Start(); } catch { }
            LoadCpuStatic();
            LoadRamStatic();
            LoadGpuStatic();
            LoadStorageStatic();
            InitPerformanceCounters();
            InitNetworkBaseline();
            DetectNvidiaSmi();
            ProbeWmiCapabilities();
        }

        private void ProbeWmiCapabilities()
        {
            string[] lhmNamespaces = { @"root\LibreHardwareMonitor", @"root\OpenHardwareMonitor" };
            foreach (var ns in lhmNamespaces)
            {
                try
                {
                    using (var s = new ManagementObjectSearcher(ns, "SELECT Name FROM Sensor WHERE SensorType='Temperature'"))
                    {
                        foreach (ManagementObject o in s.Get()) { _lhmNamespace = ns; break; }
                    }
                    if (_lhmNamespace != null) break;
                }
                catch { }
            }

            try
            {
                using (var s = new ManagementObjectSearcher(@"root\WMI", "SELECT CurrentTemperature FROM MSAcpi_ThermalZoneTemperature"))
                {
                    foreach (ManagementObject o in s.Get()) { _hasAcpiTemp = true; break; }
                }
            }
            catch { _hasAcpiTemp = false; }

            try
            {
                using (var s = new ManagementObjectSearcher(@"root\Microsoft\Windows\Storage", "SELECT Temperature FROM MSFT_StorageReliabilityCounter"))
                {
                    foreach (ManagementObject o in s.Get()) { _hasStorageTemp = true; break; }
                }
            }
            catch { _hasStorageTemp = false; }

            try
            {
                using (var s = new ManagementObjectSearcher("SELECT DedicatedUsage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory"))
                {
                    foreach (ManagementObject o in s.Get()) { _hasGpuAdapterMemWmi = true; break; }
                }
            }
            catch { _hasGpuAdapterMemWmi = false; }

            try
            {
                using (var s = new ManagementObjectSearcher("SELECT UtilizationPercentage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine"))
                {
                    foreach (ManagementObject o in s.Get()) { _hasGpuEngineWmi = true; break; }
                }
            }
            catch { _hasGpuEngineWmi = false; }
        }

        private void LoadCpuStatic()
        {
            try
            {
                using (var searcher = new ManagementObjectSearcher("SELECT Name, NumberOfCores, NumberOfLogicalProcessors, MaxClockSpeed FROM Win32_Processor"))
                {
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        _staticCpu.Name = (obj["Name"] ?? "Bilinmeyen İşlemci").ToString().Trim();
                        _staticCpu.PhysicalCores = Convert.ToInt32(obj["NumberOfCores"] ?? Environment.ProcessorCount);
                        _staticCpu.LogicalProcessors = Convert.ToInt32(obj["NumberOfLogicalProcessors"] ?? Environment.ProcessorCount);
                        double maxMhz = Convert.ToDouble(obj["MaxClockSpeed"] ?? 0);
                        _staticCpu.BaseClockGHz = Math.Round(maxMhz / 1000.0, 2);
                        break;
                    }
                }
            }
            catch
            {
                _staticCpu.Name = "İşlemci (" + Environment.ProcessorCount + " Mantıksal Çekirdek)";
                _staticCpu.PhysicalCores = Environment.ProcessorCount;
                _staticCpu.LogicalProcessors = Environment.ProcessorCount;
            }
        }

        private void LoadRamStatic()
        {
            ulong installedKb;
            if (NativeMethods.GetPhysicallyInstalledSystemMemory(out installedKb) && installedKb > 0)
            {
                _staticRam.InstalledGB = Math.Round(installedKb / (1024.0 * 1024.0), 1);
            }

            var memStatus = new NativeMethods.MEMORYSTATUSEX { dwLength = (uint)System.Runtime.InteropServices.Marshal.SizeOf(typeof(NativeMethods.MEMORYSTATUSEX)) };
            if (NativeMethods.GlobalMemoryStatusEx(ref memStatus))
            {
                _staticRam.TotalGB = Math.Round(memStatus.ullTotalPhys / (1024.0 * 1024.0 * 1024.0), 1);
            }
            if (_staticRam.InstalledGB <= 0) _staticRam.InstalledGB = _staticRam.TotalGB;

            try
            {
                var slots = new List<RamSlotInfo>();
                using (var searcher = new ManagementObjectSearcher("SELECT DeviceLocator, Capacity, Speed, SMBIOSMemoryType, Manufacturer FROM Win32_PhysicalMemory"))
                {
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        double capGb = Math.Round(Convert.ToDouble(obj["Capacity"] ?? 0) / (1024.0 * 1024.0 * 1024.0), 1);
                        int speed = Convert.ToInt32(obj["Speed"] ?? 0);
                        int smbiosType = Convert.ToInt32(obj["SMBIOSMemoryType"] ?? 0);
                        string memType = DecodeMemoryType(smbiosType);
                        slots.Add(new RamSlotInfo
                        {
                            Locator = (obj["DeviceLocator"] ?? "Slot").ToString().Trim(),
                            CapacityGB = capGb,
                            SpeedMHz = speed,
                            MemoryType = memType,
                            Manufacturer = (obj["Manufacturer"] ?? "").ToString().Trim()
                        });
                    }
                }
                if (slots.Count > 0)
                {
                    _staticRam.Slots = slots;
                    _staticRam.SpeedMHz = slots.Max(s => s.SpeedMHz);
                    var firstTyped = slots.FirstOrDefault(s => !string.IsNullOrEmpty(s.MemoryType));
                    _staticRam.MemoryType = firstTyped != null ? firstTyped.MemoryType : "DDR";
                    if (_staticRam.TotalGB <= 0)
                        _staticRam.TotalGB = slots.Sum(s => s.CapacityGB);
                }
            }
            catch { }
        }

        private string DecodeMemoryType(int code)
        {
            switch (code)
            {
                case 20: return "DDR";
                case 21: return "DDR2";
                case 24: return "DDR3";
                case 26: return "DDR4";
                case 34: return "DDR5";
                default: return "RAM";
            }
        }

        private void LoadGpuStatic()
        {
            try
            {
                var found = new List<GpuInfo>();
                using (var searcher = new ManagementObjectSearcher("SELECT Name, AdapterRAM, DriverVersion FROM Win32_VideoController"))
                {
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        string name = (obj["Name"] ?? "").ToString().Trim();
                        if (string.IsNullOrEmpty(name) || name.Contains("Microsoft Basic")) continue;

                        double wmiVram = Math.Round(Convert.ToDouble(obj["AdapterRAM"] ?? 0) / (1024.0 * 1024.0 * 1024.0), 1);
                        string drv = (obj["DriverVersion"] ?? "").ToString();
                        bool isInteg = IsIntegratedGpuName(name);

                        double regVram = LookupRegistryVramGB(name);
                        double finalVram = Math.Max(wmiVram, regVram);

                        found.Add(new GpuInfo
                        {
                            Name = name,
                            TotalVramGB = finalVram,
                            DriverVersion = drv,
                            IsIntegrated = isInteg,
                            IsAvailable = true
                        });
                    }
                }

                var discrete = found.FirstOrDefault(g => !g.IsIntegrated);
                var integrated = found.FirstOrDefault(g => g.IsIntegrated);

                if (discrete != null)
                {
                    _staticGpu = discrete;
                    if (integrated != null) _staticIGpu = integrated;
                }
                else if (integrated != null)
                {
                    _staticGpu = integrated;
                    _staticGpu.IsIntegrated = false; // primary display
                    _staticIGpu = new GpuInfo { IsIntegrated = true, IsAvailable = false, Name = "Harici GPU Yok (Yalnızca Dahili)" };
                }
                else
                {
                    _staticGpu.Name = "Standart Ekran Bağdaştırıcısı";
                    _staticGpu.IsAvailable = false;
                }
            }
            catch
            {
                _staticGpu.Name = "Ekran Kartı Bilgisi Alınamadı";
            }
        }

        private bool IsIntegratedGpuName(string name)
        {
            if (string.IsNullOrEmpty(name)) return false;
            return name.IndexOf("Intel", StringComparison.OrdinalIgnoreCase) >= 0 && name.IndexOf("Arc", StringComparison.OrdinalIgnoreCase) < 0
                || name.IndexOf("UHD", StringComparison.OrdinalIgnoreCase) >= 0
                || name.IndexOf("Iris", StringComparison.OrdinalIgnoreCase) >= 0
                || name.IndexOf("Radeon(TM) Graphics", StringComparison.OrdinalIgnoreCase) >= 0
                || name.IndexOf("Vega", StringComparison.OrdinalIgnoreCase) >= 0;
        }

        private double LookupRegistryVramGB(string gpuName)
        {
            try
            {
                using (var key = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}"))
                {
                    if (key == null) return 0;
                    foreach (var subName in key.GetSubKeyNames())
                    {
                        if (!subName.StartsWith("0")) continue;
                        using (var sub = key.OpenSubKey(subName))
                        {
                            if (sub == null) continue;
                            var descObj = sub.GetValue("DriverDesc");
                            var desc = descObj != null ? descObj.ToString() : null;
                            var qwMem = sub.GetValue("HardwareInformation.qwMemorySize");
                            if (qwMem != null && desc != null && gpuName.IndexOf(desc, StringComparison.OrdinalIgnoreCase) >= 0)
                            {
                                return Math.Round(Convert.ToDouble(qwMem) / (1024.0 * 1024.0 * 1024.0), 1);
                            }
                        }
                    }
                }
            }
            catch { }
            return 0;
        }

        private void LoadStorageStatic()
        {
            var diskList = new List<DiskDetailInfo>();
            double totalAll = 0;
            double freeAll = 0;

            try
            {
                foreach (var drive in DriveInfo.GetDrives())
                {
                    if (drive.IsReady && drive.DriveType == DriveType.Fixed)
                    {
                        totalAll += drive.TotalSize / (1024.0 * 1024.0 * 1024.0);
                        freeAll += drive.AvailableFreeSpace / (1024.0 * 1024.0 * 1024.0);
                    }
                }
            }
            catch { }

            try
            {
                using (var searcher = new ManagementObjectSearcher("SELECT Model, Size, MediaType FROM Win32_DiskDrive"))
                {
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        string model = (obj["Model"] ?? "Disk").ToString().Trim();
                        double sizeGb = Math.Round(Convert.ToDouble(obj["Size"] ?? 0) / (1024.0 * 1024.0 * 1024.0), 0);
                        string media = (obj["MediaType"] ?? "Fixed").ToString();
                        diskList.Add(new DiskDetailInfo
                        {
                            Model = model,
                            TotalGB = sizeGb,
                            MediaType = media
                        });
                    }
                }
            }
            catch { }

            _staticStorage.TotalGB = Math.Round(totalAll, 0);
            _staticStorage.FreeGB = Math.Round(freeAll, 0);
            _staticStorage.Disks = diskList;
            _staticStorage.PrimaryModel = diskList.Count > 0 ? diskList[0].Model : "Sistem Diski";
        }

        private void InitPerformanceCounters()
        {
            try
            {
                // Task Manager uses "% Processor Utility" on Windows 10/11
                _cpuTotalCounter = new PerformanceCounter("Processor Information", "% Processor Utility", "_Total", true);
                _cpuTotalCounter.NextValue();
            }
            catch
            {
                try
                {
                    _cpuTotalCounter = new PerformanceCounter("Processor", "% Processor Time", "_Total", true);
                    _cpuTotalCounter.NextValue();
                }
                catch { _cpuTotalCounter = null; }
            }

            try
            {
                _cpuPerfCounter = new PerformanceCounter("Processor Information", "% Processor Performance", "_Total", true);
                _cpuPerfCounter.NextValue();
            }
            catch { _cpuPerfCounter = null; }

            try
            {
                var tzCat = new PerformanceCounterCategory("Thermal Zone Information");
                foreach (var inst in tzCat.GetInstanceNames())
                {
                    var tc = new PerformanceCounter("Thermal Zone Information", "Temperature", inst, true);
                    tc.NextValue();
                    _thermalCounters.Add(tc);
                }
            }
            catch { }

            try
            {
                _diskIdleCounter = new PerformanceCounter("PhysicalDisk", "% Idle Time", "_Total", true);
                _diskReadCounter = new PerformanceCounter("PhysicalDisk", "Disk Read Bytes/sec", "_Total", true);
                _diskWriteCounter = new PerformanceCounter("PhysicalDisk", "Disk Write Bytes/sec", "_Total", true);
                _diskIdleCounter.NextValue();
                _diskReadCounter.NextValue();
                _diskWriteCounter.NextValue();
            }
            catch { }
        }

        private void InitNetworkBaseline()
        {
            try
            {
                long recv = 0, sent = 0;
                foreach (var nic in NetworkInterface.GetAllNetworkInterfaces())
                {
                    if (nic.OperationalStatus == OperationalStatus.Up && nic.NetworkInterfaceType != NetworkInterfaceType.Loopback)
                    {
                        var stats = nic.GetIPv4Statistics();
                        recv += stats.BytesReceived;
                        sent += stats.BytesSent;
                        if (_activeNicName == "Ağ") _activeNicName = nic.Name;
                    }
                }
                _prevNetRecv = recv;
                _prevNetSent = sent;
                _lastNetTime = DateTime.UtcNow;
            }
            catch { }
        }

        private void DetectNvidiaSmi()
        {
            if (_checkedNvidiaSmi) return;
            _checkedNvidiaSmi = true;

            string[] candidates =
            {
                @"C:\Windows\System32\nvidia-smi.exe",
                @"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe"
            };

            foreach (var path in candidates)
            {
                if (File.Exists(path))
                {
                    _nvidiaSmiPath = path;
                    return;
                }
            }
        }

        public HardwareSnapshot PollSnapshot(bool includeProcesses, bool groupProcesses)
        {
            _tickCount++;
            var now = DateTime.UtcNow;
            double elapsedSec = Math.Max(0.2, (now - _lastPollTime).TotalSeconds);
            _lastPollTime = now;

            var snap = new HardwareSnapshot();

            // 1. CPU
            snap.Cpu.Name = _staticCpu.Name;
            snap.Cpu.PhysicalCores = _staticCpu.PhysicalCores;
            snap.Cpu.LogicalProcessors = _staticCpu.LogicalProcessors;
            snap.Cpu.BaseClockGHz = _staticCpu.BaseClockGHz;

            try
            {
                if (_cpuTotalCounter != null)
                {
                    snap.Cpu.UsagePercent = Math.Min(100, Math.Max(0, Math.Round(_cpuTotalCounter.NextValue(), 1)));
                }
                else
                {
                    long idle, kernel, user;
                    if (NativeMethods.GetSystemTimes(out idle, out kernel, out user))
                    {
                        if (_prevCpuKernel > 0 || _prevCpuUser > 0)
                        {
                            long dIdle = idle - _prevCpuIdle;
                            long dTotal = (kernel - _prevCpuKernel) + (user - _prevCpuUser);
                            if (dTotal > 0)
                                snap.Cpu.UsagePercent = Math.Min(100, Math.Max(0, Math.Round((1.0 - (double)dIdle / dTotal) * 100.0, 1)));
                        }
                        _prevCpuIdle = idle;
                        _prevCpuKernel = kernel;
                        _prevCpuUser = user;
                    }
                }
            }
            catch { }

            try
            {
                if (_cpuPerfCounter != null && _staticCpu.BaseClockGHz > 0)
                {
                    double perf = _cpuPerfCounter.NextValue();
                    snap.Cpu.CurrentClockGHz = Math.Round(_staticCpu.BaseClockGHz * (perf / 100.0), 2);
                }
                else
                {
                    snap.Cpu.CurrentClockGHz = _staticCpu.BaseClockGHz;
                }
            }
            catch { snap.Cpu.CurrentClockGHz = _staticCpu.BaseClockGHz; }

            string cpuTempNote;
            snap.Cpu.TemperatureC = ReadCpuTemperature(out cpuTempNote);
            snap.Cpu.TemperatureNote = cpuTempNote;

            // 2. RAM
            snap.Ram.InstalledGB = _staticRam.InstalledGB;
            snap.Ram.TotalGB = _staticRam.TotalGB;
            snap.Ram.SpeedMHz = _staticRam.SpeedMHz;
            snap.Ram.MemoryType = _staticRam.MemoryType;
            snap.Ram.Slots = _staticRam.Slots;

            var memStatus = new NativeMethods.MEMORYSTATUSEX { dwLength = (uint)System.Runtime.InteropServices.Marshal.SizeOf(typeof(NativeMethods.MEMORYSTATUSEX)) };
            if (NativeMethods.GlobalMemoryStatusEx(ref memStatus))
            {
                double totalGb = memStatus.ullTotalPhys / (1024.0 * 1024.0 * 1024.0);
                double availGb = memStatus.ullAvailPhys / (1024.0 * 1024.0 * 1024.0);
                snap.Ram.TotalGB = Math.Round(totalGb, 1);
                snap.Ram.UsedGB = Math.Round(Math.Max(0, totalGb - availGb), 1);
                snap.Ram.UsagePercent = memStatus.dwMemoryLoad;
            }

            // 3. GPU (Dedicated + Integrated)
            snap.Gpu.Name = _staticGpu.Name;
            snap.Gpu.TotalVramGB = _staticGpu.TotalVramGB;
            snap.Gpu.DriverVersion = _staticGpu.DriverVersion;
            snap.Gpu.IsAvailable = _staticGpu.IsAvailable;
            PollGpuMetrics(snap.Gpu);

            snap.IntegratedGpu.Name = _staticIGpu.Name;
            snap.IntegratedGpu.TotalVramGB = _staticIGpu.TotalVramGB;
            snap.IntegratedGpu.DriverVersion = _staticIGpu.DriverVersion;
            snap.IntegratedGpu.IsIntegrated = true;
            snap.IntegratedGpu.IsAvailable = _staticIGpu.IsAvailable;
            if (_staticIGpu.IsAvailable)
            {
                PollIntegratedGpuMetrics(snap.IntegratedGpu, snap.Cpu.TemperatureC);
            }

            // 4. Storage
            snap.Storage.PrimaryModel = _staticStorage.PrimaryModel;
            snap.Storage.TotalGB = _staticStorage.TotalGB;
            snap.Storage.FreeGB = _staticStorage.FreeGB;
            snap.Storage.Disks = _staticStorage.Disks;

            try
            {
                if (_diskIdleCounter != null)
                    snap.Storage.UsagePercent = Math.Min(100, Math.Max(0, Math.Round(100.0 - _diskIdleCounter.NextValue(), 1)));
                if (_diskReadCounter != null)
                    snap.Storage.ReadMBps = Math.Round(_diskReadCounter.NextValue() / (1024.0 * 1024.0), 1);
                if (_diskWriteCounter != null)
                    snap.Storage.WriteMBps = Math.Round(_diskWriteCounter.NextValue() / (1024.0 * 1024.0), 1);
            }
            catch { }

            if (_tickCount == 1 || _tickCount % 5 == 0)
            {
                _cachedDiskTemp = ReadDiskTemperature();
            }
            snap.Storage.TemperatureC = _cachedDiskTemp;

            // 5. Network
            PollNetwork(snap.Network);

            // 6. Processes
            if (includeProcesses)
            {
                snap.Processes = PollProcesses(elapsedSec, groupProcesses);
            }

            // 7. FPS (via RTSS Shared Memory or built-in DXGI ETW)
            snap.Fps = ReadRtssFps() ?? _etwFps.GetCurrentFps();

            // 8. Alerts
            if (snap.Cpu.TemperatureC.HasValue && snap.Cpu.TemperatureC.Value >= CpuTempAlertThreshold)
                snap.Alerts.Add(string.Format("CPU Sıcaklığı Yüksek: {0:0}°C", snap.Cpu.TemperatureC.Value));
            if (snap.Gpu.TemperatureC.HasValue && snap.Gpu.TemperatureC.Value >= GpuTempAlertThreshold)
                snap.Alerts.Add(string.Format("GPU Sıcaklığı Yüksek: {0:0}°C", snap.Gpu.TemperatureC.Value));
            if (snap.Cpu.UsagePercent >= CpuUsageAlertThreshold)
                snap.Alerts.Add(string.Format("CPU Kullanımı Kritik: %{0:0}", snap.Cpu.UsagePercent));
            if (snap.Ram.UsagePercent >= RamUsageAlertThreshold)
                snap.Alerts.Add(string.Format("RAM Kullanımı Kritik: %{0:0}", snap.Ram.UsagePercent));

            return snap;
        }

        private double? ReadCpuTemperature(out string note)
        {
            note = "";
            if (_lhmComputer != null)
            {
                try
                {
                    double? pkgTemp = null;
                    double? anyTemp = null;
                    foreach (var hw in _lhmComputer.Hardware)
                    {
                        if (hw.HardwareType != LibreHardwareMonitor.Hardware.HardwareType.Cpu) continue;
                        hw.Update();
                        foreach (var s in hw.Sensors)
                        {
                            if (s.SensorType == LibreHardwareMonitor.Hardware.SensorType.Temperature && s.Value.HasValue)
                            {
                                double val = s.Value.Value;
                                if (val > 10 && val < 125)
                                {
                                    if (s.Name.IndexOf("Package", StringComparison.OrdinalIgnoreCase) >= 0 ||
                                        s.Name.IndexOf("Core Average", StringComparison.OrdinalIgnoreCase) >= 0)
                                        pkgTemp = val;
                                    else if (s.Name.IndexOf("Distance", StringComparison.OrdinalIgnoreCase) < 0 && !anyTemp.HasValue)
                                        anyTemp = val;
                                }
                            }
                        }
                    }
                    double? lhmCpu = pkgTemp ?? anyTemp;
                    if (lhmCpu.HasValue) { _cachedCpuTemp = Math.Round(lhmCpu.Value, 0); return _cachedCpuTemp; }
                }
                catch { }
            }
            if (_tickCount > 1 && _tickCount % 3 != 0)
            {
                if (!_cachedCpuTemp.HasValue && _lhmNamespace == null && !_hasAcpiTemp)
                    try
            {
                using (var searcher = new ManagementObjectSearcher(@"root\cimv2", "SELECT HighPrecisionTemperature, Temperature FROM Win32_PerfFormattedData_Counters_ThermalZoneInformation"))
                {
                    double maxTz = -1;
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        double celsius = -1;
                        if (obj["HighPrecisionTemperature"] != null)
                        {
                            double hp = Convert.ToDouble(obj["HighPrecisionTemperature"]);
                            if (hp > 2000) celsius = (hp / 10.0) - 273.15;
                        }
                        if (celsius <= 0 && obj["Temperature"] != null)
                        {
                            double k = Convert.ToDouble(obj["Temperature"]);
                            if (k > 200) celsius = k - 273.15;
                        }
                        if (celsius > 10 && celsius < 125 && celsius > maxTz)
                            maxTz = celsius;
                    }
                    if (maxTz > 0) { _cachedCpuTemp = Math.Round(maxTz, 0); return _cachedCpuTemp; }
                }
            }
            catch { }

            note = "Sıcaklık sensörü için Yönetici izni veya LibreHardwareMonitor gerekebilir.";
                return _cachedCpuTemp;
            }

            if (_lhmNamespace != null)
            {
                double? lhmTemp = QueryHardwareMonitorWmiTemp("CPU");
                if (lhmTemp.HasValue) { _cachedCpuTemp = lhmTemp; return lhmTemp; }
            }

            if (_thermalCounters.Count > 0)
            {
                double maxTz = -1;
                foreach (var tc in _thermalCounters)
                {
                    try
                    {
                        double k = tc.NextValue();
                        double c = k > 200 ? k - 273.15 : k;
                        if (c > 15 && c < 125 && c > maxTz) maxTz = c;
                    }
                    catch { }
                }
                if (maxTz > 0) { _cachedCpuTemp = Math.Round(maxTz, 0); return _cachedCpuTemp; }
            }

            if (_hasAcpiTemp)
            {
                try
                {
                    using (var searcher = new ManagementObjectSearcher(@"root\WMI", "SELECT CurrentTemperature FROM MSAcpi_ThermalZoneTemperature"))
                    {
                        double maxTemp = -1;
                        foreach (ManagementObject obj in searcher.Get())
                        {
                            double raw = Convert.ToDouble(obj["CurrentTemperature"]);
                            double celsius = (raw / 10.0) - 273.15;
                            if (celsius > 10 && celsius < 125 && celsius > maxTemp)
                                maxTemp = celsius;
                        }
                        if (maxTemp > 0) { _cachedCpuTemp = Math.Round(maxTemp, 0); return _cachedCpuTemp; }
                    }
                }
                catch { _hasAcpiTemp = false; }
            }

            try
            {
                using (var searcher = new ManagementObjectSearcher(@"root\cimv2", "SELECT HighPrecisionTemperature, Temperature FROM Win32_PerfFormattedData_Counters_ThermalZoneInformation"))
                {
                    double maxTz = -1;
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        double celsius = -1;
                        if (obj["HighPrecisionTemperature"] != null)
                        {
                            double hp = Convert.ToDouble(obj["HighPrecisionTemperature"]);
                            if (hp > 2000) celsius = (hp / 10.0) - 273.15;
                        }
                        if (celsius <= 0 && obj["Temperature"] != null)
                        {
                            double k = Convert.ToDouble(obj["Temperature"]);
                            if (k > 200) celsius = k - 273.15;
                        }
                        if (celsius > 10 && celsius < 125 && celsius > maxTz)
                            maxTz = celsius;
                    }
                    if (maxTz > 0) { _cachedCpuTemp = Math.Round(maxTz, 0); return _cachedCpuTemp; }
                }
            }
            catch { }

            note = "Sıcaklık sensörü için Yönetici izni veya LibreHardwareMonitor gerekebilir.";
            return _cachedCpuTemp;
        }

        private double? QueryHardwareMonitorWmiTemp(string keyword)
        {
            if (_lhmNamespace == null) return null;
            try
            {
                using (var searcher = new ManagementObjectSearcher(_lhmNamespace, "SELECT Name, Value, SensorType FROM Sensor WHERE SensorType='Temperature'"))
                {
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        string name = (obj["Name"] ?? "").ToString();
                        if (name.IndexOf(keyword, StringComparison.OrdinalIgnoreCase) >= 0 ||
                            name.IndexOf("Package", StringComparison.OrdinalIgnoreCase) >= 0)
                        {
                            double val = Convert.ToDouble(obj["Value"]);
                            if (val > 5 && val < 125) return Math.Round(val, 0);
                        }
                    }
                }
            }
            catch { _lhmNamespace = null; }
            return null;
        }

        private void PollGpuMetrics(GpuInfo gpu)
        {
            if (_tickCount > 1 && _tickCount % 3 != 0)
            {
                gpu.UsagePercent = _cachedGpuUtil;
                gpu.TemperatureC = _cachedGpuTemp;
                gpu.UsedVramGB = _cachedGpuUsedVram;
                if (_cachedGpuTotalVram > 0) gpu.TotalVramGB = _cachedGpuTotalVram;
                gpu.ClockMHz = _cachedGpuClock;
                gpu.FanPercent = _cachedGpuFan;
                return;
            }

            if (!string.IsNullOrEmpty(_nvidiaSmiPath))
            {
                try
                {
                    var psi = new ProcessStartInfo
                    {
                        FileName = _nvidiaSmiPath,
                        Arguments = "--query-gpu=utilization.gpu,temperature.gpu,memory.used,memory.total,clocks.current.graphics,fan.speed --format=csv,noheader,nounits",
                        UseShellExecute = false,
                        RedirectStandardOutput = true,
                        CreateNoWindow = true
                    };
                    using (var proc = Process.Start(psi))
                    {
                        if (proc != null)
                        {
                            string output = proc.StandardOutput.ReadLine();
                            proc.WaitForExit(350);
                            if (!string.IsNullOrEmpty(output))
                            {
                                var parts = output.Split(',');
                                if (parts.Length >= 4)
                                {
                                    double util, temp, usedMb, totalMb;
                                    if (double.TryParse(parts[0].Trim(), System.Globalization.NumberStyles.Any, System.Globalization.CultureInfo.InvariantCulture, out util))
                                        _cachedGpuUtil = gpu.UsagePercent = Math.Round(util, 1);
                                    if (double.TryParse(parts[1].Trim(), System.Globalization.NumberStyles.Any, System.Globalization.CultureInfo.InvariantCulture, out temp))
                                        _cachedGpuTemp = gpu.TemperatureC = Math.Round(temp, 0);
                                    if (double.TryParse(parts[2].Trim(), System.Globalization.NumberStyles.Any, System.Globalization.CultureInfo.InvariantCulture, out usedMb))
                                        _cachedGpuUsedVram = gpu.UsedVramGB = Math.Round(usedMb / 1024.0, 2);
                                    if (double.TryParse(parts[3].Trim(), System.Globalization.NumberStyles.Any, System.Globalization.CultureInfo.InvariantCulture, out totalMb) && totalMb > 0)
                                        _cachedGpuTotalVram = gpu.TotalVramGB = Math.Round(totalMb / 1024.0, 1);

                                    if (parts.Length >= 5)
                                    {
                                        double clk;
                                        if (double.TryParse(parts[4].Trim(), System.Globalization.NumberStyles.Any, System.Globalization.CultureInfo.InvariantCulture, out clk))
                                            _cachedGpuClock = gpu.ClockMHz = clk;
                                    }
                                    if (parts.Length >= 6)
                                    {
                                        double fan;
                                        if (double.TryParse(parts[5].Trim(), System.Globalization.NumberStyles.Any, System.Globalization.CultureInfo.InvariantCulture, out fan))
                                            _cachedGpuFan = gpu.FanPercent = fan;
                                    }
                                    return;
                                }
                            }
                        }
                    }
                }
                catch { }
            }

            _cachedGpuTemp = gpu.TemperatureC = QueryHardwareMonitorWmiTemp("GPU");
            if (_hasGpuEngineWmi)
            {
                try
                {
                    double max3d = 0;
                    using (var searcher = new ManagementObjectSearcher("SELECT Name, UtilizationPercentage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine"))
                    {
                        foreach (ManagementObject obj in searcher.Get())
                        {
                            string engName = (obj["Name"] ?? "").ToString();
                            if (engName.IndexOf("engtype_3D", StringComparison.OrdinalIgnoreCase) >= 0)
                                max3d += Convert.ToDouble(obj["UtilizationPercentage"] ?? 0);
                        }
                    }
                    _cachedGpuUtil = gpu.UsagePercent = Math.Min(100, Math.Round(max3d, 1));
                }
                catch { _hasGpuEngineWmi = false; }
            }
            if (_hasGpuAdapterMemWmi)
            {
                try
                {
                    using (var searcher = new ManagementObjectSearcher("SELECT DedicatedUsage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory"))
                    {
                        ulong maxUsed = 0;
                        foreach (ManagementObject obj in searcher.Get())
                        {
                            ulong val = Convert.ToUInt64(obj["DedicatedUsage"] ?? 0);
                            if (val > maxUsed) maxUsed = val;
                        }
                        if (maxUsed > 0)
                            _cachedGpuUsedVram = gpu.UsedVramGB = Math.Round(maxUsed / (1024.0 * 1024.0 * 1024.0), 2);
                    }
                }
                catch { _hasGpuAdapterMemWmi = false; }
            }

            if (!gpu.TemperatureC.HasValue)
                gpu.SensorNote = "NVIDIA dışındaki GPU sıcaklığı için LibreHardwareMonitor gerekebilir.";
        }

        private void PollIntegratedGpuMetrics(GpuInfo igpu, double? cpuTemp)
        {
            igpu.TemperatureC = QueryHardwareMonitorWmiTemp("Intel") ?? QueryHardwareMonitorWmiTemp("Graphics") ?? cpuTemp;
            if (_tickCount > 1 && _tickCount % 4 != 0)
            {
                igpu.UsagePercent = _cachedIGpuUtil;
                igpu.UsedVramGB = _cachedIGpuVram;
                return;
            }

            if (_hasGpuEngineWmi)
            {
                try
                {
                    double max3d = 0;
                    using (var searcher = new ManagementObjectSearcher("SELECT Name, UtilizationPercentage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine"))
                    {
                        foreach (ManagementObject obj in searcher.Get())
                        {
                            string engName = (obj["Name"] ?? "").ToString();
                            if (engName.IndexOf("engtype_3D", StringComparison.OrdinalIgnoreCase) >= 0)
                            {
                                max3d += Convert.ToDouble(obj["UtilizationPercentage"] ?? 0);
                            }
                        }
                    }
                    _cachedIGpuUtil = igpu.UsagePercent = Math.Min(100, Math.Round(max3d, 1));
                }
                catch { _hasGpuEngineWmi = false; }
            }

            if (_hasGpuAdapterMemWmi)
            {
                try
                {
                    using (var searcher = new ManagementObjectSearcher("SELECT SharedUsage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory"))
                    {
                        ulong shared = 0;
                        foreach (ManagementObject obj in searcher.Get())
                        {
                            ulong s = Convert.ToUInt64(obj["SharedUsage"] ?? 0);
                            if (s > shared) shared = s;
                        }
                        if (shared > 0)
                            _cachedIGpuVram = igpu.UsedVramGB = Math.Round(shared / (1024.0 * 1024.0 * 1024.0), 2);
                    }
                }
                catch { _hasGpuAdapterMemWmi = false; }
            }
        }

        private double? ReadDiskTemperature()
        {
            double? directNvme = ReadNvmeDeviceProtocolTemp();
            if (directNvme.HasValue) return directNvme;

            if (_lhmComputer != null)
            {
                try
                {
                    foreach (var hw in _lhmComputer.Hardware)
                    {
                        if (hw.HardwareType != LibreHardwareMonitor.Hardware.HardwareType.Storage) continue;
                        hw.Update();
                        foreach (var s in hw.Sensors)
                        {
                            if (s.SensorType == LibreHardwareMonitor.Hardware.SensorType.Temperature && s.Value.HasValue)
                            {
                                double val = s.Value.Value;
                                if (val > 5 && val < 110) return Math.Round(val, 0);
                            }
                        }
                    }
                }
                catch { }
            }
            if (_hasStorageTemp)
            {
                try
                {
                    using (var searcher = new ManagementObjectSearcher(@"root\Microsoft\Windows\Storage", "SELECT Temperature FROM MSFT_StorageReliabilityCounter"))
                    {
                        foreach (ManagementObject obj in searcher.Get())
                        {
                            if (obj["Temperature"] != null)
                            {
                                double t = Convert.ToDouble(obj["Temperature"]);
                                if (t > 0 && t < 110) return Math.Round(t, 0);
                            }
                        }
                    }
                    using (var diskSearcher = new ManagementObjectSearcher(@"root\Microsoft\Windows\Storage", "SELECT ObjectId FROM MSFT_PhysicalDisk"))
                    {
                        foreach (ManagementObject disk in diskSearcher.Get())
                        {
                            foreach (ManagementObject rel in disk.GetRelated("MSFT_StorageReliabilityCounter"))
                            {
                                if (rel["Temperature"] != null)
                                {
                                    double t = Convert.ToDouble(rel["Temperature"]);
                                    if (t > 0 && t < 110) return Math.Round(t, 0);
                                }
                            }
                        }
                    }
                }
                catch { _hasStorageTemp = false; }
            }

            try
            {
                using (var searcher = new ManagementObjectSearcher(@"root\WMI", "SELECT VendorSpecific FROM MSStorageDriver_ATAPISmartData"))
                {
                    foreach (ManagementObject obj in searcher.Get())
                    {
                        byte[] vs = obj["VendorSpecific"] as byte[];
                        if (vs != null && vs.Length >= 362)
                        {
                            for (int i = 2; i + 12 <= vs.Length; i += 12)
                            {
                                byte attrId = vs[i];
                                if (attrId == 194 || attrId == 190)
                                {
                                    byte temp = vs[i + 5];
                                    if (temp > 10 && temp < 105) return (double)temp;
                                }
                            }
                        }
                    }
                }
            }
            catch { }

            return QueryHardwareMonitorWmiTemp("Drive") ?? QueryHardwareMonitorWmiTemp("SSD") ?? QueryHardwareMonitorWmiTemp("NVMe");
        }

        private void PollNetwork(NetworkInfo net)
        {
            net.AdapterName = _activeNicName;
            try
            {
                long recv = 0, sent = 0;
                foreach (var nic in NetworkInterface.GetAllNetworkInterfaces())
                {
                    if (nic.OperationalStatus == OperationalStatus.Up && nic.NetworkInterfaceType != NetworkInterfaceType.Loopback)
                    {
                        var stats = nic.GetIPv4Statistics();
                        recv += stats.BytesReceived;
                        sent += stats.BytesSent;
                    }
                }
                var now = DateTime.UtcNow;
                double sec = Math.Max(0.2, (now - _lastNetTime).TotalSeconds);
                net.DownloadMBps = Math.Round(Math.Max(0, recv - _prevNetRecv) / (sec * 1024.0 * 1024.0), 2);
                net.UploadMBps = Math.Round(Math.Max(0, sent - _prevNetSent) / (sec * 1024.0 * 1024.0), 2);
                _prevNetRecv = recv;
                _prevNetSent = sent;
                _lastNetTime = now;
            }
            catch { }
        }

        private List<ProcessUsageItem> PollProcesses(double elapsedSec, bool groupByName)
        {
            var items = new List<ProcessUsageItem>();
            var activePids = new HashSet<int>();
            int procCount = Math.Max(1, Environment.ProcessorCount);

            Process[] procs;
            try { procs = Process.GetProcesses(); }
            catch { return items; }

            foreach (var p in procs)
            {
                try
                {
                    int pid = p.Id;
                    if (pid == 0) continue; // Idle
                    activePids.Add(pid);

                    double cpuPct = 0;
                    try
                    {
                        TimeSpan totalCpu = p.TotalProcessorTime;
                        TimeSpan prevCpu;
                        if (_prevProcCpu.TryGetValue(pid, out prevCpu))
                        {
                            double deltaMs = (totalCpu - prevCpu).TotalMilliseconds;
                            cpuPct = Math.Min(100.0, Math.Max(0.0, (deltaMs / (elapsedSec * 1000.0 * procCount)) * 100.0));
                        }
                        _prevProcCpu[pid] = totalCpu;
                    }
                    catch { }

                    double diskMBps = 0;
                    try
                    {
                        NativeMethods.IO_COUNTERS io;
                        if (NativeMethods.GetProcessIoCounters(p.Handle, out io))
                        {
                            ulong totalIo = io.ReadTransferCount + io.WriteTransferCount;
                            ulong prevIo;
                            if (_prevProcIo.TryGetValue(pid, out prevIo) && totalIo >= prevIo)
                            {
                                diskMBps = (totalIo - prevIo) / (elapsedSec * 1024.0 * 1024.0);
                            }
                            _prevProcIo[pid] = totalIo;
                        }
                    }
                    catch { }

                    double ramMb = Math.Min(p.WorkingSet64, p.PrivateMemorySize64) / (1024.0 * 1024.0);
                    string exePath = null;
                    try
                    {
                        var sb = new System.Text.StringBuilder(512);
                        int sz = sb.Capacity;
                        if (NativeMethods.QueryFullProcessImageName(p.Handle, 0, sb, ref sz))
                            exePath = sb.ToString();
                    }
                    catch { }

                    items.Add(new ProcessUsageItem
                    {
                        Pid = pid,
                        Name = p.ProcessName,
                        ExecutablePath = exePath,
                        CpuPercent = Math.Round(cpuPct, 1),
                        RamMB = Math.Round(ramMb, 1),
                        DiskMBps = Math.Round(diskMBps, 2),
                        GpuPercent = null
                    });
                }
                catch { }
                finally
                {
                    p.Dispose();
                }
            }

            // Cleanup exited PIDs
            var staleCpu = _prevProcCpu.Keys.Where(k => !activePids.Contains(k)).ToList();
            foreach (var k in staleCpu) _prevProcCpu.Remove(k);
            var staleIo = _prevProcIo.Keys.Where(k => !activePids.Contains(k)).ToList();
            foreach (var k in staleIo) _prevProcIo.Remove(k);

            if (groupByName)
            {
                return items
                    .GroupBy(x => x.Name, StringComparer.OrdinalIgnoreCase)
                    .Select(g => new ProcessUsageItem
                    {
                        Pid = g.First().Pid,
                        Name = g.Key,
                        ExecutablePath = g.Select(x => x.ExecutablePath).FirstOrDefault(ep => !string.IsNullOrEmpty(ep)),
                        InstanceCount = g.Count(),
                        CpuPercent = Math.Round(g.Sum(x => x.CpuPercent), 1),
                        RamMB = Math.Round(g.Sum(x => x.RamMB), 1),
                        DiskMBps = Math.Round(g.Sum(x => x.DiskMBps), 2),
                        GpuPercent = null
                    })
                    .ToList();
            }

            return items;
        }

        private int? ReadRtssFps()
        {
            try
            {
                using (var mmf = System.IO.MemoryMappedFiles.MemoryMappedFile.OpenExisting("RTSSSharedMemoryV2", System.IO.MemoryMappedFiles.MemoryMappedFileRights.Read))
                using (var acc = mmf.CreateViewAccessor(0, 0, System.IO.MemoryMappedFiles.MemoryMappedFileAccess.Read))
                {
                    uint sig = acc.ReadUInt32(0);
                    if (sig != 0x52545353) return null; // 'RTSS'
                    uint arrEntrySize = acc.ReadUInt32(8);
                    uint arrOffset = acc.ReadUInt32(12);
                    uint arrSize = acc.ReadUInt32(16);

                    int bestFps = 0;
                    for (uint i = 0; i < arrSize && i < 64; i++)
                    {
                        long baseOff = arrOffset + (i * arrEntrySize);
                        uint pid = acc.ReadUInt32(baseOff);
                        if (pid == 0) continue;
                        // dwFlags (264), dwTime0 (268), dwTime1 (272), dwFrames (276) in RTSS v2 entry
                        uint t0 = acc.ReadUInt32(baseOff + 268);
                        uint t1 = acc.ReadUInt32(baseOff + 272);
                        uint frames = acc.ReadUInt32(baseOff + 276);
                        if (t1 > t0 && frames > 0)
                        {
                            int fps = (int)Math.Round((frames * 1000.0) / (t1 - t0));
                            if (fps > bestFps && fps < 2000) bestFps = fps;
                        }
                    }
                    if (bestFps > 0) return bestFps;
                }
            }
            catch { }
            return null;
        }

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        private static extern SafeFileHandle CreateFile(string lpFileName, uint dwDesiredAccess, uint dwShareMode,
            IntPtr lpSecurityAttributes, uint dwCreationDisposition, uint dwFlagsAndAttributes, IntPtr hTemplateFile);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool DeviceIoControl(SafeFileHandle hDevice, uint dwIoControlCode,
            byte[] lpInBuffer, int nInBufferSize, byte[] lpOutBuffer, int nOutBufferSize,
            out int lpBytesReturned, IntPtr lpOverlapped);

        private static double? ReadNvmeDeviceProtocolTemp()
        {
            const uint GENERIC_READ = 0x80000000;
            const uint GENERIC_WRITE = 0x40000000;
            const uint FILE_SHARE_READ = 1;
            const uint FILE_SHARE_WRITE = 2;
            const uint OPEN_EXISTING = 3;
            const uint IOCTL_STORAGE_QUERY_PROPERTY = 0x002D1400;

            for (int idx = 0; idx < 4; idx++)
            {
                string path = @"\\.\PhysicalDrive" + idx;
                try
                {
                    using (var h = CreateFile(path, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero))
                    {
                        if (h.IsInvalid) continue;
                        byte[] buf = new byte[48 + 512];
                        BitConverter.GetBytes(50u).CopyTo(buf, 0);          // StorageDeviceProtocolSpecificProperty
                        BitConverter.GetBytes(0u).CopyTo(buf, 4);           // PropertyStandardQuery
                        BitConverter.GetBytes(3u).CopyTo(buf, 8);           // ProtocolType = Nvme
                        BitConverter.GetBytes(2u).CopyTo(buf, 12);          // DataType = NVMeDataTypeLogPage
                        BitConverter.GetBytes(2u).CopyTo(buf, 16);          // LogPage 0x02 (SMART/Health)
                        BitConverter.GetBytes(0xFFFFFFFFu).CopyTo(buf, 20); // NSID = all
                        BitConverter.GetBytes(40u).CopyTo(buf, 24);         // ProtocolDataOffset
                        BitConverter.GetBytes(512u).CopyTo(buf, 28);        // ProtocolDataLength

                        int ret;
                        if (DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, buf, buf.Length, buf, buf.Length, out ret, IntPtr.Zero) && ret >= 52)
                        {
                            int kelvin = buf[49] | (buf[50] << 8);
                            double tempC = kelvin - 273.15;
                            if (tempC > 5 && tempC < 110) return Math.Round(tempC, 0);
                        }
                    }
                }
                catch { }
            }
            return null;
        }

        public void Dispose()
        {
            try { if (_lhmComputer != null) _lhmComputer.Close(); } catch { }
            try { _etwFps.Dispose(); } catch { }
            if (_cpuTotalCounter != null) _cpuTotalCounter.Dispose();
            if (_cpuPerfCounter != null) _cpuPerfCounter.Dispose();
            if (_diskIdleCounter != null) _diskIdleCounter.Dispose();
            if (_diskReadCounter != null) _diskReadCounter.Dispose();
            if (_diskWriteCounter != null) _diskWriteCounter.Dispose();
            foreach (var c in _thermalCounters) c.Dispose();
        }
    }
}
