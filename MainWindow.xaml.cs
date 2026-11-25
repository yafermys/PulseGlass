using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using PulseGlassWidget.Models;
using PulseGlassWidget.Services;

namespace PulseGlassWidget
{
    public class CoreBarVm
    {
        public string Label { get; set; }
        public double Value { get; set; }
        public string Text { get; set; }
        public Brush BarBrush { get; set; }
    }

    public partial class MainWindow : Window
    {
        private const int HOTKEY_GHOST_ID = 9001;
        private const int HOTKEY_SUMMARY_ID = 9002;

        private readonly HardwareMonitorService _monitor = new HardwareMonitorService();
        private readonly SystemControlService _sysControl = new SystemControlService();
        private readonly DispatcherTimer _timer = new DispatcherTimer();

        private bool _isDetailed;
        private bool _isGameMode;
        private bool _isWindowMode;
        private bool _isDesktopPinned;
        private bool _isGhostMode;
        private bool _isPolling;
        private bool _uiReady;
        private bool _isMiddleDragging;
        private Point _dragStartScreen;
        private Point _windowStartPos;

        private double _widgetOpacity = 0.78;
        private double _windowOpacity = 0.94;
        private Color _primaryColor = Color.FromRgb(255, 77, 109);
        private Color _secondaryColor = Color.FromRgb(255, 176, 133);
        private Color _bgBaseColor = Color.FromRgb(18, 15, 24);
        private HardwareSnapshot _lastSnapshot;
        private System.Windows.Forms.NotifyIcon _trayIcon;
        private readonly UpdateService _updateService = new UpdateService();
        private readonly DispatcherTimer _updateTimer = new DispatcherTimer();
        private UpdateCheckResult _lastUpdateResult;
        private bool _isCheckingUpdates;
        private bool _isEnglish;
        private bool _hasPendingUpdates;
        private bool _skipSaveOnClose;

        public MainWindow()
        {
            InitializeComponent();
            LoadSavedSettings();
            _uiReady = true;
            ApplyActiveOpacity();
            InitTrayIcon();
            Loaded += MainWindow_Loaded;
            Closed += MainWindow_Closed;
        }

        private void LoadSavedSettings()
        {
            var s = SettingsService.Load();
            _widgetOpacity = s.WidgetOpacity;
            _windowOpacity = s.WindowOpacity;
            SldWidgetOpacity.Value = _widgetOpacity;
            SldWindowOpacity.Value = _windowOpacity;
            LblWidgetOpacityVal.Text = string.Format("{0:0}%", _widgetOpacity * 100);
            LblWindowOpacityVal.Text = string.Format("{0:0}%", _windowOpacity * 100);

            SldScale.Value = s.Scale;
            WidgetScale.ScaleX = s.Scale;
            WidgetScale.ScaleY = s.Scale;
            LblScaleVal.Text = string.Format("{0:0.00}x", s.Scale);

            TxtPrimaryHex.Text = s.PrimaryHex;
            TxtSecondaryHex.Text = s.SecondaryHex;
            TxtBgHex.Text = s.BgHex;
            try
            {
                ApplyCustomTheme(
                    (Color)ColorConverter.ConvertFromString(s.PrimaryHex),
                    (Color)ColorConverter.ConvertFromString(s.SecondaryHex),
                    (Color)ColorConverter.ConvertFromString(s.BgHex));
            }
            catch { }

            SldCpuTempThresh.Value = s.CpuTempThresh;
            SldGpuTempThresh.Value = s.GpuTempThresh;
            SldCpuUseThresh.Value = s.CpuUseThresh;
            SldRamUseThresh.Value = s.RamUseThresh;
            _monitor.CpuTempAlertThreshold = s.CpuTempThresh;
            _monitor.GpuTempAlertThreshold = s.GpuTempThresh;
            _monitor.CpuUsageAlertThreshold = s.CpuUseThresh;
            _monitor.RamUsageAlertThreshold = s.RamUseThresh;

            ChkSumCpuPct.IsChecked = s.SumCpuPct;
            ChkSumCpuTemp.IsChecked = s.SumCpuTemp;
            ChkSumGpuPct.IsChecked = s.SumGpuPct;
            ChkSumGpuTemp.IsChecked = s.SumGpuTemp;
            ChkSumIGpuPct.IsChecked = s.SumIGpuPct;
            ChkSumRamPct.IsChecked = s.SumRamPct;
            ChkSumSsdTemp.IsChecked = s.SumSsdTemp;
            ChkSumFps.IsChecked = s.SumFps;

            _isEnglish = string.Equals(s.Language, "en", StringComparison.OrdinalIgnoreCase);
            CmbLanguage.SelectedIndex = _isEnglish ? 1 : 0;
            ApplyLanguage(_isEnglish);

            ChkAutoStart.IsChecked = SettingsService.IsAutoStartEnabled();
        }

        private void InitTrayIcon()
        {
            try
            {
                string exePath = Process.GetCurrentProcess().MainModule.FileName;
                _trayIcon = new System.Windows.Forms.NotifyIcon
                {
                    Icon = System.Drawing.Icon.ExtractAssociatedIcon(exePath),
                    Text = "PulseGlass Hardware Monitor",
                    Visible = true
                };
                var menu = new System.Windows.Forms.ContextMenuStrip();
                menu.Items.Add("Göster / Öne Getir", null, (s, e) => { Show(); WindowState = WindowState.Normal; Activate(); });
                menu.Items.Add("Özet Modu (Mini Şerit)", null, (s, e) => { if (!_isGameMode) BtnGameMode_Click(null, null); });
                menu.Items.Add("Kompakt Widget", null, (s, e) => { if (_isGameMode) BtnGameMode_Click(null, null); if (_isWindowMode) BtnWindowMode_Click(null, null); ToggleMode(false); });
                menu.Items.Add("Tam Pencere Modu", null, (s, e) => { if (!_isWindowMode) BtnWindowMode_Click(null, null); });
                menu.Items.Add("Geçirgenlik (Shift+Enter)", null, (s, e) => ToggleGhostMode());
                menu.Items.Add(new System.Windows.Forms.ToolStripSeparator());
                menu.Items.Add("Çıkış", null, (s, e) => Close());
                _trayIcon.ContextMenuStrip = menu;
                _trayIcon.DoubleClick += (s, e) => { Show(); WindowState = WindowState.Normal; Activate(); };
            }
            catch { }
        }

        private async void MainWindow_Loaded(object sender, RoutedEventArgs e)
        {
            var area = SystemParameters.WorkArea;
            var saved = SettingsService.Load();
            if (saved.Left.HasValue && saved.Top.HasValue &&
                saved.Left.Value >= SystemParameters.VirtualScreenLeft - 50 &&
                saved.Left.Value <= SystemParameters.VirtualScreenLeft + SystemParameters.VirtualScreenWidth - 100 &&
                saved.Top.Value >= SystemParameters.VirtualScreenTop &&
                saved.Top.Value <= SystemParameters.VirtualScreenTop + SystemParameters.VirtualScreenHeight - 80)
            {
                Left = saved.Left.Value;
                Top = saved.Top.Value;
            }
            else
            {
                double w = ActualWidth > 0 ? ActualWidth : Width;
                Left = area.Right - w - 16;
                Top = area.Top + 16;
            }

            if (!_sysControl.IsAdmin && !Environment.GetCommandLineArgs().Any(a => string.Equals(a, "--no-elevate", StringComparison.OrdinalIgnoreCase)))
            {
                try
                {
                    _skipSaveOnClose = true;
                    App.ReleaseSingleInstanceMutex();
                    var exe = Process.GetCurrentProcess().MainModule.FileName;
                    var psi = new ProcessStartInfo(exe, "--no-elevate") { UseShellExecute = true, Verb = "runas" };
                    Process.Start(psi);
                    Close();
                    return;
                }
                catch
                {
                    _skipSaveOnClose = false;
                }
            }

            var hwnd = new WindowInteropHelper(this).Handle;
            NativeMethods.TryEnableWin11RoundedCorners(hwnd);
            NativeMethods.RegisterHotKey(hwnd, HOTKEY_GHOST_ID, NativeMethods.MOD_SHIFT, NativeMethods.VK_RETURN);
            NativeMethods.RegisterHotKey(hwnd, HOTKEY_SUMMARY_ID, NativeMethods.MOD_CONTROL, NativeMethods.VK_RETURN);
            var source = HwndSource.FromHwnd(hwnd);
            if (source != null) source.AddHook(WndProc);

            await Task.Run(() => _monitor.InitializeStaticInfo());

            _timer.Interval = TimeSpan.FromMilliseconds(1000);
            _timer.Tick += Timer_Tick;
            _timer.Start();

            _updateTimer.Interval = TimeSpan.FromHours(6);
            _updateTimer.Tick += async (s2, e2) => await RunUpdateCheckAsync(false);
            _updateTimer.Start();

            await PollOnceAsync();
            await Task.Delay(8000);
            await RunUpdateCheckAsync(false);
        }

        private IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
        {
            if (msg == NativeMethods.WM_HOTKEY)
            {
                int id = wParam.ToInt32();
                if (id == HOTKEY_GHOST_ID)
                {
                    ToggleGhostMode();
                    handled = true;
                }
                else if (id == HOTKEY_SUMMARY_ID)
                {
                    BtnGameMode_Click(null, null);
                    handled = true;
                }
            }
            return IntPtr.Zero;
        }

        private void MainWindow_Closed(object sender, EventArgs e)
        {
            if (!_skipSaveOnClose)
            {
                SaveCurrentSettings();
            }
            if (_trayIcon != null)
            {
                _trayIcon.Visible = false;
                _trayIcon.Dispose();
            }
            try
            {
                var hwnd = new WindowInteropHelper(this).Handle;
                NativeMethods.UnregisterHotKey(hwnd, HOTKEY_GHOST_ID);
                NativeMethods.UnregisterHotKey(hwnd, HOTKEY_SUMMARY_ID);
            }
            catch { }
            _timer.Stop();
            _updateTimer.Stop();
            _monitor.Dispose();
        }

        private void SaveCurrentSettings()
        {
            SettingsService.Save(new AppSettingsData
            {
                WidgetOpacity = _widgetOpacity,
                WindowOpacity = _windowOpacity,
                Scale = SldScale.Value,
                RefreshMs = (int)_timer.Interval.TotalMilliseconds,
                ActiveModeId = _sysControl.ActiveModeId,
                PrimaryHex = TxtPrimaryHex.Text,
                SecondaryHex = TxtSecondaryHex.Text,
                BgHex = TxtBgHex.Text,
                CpuTempThresh = SldCpuTempThresh.Value,
                GpuTempThresh = SldGpuTempThresh.Value,
                CpuUseThresh = SldCpuUseThresh.Value,
                RamUseThresh = SldRamUseThresh.Value,
                SumCpuPct = ChkSumCpuPct.IsChecked == true,
                SumCpuTemp = ChkSumCpuTemp.IsChecked == true,
                SumGpuPct = ChkSumGpuPct.IsChecked == true,
                SumGpuTemp = ChkSumGpuTemp.IsChecked == true,
                SumIGpuPct = ChkSumIGpuPct.IsChecked == true,
                SumRamPct = ChkSumRamPct.IsChecked == true,
                SumSsdTemp = ChkSumSsdTemp.IsChecked == true,
                SumFps = ChkSumFps.IsChecked == true,
                Language = _isEnglish ? "en" : "tr",
                Left = Left,
                Top = Top
            });
        }

        private async void Timer_Tick(object sender, EventArgs e)
        {
            await PollOnceAsync();
        }

        private async Task PollOnceAsync()
        {
            if (_isPolling) return;
            _isPolling = true;
            try
            {
                bool needProcs = _isDetailed && ViewProcesses.Visibility == Visibility.Visible;
                bool groupProcs = ChkGroupProcs.IsChecked == true;

                var snap = await Task.Run(() => _monitor.PollSnapshot(needProcs, groupProcs));
                _lastSnapshot = snap;
                UpdateUi(snap);
            }
            catch { }
            finally
            {
                _isPolling = false;
            }
        }

        private void UpdateUi(HardwareSnapshot snap)
        {
            if (_isGameMode)
            {
                var parts = new List<string>();
                if (ChkSumCpuPct.IsChecked == true || ChkSumCpuTemp.IsChecked == true)
                {
                    string cpuStr = "CPU";
                    if (ChkSumCpuPct.IsChecked == true) cpuStr += string.Format(" {0:0}%", snap.Cpu.UsagePercent);
                    if (ChkSumCpuTemp.IsChecked == true) cpuStr += string.Format(" ({0})", snap.Cpu.TemperatureText);
                    parts.Add(cpuStr);
                }
                if (ChkSumGpuPct.IsChecked == true || ChkSumGpuTemp.IsChecked == true)
                {
                    string gpuStr = "GPU";
                    if (ChkSumGpuPct.IsChecked == true) gpuStr += string.Format(" {0:0}%", snap.Gpu.UsagePercent);
                    if (ChkSumGpuTemp.IsChecked == true) gpuStr += string.Format(" ({0})", snap.Gpu.TemperatureText);
                    parts.Add(gpuStr);
                }
                if (ChkSumIGpuPct.IsChecked == true)
                    parts.Add(string.Format("iGPU {0:0}%", snap.IntegratedGpu.UsagePercent));
                if (ChkSumRamPct.IsChecked == true)
                    parts.Add(string.Format("RAM {0:0}%", snap.Ram.UsagePercent));
                if (ChkSumSsdTemp.IsChecked == true)
                    parts.Add(string.Format("SSD {0}", snap.Storage.TemperatureText));
                if (ChkSumFps.IsChecked == true)
                    parts.Add(snap.Fps.HasValue ? string.Format("FPS: {0}", snap.Fps.Value) : "FPS: --");

                MiniOverlayText.Text = parts.Count > 0 ? string.Join(" • ", parts) : (_isEnglish ? "Summary empty (select in Settings)" : "Özet kapalı (Ayarlar'dan seçin)");
            }
            else if (!_isDetailed)
            {
                CompactCpuGauge.Value = snap.Cpu.UsagePercent;
                CompactCpuGauge.SubText = snap.Cpu.TemperatureText;
                CompactCpuClock.Text = string.Format("{0:0.00} GHz", snap.Cpu.CurrentClockGHz);

                CompactGpuGauge.Value = snap.Gpu.UsagePercent;
                CompactGpuGauge.SubText = snap.Gpu.TemperatureText;
                CompactGpuVram.Text = snap.Gpu.TotalVramGB > 0
                    ? string.Format("{0:0.0}/{1:0.0} GB", snap.Gpu.UsedVramGB, snap.Gpu.TotalVramGB)
                    : "VRAM N/A";

                CompactRamGauge.Value = snap.Ram.UsagePercent;
                CompactRamGauge.SubText = snap.Ram.UsedGB >= 1.0
                    ? string.Format("{0:0} GB", Math.Round(snap.Ram.UsedGB))
                    : string.Format("{0:0} MB", snap.Ram.UsedGB * 1024);
                double displayTotalRam = snap.Ram.InstalledGB > 0 ? snap.Ram.InstalledGB : snap.Ram.TotalGB;
                CompactRamDetail.Text = string.Format(System.Globalization.CultureInfo.InvariantCulture, "{0:0.0} / {1:0} GB", snap.Ram.UsedGB, displayTotalRam);

                CompactIGpuGauge.Value = snap.IntegratedGpu.IsAvailable ? snap.IntegratedGpu.UsagePercent : 0;
                CompactIGpuGauge.SubText = snap.IntegratedGpu.IsAvailable ? snap.IntegratedGpu.TemperatureText : "N/A";
                CompactIGpuSub.Text = snap.IntegratedGpu.IsAvailable
                    ? string.Format(System.Globalization.CultureInfo.InvariantCulture, "{0:0.0} GB", snap.IntegratedGpu.UsedVramGB)
                    : "Yok";

                CompactDiskText.Text = string.Format("SSD: %{0:0} Aktif  •  {1}  •  {2}",
                    snap.Storage.UsagePercent, snap.Storage.IoText, snap.Storage.TemperatureText);

                CompactNetText.Text = string.Format("↓ {0:0.00} MB/s   ↑ {1:0.00} MB/s", snap.Network.DownloadMBps, snap.Network.UploadMBps);
                CompactFpsText.Text = snap.Fps.HasValue ? string.Format("FPS: {0}", snap.Fps.Value) : "FPS: --";
            }

            // Alerts (use Hidden instead of Collapsed so panel never shifts)
            if (snap.Alerts.Count > 0)
            {
                AlertBadge.Visibility = Visibility.Visible;
                AlertBadgeText.Text = snap.Alerts[0];
                var b = new SolidColorBrush(Color.FromArgb(220, _primaryColor.R, _primaryColor.G, _primaryColor.B)); b.Freeze();
                var d = new SolidColorBrush(Color.FromRgb(239, 68, 68)); d.Freeze();
                GlassFrame.BorderBrush = b;
                StatusDot.Fill = d;
            }
            else if (_isGhostMode)
            {
                AlertBadge.Visibility = Visibility.Visible;
                AlertBadgeText.Text = "Geçirgen (Shift+Enter)";
                var b = new SolidColorBrush(Color.FromArgb(140, _secondaryColor.R, _secondaryColor.G, _secondaryColor.B)); b.Freeze();
                var d = new SolidColorBrush(_secondaryColor); d.Freeze();
                GlassFrame.BorderBrush = b;
                StatusDot.Fill = d;
            }
            else
            {
                AlertBadge.Visibility = Visibility.Hidden;
                var b = new SolidColorBrush(Color.FromArgb(68, _secondaryColor.R, _secondaryColor.G, _secondaryColor.B)); b.Freeze();
                var d = new SolidColorBrush(_primaryColor); d.Freeze();
                GlassFrame.BorderBrush = b;
                StatusDot.Fill = d;
            }

            if (!_isDetailed) return;

            if (ViewOverview.Visibility == Visibility.Visible)
            {
                DetCpuGauge.Value = snap.Cpu.UsagePercent;
                DetCpuGauge.SubText = snap.Cpu.TemperatureText;
                DetCpuSub.Text = string.Format("{0:0.00} GHz  ({1}C/{2}T)", snap.Cpu.CurrentClockGHz, snap.Cpu.PhysicalCores, snap.Cpu.LogicalProcessors);

                DetGpuGauge.Value = snap.Gpu.UsagePercent;
                DetGpuGauge.SubText = snap.Gpu.TemperatureText;
                DetGpuSub.Text = snap.Gpu.VramText;

                DetRamGauge.Value = snap.Ram.UsagePercent;
                DetRamGauge.SubText = snap.Ram.UsedGB >= 1.0
                    ? string.Format("{0:0} GB", Math.Round(snap.Ram.UsedGB))
                    : string.Format("{0:0} MB", snap.Ram.UsedGB * 1024);
                double detTotalRam = snap.Ram.InstalledGB > 0 ? snap.Ram.InstalledGB : snap.Ram.TotalGB;
                DetRamSub.Text = string.Format(System.Globalization.CultureInfo.InvariantCulture, "{0:0.0} / {1:0} GB", snap.Ram.UsedGB, detTotalRam);

                DetIGpuGauge.Value = snap.IntegratedGpu.IsAvailable ? snap.IntegratedGpu.UsagePercent : 0;
                DetIGpuGauge.SubText = snap.IntegratedGpu.IsAvailable ? snap.IntegratedGpu.TemperatureText : "N/A";
                DetIGpuSub.Text = snap.IntegratedGpu.IsAvailable
                    ? string.Format(System.Globalization.CultureInfo.InvariantCulture, "Paylaşımlı: {0:0.0} GB", snap.IntegratedGpu.UsedVramGB)
                    : "Algılanmadı";

                OverviewStorageText.Text = string.Format("Depolama (SSD): %{0:0} Aktif  |  {1}  |  Sıcaklık: {2}",
                    snap.Storage.UsagePercent, snap.Storage.IoText, snap.Storage.TemperatureText);

                OverviewNetText.Text = string.Format("Ağ ({0}): ↓ {1:0.00} MB/s | ↑ {2:0.00} MB/s",
                    snap.Network.AdapterName, snap.Network.DownloadMBps, snap.Network.UploadMBps);
                OverviewFpsText.Text = snap.Fps.HasValue ? string.Format("FPS: {0}", snap.Fps.Value) : "FPS: --";
            }
            else if (ViewHardware.Visibility == Visibility.Visible)
            {
                HwCpuName.Text = snap.Cpu.Name;
                HwCpuSpecs.Text = string.Format(
                    "Çekirdek / Thread: {0} / {1}   |   Taban Hız: {2:0.00} GHz   |   Anlık Hız: {3:0.00} GHz\nKullanım: %{4:0.0}   |   Sıcaklık: {5}",
                    snap.Cpu.PhysicalCores, snap.Cpu.LogicalProcessors, snap.Cpu.BaseClockGHz, snap.Cpu.CurrentClockGHz,
                    snap.Cpu.UsagePercent, snap.Cpu.TemperatureText);
                if (!snap.Cpu.TemperatureC.HasValue && !string.IsNullOrEmpty(snap.Cpu.TemperatureNote))
                {
                    HwCpuNote.Text = "ℹ " + snap.Cpu.TemperatureNote;
                    HwCpuNote.Visibility = Visibility.Visible;
                }
                else HwCpuNote.Visibility = Visibility.Collapsed;

                if (!_monitor.PawnIoInstalled)
                {
                    HwPawnIoText.Text = _isEnglish
                        ? "ℹ CPU temperature needs the PawnIO driver. Install it from"
                        : "ℹ CPU sıcaklığı için PawnIO sürücüsü gerekli. Kurulum:";
                    HwPawnIoNote.Visibility = Visibility.Visible;
                }
                else HwPawnIoNote.Visibility = Visibility.Collapsed;

                HwGpuName.Text = snap.Gpu.Name;
                HwGpuSpecs.Text = string.Format(
                    "VRAM: {0}   |   Kullanım: %{1:0.0}   |   Sıcaklık: {2}\nSaat Hızı: {3}   |   Fan: {4}   |   Sürücü: {5}",
                    snap.Gpu.VramText, snap.Gpu.UsagePercent, snap.Gpu.TemperatureText,
                    snap.Gpu.ClockMHz.HasValue ? snap.Gpu.ClockMHz.Value + " MHz" : "N/A",
                    snap.Gpu.FanPercent.HasValue ? "%" + snap.Gpu.FanPercent.Value : "N/A",
                    string.IsNullOrEmpty(snap.Gpu.DriverVersion) ? "N/A" : snap.Gpu.DriverVersion);
                if (!snap.Gpu.TemperatureC.HasValue && !string.IsNullOrEmpty(snap.Gpu.SensorNote))
                {
                    HwGpuNote.Text = "ℹ " + snap.Gpu.SensorNote;
                    HwGpuNote.Visibility = Visibility.Visible;
                }
                else HwGpuNote.Visibility = Visibility.Collapsed;

                HwIGpuName.Text = snap.IntegratedGpu.Name;
                HwIGpuSpecs.Text = snap.IntegratedGpu.IsAvailable
                    ? string.Format(System.Globalization.CultureInfo.InvariantCulture, "Kullanım: %{0:0.0}   |   Paylaşımlı Bellek: {1:0.00} GB   |   Sıcaklık: {2}   |   Sürücü: {3}",
                        snap.IntegratedGpu.UsagePercent, snap.IntegratedGpu.UsedVramGB, snap.IntegratedGpu.TemperatureText,
                        string.IsNullOrEmpty(snap.IntegratedGpu.DriverVersion) ? "N/A" : snap.IntegratedGpu.DriverVersion)
                    : "Sistemde yalnızca birincil ekran kartı algılandı veya dahili GPU BIOS üzerinden devre dışı.";

                string ramSpecInfo = snap.Ram.SpeedMHz > 0
                    ? string.Format("{0}, {1} MHz", snap.Ram.MemoryType, snap.Ram.SpeedMHz)
                    : "Tip/Hız için Yönetici izni gerekebilir";
                HwRamSummary.Text = string.Format(System.Globalization.CultureInfo.InvariantCulture, "{0:0.0} GB Takılı ({1:0.0} GB Kullanılabilir • {2}) — Kullanılan: {3:0.0} GB | Boş: {4:0.0} GB (%{5:0})",
                    snap.Ram.InstalledGB, snap.Ram.TotalGB, ramSpecInfo, snap.Ram.UsedGB, snap.Ram.FreeGB, snap.Ram.UsagePercent);
                HwRamSlots.Text = snap.Ram.Slots.Count > 0
                    ? string.Join("\n", snap.Ram.Slots.Select(s => string.Format("• {0}: {1:0.0} GB {2} @ {3} MHz ({4})", s.Locator, s.CapacityGB, s.MemoryType, s.SpeedMHz, s.Manufacturer)))
                    : "Slot detayları için Yönetici izni gerekebilir (SMBIOS erişimi).";

                HwDiskSummary.Text = string.Format("Toplam Sabit Depolama: {0:0} GB (Boş: {1:0} GB)  |  Okuma: {2:0.0} MB/s  |  Yazma: {3:0.0} MB/s  |  Sıcaklık: {4}",
                    snap.Storage.TotalGB, snap.Storage.FreeGB, snap.Storage.ReadMBps, snap.Storage.WriteMBps, snap.Storage.TemperatureText);
                HwDiskList.Text = snap.Storage.Disks.Count > 0
                    ? string.Join("\n", snap.Storage.Disks.Select(d => string.Format("• {0} — {1:0} GB ({2})", d.Model, d.TotalGB, d.MediaType)))
                    : "Disk detayları okunamadı.";
            }
            else if (ViewProcesses.Visibility == Visibility.Visible)
            {
                RefreshProcessList();
            }
        }

        private void RefreshProcessList()
        {
            if (_lastSnapshot == null) return;
            var all = _lastSnapshot.Processes;
            ProcSummaryText.Text = string.Format(System.Globalization.CultureInfo.InvariantCulture, "Aktif Öğe: {0}   |   Sistem RAM: {1:0.0} / {2:0.0} GB (%{3:0})",
                all.Count, _lastSnapshot.Ram.UsedGB, _lastSnapshot.Ram.TotalGB, _lastSnapshot.Ram.UsagePercent);

            IEnumerable<ProcessUsageItem> list = all;
            string query = (TxtProcSearch.Text ?? "").Trim();
            if (!string.IsNullOrEmpty(query))
            {
                list = list.Where(p => p.Name.IndexOf(query, StringComparison.OrdinalIgnoreCase) >= 0);
            }

            switch (CmbProcSort.SelectedIndex)
            {
                case 0: list = list.OrderByDescending(p => p.CpuPercent).ThenByDescending(p => p.RamMB); break;
                case 1: list = list.OrderByDescending(p => p.RamMB); break;
                case 2: list = list.OrderByDescending(p => p.DiskMBps); break;
                case 3: list = list.OrderBy(p => p.Name); break;
            }

            ProcListView.ItemsSource = list.Take(120).ToList();
        }

        private void HeaderBar_MouseLeftButtonDown(object sender, MouseButtonEventArgs e)
        {
            if (e.ClickCount == 2 && _isWindowMode)
            {
                BtnMaximize_Click(sender, e);
                return;
            }
            if (e.LeftButton == MouseButtonState.Pressed) DragMove();
        }

        private void Window_PreviewMouseDown(object sender, MouseButtonEventArgs e)
        {
            if (e.ChangedButton == MouseButton.Middle)
            {
                _isMiddleDragging = true;
                _dragStartScreen = PointToScreen(e.GetPosition(this));
                _windowStartPos = new Point(Left, Top);
                CaptureMouse();
                e.Handled = true;
            }
        }

        private void Window_PreviewMouseMove(object sender, MouseEventArgs e)
        {
            if (_isMiddleDragging && e.MiddleButton == MouseButtonState.Pressed)
            {
                Point cur = PointToScreen(e.GetPosition(this));
                Left = _windowStartPos.X + (cur.X - _dragStartScreen.X);
                Top = _windowStartPos.Y + (cur.Y - _dragStartScreen.Y);
            }
        }

        private void Window_PreviewMouseUp(object sender, MouseButtonEventArgs e)
        {
            if (_isMiddleDragging && e.ChangedButton == MouseButton.Middle)
            {
                _isMiddleDragging = false;
                ReleaseMouseCapture();
                e.Handled = true;
            }
        }

        private void BtnGhost_Click(object sender, RoutedEventArgs e)
        {
            ToggleGhostMode();
        }

        private void ToggleGhostMode()
        {
            _isGhostMode = !_isGhostMode;
            var hwnd = new WindowInteropHelper(this).Handle;
            int exStyle = NativeMethods.GetWindowLong(hwnd, NativeMethods.GWL_EXSTYLE);
            if (_isGhostMode)
            {
                NativeMethods.SetWindowLong(hwnd, NativeMethods.GWL_EXSTYLE, exStyle | NativeMethods.WS_EX_TRANSPARENT | NativeMethods.WS_EX_LAYERED);
                BtnGhost.Content = "👻 ✓";
            }
            else
            {
                NativeMethods.SetWindowLong(hwnd, NativeMethods.GWL_EXSTYLE, exStyle & ~NativeMethods.WS_EX_TRANSPARENT);
                BtnGhost.Content = "👻";
            }
            if (_lastSnapshot != null) UpdateUi(_lastSnapshot);
        }

        private void BtnTopmost_Click(object sender, RoutedEventArgs e)
        {
            _isDesktopPinned = false;
            Topmost = !Topmost;
            UpdatePinButtons();
            if (!Topmost)
            {
                var hwnd = new WindowInteropHelper(this).Handle;
                NativeMethods.SetWindowPos(hwnd, NativeMethods.HWND_NOTOPMOST, 0, 0, 0, 0, NativeMethods.SWP_NOMOVE | NativeMethods.SWP_NOSIZE | NativeMethods.SWP_NOACTIVATE);
            }
        }

        private void BtnDesktopPin_Click(object sender, RoutedEventArgs e)
        {
            _isDesktopPinned = !_isDesktopPinned;
            var hwnd = new WindowInteropHelper(this).Handle;
            if (_isDesktopPinned)
            {
                Topmost = false;
                NativeMethods.SetWindowPos(hwnd, NativeMethods.HWND_BOTTOM, 0, 0, 0, 0, NativeMethods.SWP_NOMOVE | NativeMethods.SWP_NOSIZE | NativeMethods.SWP_NOACTIVATE);
            }
            UpdatePinButtons();
        }

        private void UpdatePinButtons()
        {
            BtnTopmost.Content = Topmost ? "📌 ✓" : "📌";
            BtnDesktopPin.Content = _isDesktopPinned ? "🖥 ✓" : "🖥";
        }

        private void MiniPanel_MouseLeftButtonDown(object sender, MouseButtonEventArgs e)
        {
            if (!_isGhostMode && e.ChangedButton == MouseButton.Left)
            {
                try { DragMove(); } catch { }
            }
        }

        private void RestoreGlassChrome()
        {
            HeaderRow.Height = new GridLength(26);
            HeaderBar.Visibility = Visibility.Visible;
            RootContainer.Margin = new Thickness(5);
            MainContentGrid.Margin = new Thickness(12, 8, 12, 11);
            GlassFrame.BorderThickness = new Thickness(1.2);
            if (GlassBgBrush != null) GlassFrame.Background = GlassBgBrush;
            ApplyActiveOpacity();
        }

        private void BtnGameMode_Click(object sender, RoutedEventArgs e)
        {
            _isGameMode = !_isGameMode;
            if (_isGameMode)
            {
                if (_isWindowMode) BtnWindowMode_Click(sender, e);
                _isDetailed = false;
                HeaderRow.Height = new GridLength(0);
                HeaderBar.Visibility = Visibility.Collapsed;
                RootContainer.Margin = new Thickness(0);
                MainContentGrid.Margin = new Thickness(4, 2, 4, 2);
                GlassFrame.BorderThickness = new Thickness(0);
                GlassFrame.Background = Brushes.Transparent;

                MiniPanel.Visibility = Visibility.Visible;
                CompactPanel.Visibility = Visibility.Collapsed;
                DetailedPanel.Visibility = Visibility.Collapsed;
                MinWidth = 0;
                MaxWidth = 1000;
                SizeToContent = SizeToContent.WidthAndHeight;
                BtnGameMode.Content = _isEnglish ? "Summary ✓" : "Özet ✓";
                BtnToggleMode.Content = _isEnglish ? "Detail ⤢" : "Detay ⤢";
            }
            else
            {
                RestoreGlassChrome();
                SizeToContent = SizeToContent.WidthAndHeight;
                MinWidth = 0;
                MaxWidth = double.PositiveInfinity;
                MiniPanel.Visibility = Visibility.Collapsed;
                BtnGameMode.Content = _isEnglish ? "Summary ▬" : "Özet ▬";
                ToggleMode(false);
            }
        }

        private void BtnWindowMode_Click(object sender, RoutedEventArgs e)
        {
            SizeToContent = SizeToContent.Manual;
            MinWidth = 0;
            MaxWidth = double.PositiveInfinity;
            _isWindowMode = !_isWindowMode;
            _isGameMode = false;
            RestoreGlassChrome();
            MiniPanel.Visibility = Visibility.Collapsed;
            BtnGameMode.Content = _isEnglish ? "Summary ▬" : "Özet ▬";

            if (_isWindowMode)
            {
                Topmost = false;
                _isDesktopPinned = false;
                UpdatePinButtons();
                ResizeMode = ResizeMode.CanResizeWithGrip;
                SizeToContent = SizeToContent.Manual;
                DetailedPanel.MaxHeight = double.PositiveInfinity;
                ProcListView.MaxHeight = double.PositiveInfinity;
                Width = 860;
                Height = 600;
                BtnMaximize.Visibility = Visibility.Visible;
                BtnWindowMode.Content = "Widget ▣";
                ToggleMode(true);
                var area = SystemParameters.WorkArea;
                Left = area.Left + (area.Width - Width) / 2;
                Top = area.Top + (area.Height - Height) / 2;
            }
            else
            {
                WindowState = WindowState.Normal;
                ResizeMode = ResizeMode.NoResize;
                SizeToContent = SizeToContent.Height;
                DetailedPanel.MaxHeight = 440;
                ProcListView.MaxHeight = 300;
                BtnMaximize.Visibility = Visibility.Collapsed;
                BtnWindowMode.Content = "Pencere ▢";
                Topmost = true;
                UpdatePinButtons();
                ToggleMode(false);
                var area = SystemParameters.WorkArea;
                Left = area.Right - 410;
                Top = area.Top + 18;
            }
            ApplyActiveOpacity();
        }

        private void BtnMaximize_Click(object sender, RoutedEventArgs e)
        {
            WindowState = WindowState == WindowState.Maximized ? WindowState.Normal : WindowState.Maximized;
        }

        private void BtnToggleMode_Click(object sender, RoutedEventArgs e)
        {
            SizeToContent = _isWindowMode ? SizeToContent.Manual : SizeToContent.WidthAndHeight;
            MinWidth = 0;
            MaxWidth = double.PositiveInfinity;
            _isGameMode = false;
            RestoreGlassChrome();
            MiniPanel.Visibility = Visibility.Collapsed;
            BtnGameMode.Content = _isEnglish ? "Summary ▬" : "Özet ▬";
            ToggleMode(!_isDetailed);
        }

        private void ShortcutProcs_Click(object sender, RoutedEventArgs e)
        {
            ToggleMode(true);
            TabProcesses.IsChecked = true;
        }

        private void ShortcutHw_Click(object sender, RoutedEventArgs e)
        {
            ToggleMode(true);
            TabHardware.IsChecked = true;
        }

        private void ToggleMode(bool detailed)
        {
            _isDetailed = detailed;
            CompactPanel.Visibility = detailed ? Visibility.Collapsed : Visibility.Visible;
            DetailedPanel.Visibility = detailed ? Visibility.Visible : Visibility.Collapsed;
            BtnToggleMode.Content = detailed ? (_isEnglish ? "Compact ⤡" : "Küçült ⤡") : (_isEnglish ? "Detail ⤢" : "Detay ⤢");
            if (!_isWindowMode)
            {
                Width = detailed ? 770 : 440;
                var area = SystemParameters.WorkArea;
                if (Left + Width > area.Right) Left = Math.Max(area.Left, area.Right - Width - 12);
            }
            if (_lastSnapshot != null) UpdateUi(_lastSnapshot);
        }

        private void ProcCtxOpenLocation_Click(object sender, RoutedEventArgs e)
        {
            var item = ProcListView.SelectedItem as ProcessUsageItem;
            if (item == null) return;
            string path = item.ExecutablePath;
            if (string.IsNullOrEmpty(path) || !System.IO.File.Exists(path))
            {
                MessageBox.Show("Bu sistem sürecinin dosya konumu erişilemiyor.", "PulseGlass", MessageBoxButton.OK, MessageBoxImage.Information);
                return;
            }
            try { Process.Start("explorer.exe", "/select,\"" + path + "\""); }
            catch { }
        }

        private void ProcCtxCopyPath_Click(object sender, RoutedEventArgs e)
        {
            var item = ProcListView.SelectedItem as ProcessUsageItem;
            if (item != null && !string.IsNullOrEmpty(item.ExecutablePath))
            {
                Clipboard.SetText(item.ExecutablePath);
            }
        }

        private async void ProcCtxKill_Click(object sender, RoutedEventArgs e)
        {
            var item = ProcListView.SelectedItem as ProcessUsageItem;
            if (item == null) return;
            bool grouped = (ChkGroupProcs.IsChecked == true);
            string procName = item.Name;
            int pid = item.Pid;

            await Task.Run(() =>
            {
                try
                {
                    if (grouped)
                    {
                        foreach (var p in Process.GetProcessesByName(procName))
                        {
                            try { p.Kill(); } catch { } finally { p.Dispose(); }
                        }
                    }
                    else
                    {
                        using (var p = Process.GetProcessById(pid)) { p.Kill(); }
                    }
                }
                catch { }
            });
            await PollOnceAsync();
        }

        private void PresetTheme_Click(object sender, RoutedEventArgs e)
        {
            var btn = sender as Button;
            if (btn == null || btn.Tag == null) return;
            var parts = btn.Tag.ToString().Split('|');
            if (parts.Length < 3) return;
            TxtPrimaryHex.Text = parts[0];
            TxtSecondaryHex.Text = parts[1];
            TxtBgHex.Text = parts[2];
            BtnApplyCustomTheme_Click(sender, e);
        }

        private void BtnApplyCustomTheme_Click(object sender, RoutedEventArgs e)
        {
            try
            {
                Color p = (Color)ColorConverter.ConvertFromString(TxtPrimaryHex.Text.Trim());
                Color s = (Color)ColorConverter.ConvertFromString(TxtSecondaryHex.Text.Trim());
                Color bg = (Color)ColorConverter.ConvertFromString(TxtBgHex.Text.Trim());
                ApplyCustomTheme(p, s, bg);
            }
            catch
            {
                MessageBox.Show("Geçerli Hex renk kodu girin (ör. #FF4D6D).", "Renk Hatası", MessageBoxButton.OK, MessageBoxImage.Warning);
            }
        }

        private void ApplyCustomTheme(Color primary, Color secondary, Color bg)
        {
            _primaryColor = primary;
            _secondaryColor = secondary;
            _bgBaseColor = bg;

            var pBrush = new SolidColorBrush(primary); pBrush.Freeze();
            var sBrush = new SolidColorBrush(secondary); sBrush.Freeze();
            Resources["NarAccent"] = pBrush;
            Resources["SeftaliAccent"] = sBrush;

            var gauges = new[]
            {
                CompactCpuGauge, CompactGpuGauge, CompactRamGauge, CompactIGpuGauge,
                DetCpuGauge, DetGpuGauge, DetRamGauge, DetIGpuGauge
            };
            foreach (var g in gauges)
            {
                g.NormalColor = secondary;
                g.WarnColor = primary;
            }

            ApplyActiveOpacity();
            if (_lastSnapshot != null) UpdateUi(_lastSnapshot);
        }

        private void BtnElevateAdmin_Click(object sender, RoutedEventArgs e)
        {
            try
            {
                var exe = Process.GetCurrentProcess().MainModule.FileName;
                var psi = new ProcessStartInfo(exe) { UseShellExecute = true, Verb = "runas" };
                Process.Start(psi);
                Close();
            }
            catch { }
        }

        private void Tab_Checked(object sender, RoutedEventArgs e)
        {
            if (!_uiReady || ViewOverview == null) return;
            ViewOverview.Visibility = TabOverview.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            ViewHardware.Visibility = TabHardware.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            ViewProcesses.Visibility = TabProcesses.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            ViewSettings.Visibility = TabSettings.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            ViewUpdates.Visibility = TabUpdates.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;

            if (TabProcesses.IsChecked == true)
            {
                var ignored = PollOnceAsync();
            }
        }

        private void TxtProcSearch_TextChanged(object sender, TextChangedEventArgs e)
        {
            SearchPlaceholder.Visibility = string.IsNullOrEmpty(TxtProcSearch.Text) ? Visibility.Visible : Visibility.Collapsed;
            RefreshProcessList();
        }

        private void CmbProcSort_SelectionChanged(object sender, SelectionChangedEventArgs e) { if (_uiReady) RefreshProcessList(); }
        private void ChkGroupProcs_Changed(object sender, RoutedEventArgs e) { if (_uiReady) PollOnceAsync(); }

        private void SldWidgetOpacity_ValueChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
        {
            if (!_uiReady || LblWidgetOpacityVal == null) return;
            _widgetOpacity = e.NewValue;
            LblWidgetOpacityVal.Text = string.Format("{0:0}%", _widgetOpacity * 100);
            if (!_isWindowMode && !_isGameMode) ApplyActiveOpacity();
        }

        private void SldWindowOpacity_ValueChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
        {
            if (!_uiReady || LblWindowOpacityVal == null) return;
            _windowOpacity = e.NewValue;
            LblWindowOpacityVal.Text = string.Format("{0:0}%", _windowOpacity * 100);
            if (_isWindowMode && !_isGameMode) ApplyActiveOpacity();
        }

        private void ApplyActiveOpacity()
        {
            if (GlassFrame == null || BgStop1 == null || BgStop2 == null) return;
            if (_isGameMode)
            {
                GlassFrame.Background = Brushes.Transparent;
                return;
            }
            if (GlassBgBrush != null && GlassFrame.Background != GlassBgBrush)
            {
                GlassFrame.Background = GlassBgBrush;
            }
            double op = _isWindowMode ? _windowOpacity : _widgetOpacity;
            byte alpha = (byte)(op * 255);
            BgStop1.Color = Color.FromArgb(alpha, _bgBaseColor.R, _bgBaseColor.G, _bgBaseColor.B);
            BgStop2.Color = Color.FromArgb((byte)Math.Min(255, alpha + 10),
                (byte)Math.Min(255, _bgBaseColor.R + 10),
                (byte)Math.Min(255, _bgBaseColor.G + 4),
                (byte)Math.Min(255, _bgBaseColor.B + 6));
        }

        private void SldScale_ValueChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
        {
            if (!_uiReady || WidgetScale == null || LblScaleVal == null) return;
            WidgetScale.ScaleX = e.NewValue;
            WidgetScale.ScaleY = e.NewValue;
            LblScaleVal.Text = string.Format("{0:0.00}x", e.NewValue);
        }

        private void CmbRefreshRate_SelectionChanged(object sender, SelectionChangedEventArgs e)
        {
            if (!_uiReady) return;
            var item = CmbRefreshRate.SelectedItem as ComboBoxItem;
            if (item != null && item.Tag != null)
            {
                int ms;
                if (int.TryParse(item.Tag.ToString(), out ms))
                {
                    _timer.Interval = TimeSpan.FromMilliseconds(ms);
                }
            }
        }

        private void Threshold_Changed(object sender, RoutedPropertyChangedEventArgs<double> e)
        {
            if (!_uiReady || LblCpuTempThresh == null || LblRamUseThresh == null) return;
            _monitor.CpuTempAlertThreshold = SldCpuTempThresh.Value;
            _monitor.GpuTempAlertThreshold = SldGpuTempThresh.Value;
            _monitor.CpuUsageAlertThreshold = SldCpuUseThresh.Value;
            _monitor.RamUsageAlertThreshold = SldRamUseThresh.Value;

            LblCpuTempThresh.Text = string.Format("CPU Sıcaklık Uyarısı: {0:0}°C", SldCpuTempThresh.Value);
            LblGpuTempThresh.Text = string.Format("GPU Sıcaklık Uyarısı: {0:0}°C", SldGpuTempThresh.Value);
            LblCpuUseThresh.Text = string.Format("CPU Kullanım Uyarısı: %{0:0}", SldCpuUseThresh.Value);
            LblRamUseThresh.Text = string.Format("RAM Kullanım Uyarısı: %{0:0}", SldRamUseThresh.Value);
        }

        private void ChkAutoStart_Changed(object sender, RoutedEventArgs e)
        {
            if (!_uiReady) return;
            try
            {
                string exe = Process.GetCurrentProcess().MainModule.FileName;
                SettingsService.SetAutoStart(ChkAutoStart.IsChecked == true, exe);
            }
            catch { }
        }

        private void BtnCreateShortcut_Click(object sender, RoutedEventArgs e)
        {
            try
            {
                string exe = Process.GetCurrentProcess().MainModule.FileName;
                if (SettingsService.CreateDesktopShortcut(exe))
                    MessageBox.Show("Masaüstü kısayolu oluşturuldu: PulseGlass Monitor", "PulseGlass", MessageBoxButton.OK, MessageBoxImage.Information);
                else
                    MessageBox.Show("Kısayol oluşturulamadı.", "PulseGlass", MessageBoxButton.OK, MessageBoxImage.Warning);
            }
            catch { }
        }

        private void ChkSummary_Changed(object sender, RoutedEventArgs e)
        {
            if (!_uiReady) return;
            SaveCurrentSettings();
            if (_lastSnapshot != null) UpdateUi(_lastSnapshot);
        }

        private void BtnResetDefaults_Click(object sender, RoutedEventArgs e)
        {
            var res = MessageBox.Show(
                "Tüm görünüm, renk, eşik ve özet ayarları varsayılan değerlere sıfırlansın mı?",
                "Varsayılanlara Sıfırla",
                MessageBoxButton.YesNo,
                MessageBoxImage.Question);
            if (res != MessageBoxResult.Yes) return;

            var def = new AppSettingsData();
            _uiReady = false;
            SldWidgetOpacity.Value = def.WidgetOpacity;
            SldWindowOpacity.Value = def.WindowOpacity;
            _widgetOpacity = def.WidgetOpacity;
            _windowOpacity = def.WindowOpacity;
            LblWidgetOpacityVal.Text = string.Format("{0:0}%", _widgetOpacity * 100);
            LblWindowOpacityVal.Text = string.Format("{0:0}%", _windowOpacity * 100);

            SldScale.Value = def.Scale;
            WidgetScale.ScaleX = def.Scale;
            WidgetScale.ScaleY = def.Scale;
            LblScaleVal.Text = string.Format("{0:0.00}x", def.Scale);

            TxtPrimaryHex.Text = def.PrimaryHex;
            TxtSecondaryHex.Text = def.SecondaryHex;
            TxtBgHex.Text = def.BgHex;
            ApplyCustomTheme(
                (Color)ColorConverter.ConvertFromString(def.PrimaryHex),
                (Color)ColorConverter.ConvertFromString(def.SecondaryHex),
                (Color)ColorConverter.ConvertFromString(def.BgHex));

            SldCpuTempThresh.Value = def.CpuTempThresh;
            SldGpuTempThresh.Value = def.GpuTempThresh;
            SldCpuUseThresh.Value = def.CpuUseThresh;
            SldRamUseThresh.Value = def.RamUseThresh;

            ChkSumCpuPct.IsChecked = def.SumCpuPct;
            ChkSumCpuTemp.IsChecked = def.SumCpuTemp;
            ChkSumGpuPct.IsChecked = def.SumGpuPct;
            ChkSumGpuTemp.IsChecked = def.SumGpuTemp;
            ChkSumIGpuPct.IsChecked = def.SumIGpuPct;
            ChkSumRamPct.IsChecked = def.SumRamPct;
            ChkSumSsdTemp.IsChecked = def.SumSsdTemp;
            ChkSumFps.IsChecked = def.SumFps;

            _uiReady = true;
            Threshold_Changed(null, null);
            SaveCurrentSettings();
            if (_lastSnapshot != null) UpdateUi(_lastSnapshot);
        }

        private async void BtnCheckUpdates_Click(object sender, RoutedEventArgs e)
        {
            await RunUpdateCheckAsync(true);
        }

        private async Task RunUpdateCheckAsync(bool userInitiated)
        {
            if (_isCheckingUpdates) return;
            _isCheckingUpdates = true;
            BtnCheckUpdates.IsEnabled = false;
            TxtWinUpdateStatus.Text = _isEnglish
                ? "Checking for Windows and PulseGlass updates..."
                : "Windows ve PulseGlass güncellemeleri denetleniyor...";
            try
            {
                bool en = _isEnglish;
                var result = await Task.Run(() => _updateService.CheckAllUpdates(en));
                _lastUpdateResult = result;

                WinUpdatesList.ItemsSource = result.WindowsUpdates;
                WinOptionalUpdatesList.ItemsSource = result.OptionalWindowsUpdates;
                bool hasOpt = result.OptionalWindowsUpdates != null && result.OptionalWindowsUpdates.Count > 0;
                TxtNoOptionalUpdates.Visibility = hasOpt ? Visibility.Collapsed : Visibility.Visible;

                TxtWinUpdateStatus.Text = result.WindowsStatus;
                bool hasImp = result.WindowsUpdates != null && result.WindowsUpdates.Count > 0;
                BtnInstallWinUpdates.IsEnabled = hasImp || hasOpt;

                TxtAppUpdateStatus.Text = result.AppStatus;
                BtnInstallAppUpdate.Visibility = result.AppUpdateAvailable ? Visibility.Visible : Visibility.Collapsed;

                _hasPendingUpdates = hasImp || hasOpt || result.AppUpdateAvailable;
                TabUpdates.Content = (_isEnglish ? "Updates" : "Güncellemeler") + (_hasPendingUpdates ? " •" : "");

                if (_hasPendingUpdates && _trayIcon != null && !userInitiated)
                {
                    int winCount = (result.WindowsUpdates != null ? result.WindowsUpdates.Count : 0)
                                 + (result.OptionalWindowsUpdates != null ? result.OptionalWindowsUpdates.Count : 0);
                    string msg = winCount > 0
                        ? string.Format(_isEnglish ? "{0} Windows update(s) found." : "{0} Windows güncellemesi bulundu.", winCount)
                        : string.Format(_isEnglish ? "PulseGlass v{0} is ready." : "PulseGlass v{0} güncellemesi hazır.", result.LatestAppVersion);
                    string tipSuffix = _isEnglish ? " Open the Updates tab to install." : " Kurmak için Güncellemeler sekmesini açın.";
                    _trayIcon.ShowBalloonTip(4000, _isEnglish ? "PulseGlass Update" : "PulseGlass Güncelleme", msg + tipSuffix, System.Windows.Forms.ToolTipIcon.Info);
                }
            }
            catch (Exception ex)
            {
                TxtWinUpdateStatus.Text = (_isEnglish ? "Check error: " : "Denetleme hatası: ") + ex.Message;
            }
            finally
            {
                BtnCheckUpdates.IsEnabled = true;
                _isCheckingUpdates = false;
            }
        }

        private async void BtnInstallWinUpdates_Click(object sender, RoutedEventArgs e)
        {
            if (_lastUpdateResult == null) return;
            var selected = new List<WindowsUpdateItem>();
            if (_lastUpdateResult.WindowsUpdates != null)
                selected.AddRange(_lastUpdateResult.WindowsUpdates.Where(u => u.IsSelected));
            if (_lastUpdateResult.OptionalWindowsUpdates != null)
                selected.AddRange(_lastUpdateResult.OptionalWindowsUpdates.Where(u => u.IsSelected));

            if (selected.Count == 0)
            {
                MessageBox.Show(
                    _isEnglish ? "Please select at least one Windows update to install." : "Lütfen kurmak istediğiniz en az bir Windows güncellemesi seçin.",
                    "PulseGlass", MessageBoxButton.OK, MessageBoxImage.Information);
                return;
            }

            var confirm = MessageBox.Show(
                string.Format(_isEnglish ? "{0} Windows update(s) will be downloaded and installed. Do you confirm?" : "{0} adet Windows güncellemesi indirilecek ve kurulacak. Onaylıyor musunuz?", selected.Count),
                _isEnglish ? "Windows Update Confirmation" : "Windows Güncelleme Onayı",
                MessageBoxButton.YesNo,
                MessageBoxImage.Question);
            if (confirm != MessageBoxResult.Yes) return;

            BtnInstallWinUpdates.IsEnabled = false;
            BtnCheckUpdates.IsEnabled = false;
            bool en = _isEnglish;
            string status = await Task.Run(() => _updateService.InstallSelectedWindowsUpdates(selected, msg =>
            {
                Dispatcher.Invoke(() => { TxtWinUpdateStatus.Text = msg; });
            }, en));
            TxtWinUpdateStatus.Text = status;
            BtnCheckUpdates.IsEnabled = true;
            await RunUpdateCheckAsync(true);
        }

        private async void BtnInstallAppUpdate_Click(object sender, RoutedEventArgs e)
        {
            if (_lastUpdateResult == null || !_lastUpdateResult.AppUpdateAvailable || string.IsNullOrEmpty(_lastUpdateResult.AppDownloadUrl)) return;

            var confirm = MessageBox.Show(
                string.Format(_isEnglish ? "PulseGlass v{0} will be downloaded and the app will restart to update. Do you confirm?" : "PulseGlass v{0} indirilecek ve uygulama yeniden başlatılarak güncellenecek. Onaylıyor musunuz?", _lastUpdateResult.LatestAppVersion),
                _isEnglish ? "PulseGlass Update Confirmation" : "PulseGlass Güncelleme Onayı",
                MessageBoxButton.YesNo,
                MessageBoxImage.Question);
            if (confirm != MessageBoxResult.Yes) return;

            BtnInstallAppUpdate.IsEnabled = false;
            TxtAppUpdateStatus.Text = _isEnglish ? "Downloading new version..." : "Yeni sürüm indiriliyor...";
            string url = _lastUpdateResult.AppDownloadUrl;
            bool en = _isEnglish;
            string res = await Task.Run(() => _updateService.DownloadAndApplyAppUpdate(url, en));
            if (res == "OK")
            {
                Close();
            }
            else
            {
                TxtAppUpdateStatus.Text = res;
                BtnInstallAppUpdate.IsEnabled = true;
            }
        }

        private void PawnIoLink_RequestNavigate(object sender, System.Windows.Navigation.RequestNavigateEventArgs e)
        {
            // PulseGlass runs elevated. Going through explorer opens the browser as the normal user.
            try { Process.Start("explorer.exe", "\"" + e.Uri.AbsoluteUri + "\""); } catch { }
            e.Handled = true;
        }

        private void CmbLanguage_SelectionChanged(object sender, SelectionChangedEventArgs e)
        {
            if (!_uiReady) return;
            var item = CmbLanguage.SelectedItem as ComboBoxItem;
            if (item == null || item.Tag == null) return;
            _isEnglish = string.Equals(item.Tag.ToString(), "en", StringComparison.OrdinalIgnoreCase);
            ApplyLanguage(_isEnglish);
            SaveCurrentSettings();
        }

        private void ApplyLanguage(bool en)
        {
            if (TabOverview == null) return;

            // Header & Compact
            BtnGhost.ToolTip = en ? "Ghost Mode / Click-through (Shift+Enter)" : "Geçirgen Mod / Arkaya Tıkla (Shift+Enter)";
            BtnTopmost.ToolTip = en ? "Always on top" : "Her zaman üstte";
            BtnDesktopPin.ToolTip = en ? "Pin to desktop" : "Masaüstüne sabitle";
            BtnGameMode.Content = en ? "Summary ▬" : "Özet ▬";
            BtnGameMode.ToolTip = en ? "Summary / Mini Strip Mode" : "Özet / Mini Şerit Modu";
            BtnWindowMode.Content = en ? "Window ▢" : "Pencere ▢";
            BtnWindowMode.ToolTip = en ? "Full window / Widget mode" : "Tam pencere / Widget modu";
            BtnToggleMode.Content = _isDetailed ? (en ? "Compact ⤡" : "Kompakt ⤡") : (en ? "Detail ⤢" : "Detay ⤢");
            BtnToggleMode.ToolTip = en ? "Compact / Detailed view" : "Kompakt / Detaylı görünüm";
            BtnMaximize.ToolTip = en ? "Maximize / Restore" : "Büyüt / Geri yükle";
            BtnMinimize.ToolTip = en ? "Minimize" : "Küçült";
            BtnClose.ToolTip = en ? "Close" : "Kapat";

            LblCompactGpu.Text = en ? "Discrete GPU" : "Harici GPU";
            LblCompactIGpu.Text = en ? "Integrated GPU" : "Dahili GPU";
            BtnShortcutProcs.Content = en ? "📊 Apps" : "📊 Uygulamalar";
            BtnShortcutHw.Content = en ? "🖥 Hardware" : "🖥 Donanım";

            // Tabs
            TabOverview.Content = en ? "Overview" : "Genel Bakış";
            TabHardware.Content = en ? "Hardware Details" : "Donanım Detayları";
            TabProcesses.Content = en ? "Processes" : "Uygulamalar";
            TabSettings.Content = en ? "Settings" : "Ayarlar";
            TabUpdates.Content = (en ? "Updates" : "Güncellemeler") + (_hasPendingUpdates ? " •" : "");

            // Overview
            LblDetCpu.Text = en ? "Processor (CPU)" : "İşlemci (CPU)";
            LblDetGpu.Text = en ? "Discrete GPU" : "Harici GPU";
            LblDetIGpu.Text = en ? "Integrated GPU (iGPU)" : "Dahili GPU (iGPU)";
            LblDetRam.Text = en ? "Memory (RAM)" : "Bellek (RAM)";

            // Hardware
            LblHwCpuHeader.Text = en ? "PROCESSOR (CPU)" : "İŞLEMCİ (CPU)";
            LblHwGpuHeader.Text = en ? "DISCRETE GRAPHICS (dGPU)" : "HARİCİ EKRAN KARTI (dGPU)";
            LblHwIGpuHeader.Text = en ? "INTEGRATED GRAPHICS (iGPU)" : "DAHİLİ EKRAN KARTI (iGPU - İŞLEMCİ GRAFİĞİ)";
            LblHwRamHeader.Text = en ? "MEMORY (RAM)" : "BELLEK (RAM)";
            LblHwDiskHeader.Text = en ? "STORAGE (SSD / HDD)" : "DEPOLAMA (SSD / HDD)";

            // Processes
            LblProcLive.Text = en ? "Live Updates" : "Canlı Güncelleme";
            SearchPlaceholder.Text = en ? "Search app or process..." : "Uygulama veya süreç ara...";
            CmbSortCpu.Content = en ? "Sort: CPU %" : "Sırala: CPU %";
            CmbSortRam.Content = en ? "Sort: RAM" : "Sırala: RAM";
            CmbSortDisk.Content = en ? "Sort: Disk" : "Sırala: Disk";
            CmbSortName.Content = en ? "Sort: Name" : "Sırala: İsim";
            ChkGroupProcs.Content = en ? "Group" : "Grupla";
            CtxOpenLoc.Header = en ? "📂 Open File Location" : "📂 Dosya Konumunu Aç";
            CtxCopyPath.Header = en ? "📋 Copy File Path" : "📋 Dosya Yolunu Kopyala";
            CtxKillProc.Header = en ? "⚡ End Task (Kill Now)" : "⚡ Görevi Sonlandır (Anında Kapat)";

            // Settings
            LblSetOpacityHeader.Text = en ? "OPACITY & APPEARANCE (WIDGET & WINDOW)" : "ŞEFFAFLIK VE GÖRÜNÜM (WIDGET & PENCERE AYRI)";
            LblWidgetOpacity.Text = en ? "Widget Opacity" : "Widget Şeffaflığı (Opaklık)";
            LblWindowOpacity.Text = en ? "Window Mode Opacity" : "Pencere Modu Şeffaflığı (Opaklık)";
            LblScale.Text = en ? "Widget Size (Scale)" : "Widget Boyutu (Ölçek)";
            LblRefreshRate.Text = en ? "Refresh Interval" : "Veri Yenileme Sıklığı";
            CmbRef1.Content = en ? "1 Second" : "1 Saniye";
            CmbRef2.Content = en ? "2 Seconds" : "2 Saniye";
            CmbRef3.Content = en ? "3 Seconds" : "3 Saniye";
            CmbRef5.Content = en ? "5 Seconds" : "5 Saniye";
            LblGhostTip.Text = en
                ? "Tip: Enable 'Ghost' mode to click through the widget onto windows behind it. Press Shift + Enter to toggle."
                : "İpucu: 'Geçirgen' modu açtığınızda widget arkasındaki pencerelere tıklayabilirsiniz. Açıp kapatmak için Shift + Enter kısayolunu kullanın.";
            ChkAutoStart.Content = en ? "Start with Windows" : "Windows Açılışında Başlat";
            BtnCreateShortcut.Content = en ? "🖥 Create Desktop Shortcut" : "🖥 Masaüstü Kısayolu Oluştur";
            LblThemeHeader.Text = en ? "COLOR PALETTE EDITOR" : "RENK PALETİ DÜZENLEYİCİ";
            BtnThemeNar.Content = en ? "Pomegranate" : "Nar & Şeftali";
            BtnThemeOcean.Content = en ? "Ocean" : "Okyanus";
            BtnThemePurple.Content = en ? "Purple Neon" : "Mor & Neon";
            BtnThemeEmerald.Content = en ? "Emerald" : "Zümrüt";
            BtnThemeGold.Content = en ? "Gold" : "Altın";
            LblHexPrimary.Text = en ? "Primary Accent (Hex)" : "Ana Vurgu (Hex)";
            LblHexSecondary.Text = en ? "Secondary Accent (Hex)" : "İkincil Vurgu (Hex)";
            LblHexBg.Text = en ? "Background (Hex)" : "Arka Plan (Hex)";
            BtnApplyTheme.Content = en ? "Apply Custom Palette" : "Özel Renk Paletini Uygula";
            LblSummaryHeader.Text = en ? "SUMMARY STRIP METRICS (CUSTOMIZE)" : "ÖZET ŞERİT GÖRÜNÜMÜ (ÖZELLEŞTİR)";
            LblThreshHeader.Text = en ? "ALERT THRESHOLDS" : "UYARI EŞİK DEĞERLERİ";
            BtnResetDefaults.Content = en ? "Reset to Defaults" : "Varsayılanlara Sıfırla";

            // Updates
            LblUpdateCenterHeader.Text = en ? "UPDATE CENTER" : "GÜNCELLEME MERKEZİ";
            LblUpdateScheduleNote.Text = en ? "Automatically checks at startup and every 6 hours." : "Açılışta ve her 6 saatte bir otomatik denetlenir.";
            BtnCheckUpdates.Content = en ? "🔄 Check Now" : "🔄 Şimdi Denetle";
            LblWinUpdatesHeader.Text = en ? "WINDOWS SYSTEM UPDATES" : "WINDOWS SİSTEM GÜNCELLEMELERİ";
            BtnInstallWinUpdates.Content = en ? "Install Selected Updates" : "Seçili Güncellemeleri Kur";
            LblImportantUpdatesHeader.Text = en ? "IMPORTANT / RECOMMENDED UPDATES" : "ÖNEMLİ / ÖNERİLEN GÜNCELLEMELER";
            LblOptionalUpdatesHeader.Text = en ? "OPTIONAL UPDATES" : "İSTEĞE BAĞLI GÜNCELLEMELER";
            TxtNoOptionalUpdates.Text = en ? "No optional updates found." : "İsteğe bağlı güncelleme bulunamadı.";
            LblAppUpdateHeader.Text = en ? "PULSEGLASS APP UPDATE" : "PULSEGLASS UYGULAMA GÜNCELLEMESİ";
            BtnInstallAppUpdate.Content = en ? "Update App" : "Uygulamayı Güncelle";

            if (_uiReady)
            {
                LblCpuTempThresh.Text = string.Format(en ? "CPU Temp Alert: {0:0}°C" : "CPU Sıcaklık Uyarısı: {0:0}°C", SldCpuTempThresh.Value);
                LblGpuTempThresh.Text = string.Format(en ? "GPU Temp Alert: {0:0}°C" : "GPU Sıcaklık Uyarısı: {0:0}°C", SldGpuTempThresh.Value);
                LblCpuUseThresh.Text = string.Format(en ? "CPU Usage Alert: {0:0}%" : "CPU Kullanım Uyarısı: %{0:0}", SldCpuUseThresh.Value);
                LblRamUseThresh.Text = string.Format(en ? "RAM Usage Alert: {0:0}%" : "RAM Kullanım Uyarısı: %{0:0}", SldRamUseThresh.Value);
                if (_lastSnapshot != null) UpdateUi(_lastSnapshot);
            }
        }

        private void BtnMinimize_Click(object sender, RoutedEventArgs e) { WindowState = WindowState.Minimized; }
        private void BtnClose_Click(object sender, RoutedEventArgs e) { Close(); }
    }
}
