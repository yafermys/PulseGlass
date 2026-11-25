using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Net;
using System.Text.RegularExpressions;
using PulseGlassWidget.Models;

namespace PulseGlassWidget.Services
{
    public class WindowsUpdateItem : NotifyBase
    {
        public string UpdateId { get; set; }
        public string Title { get; set; }
        public string SizeText { get; set; }
        public bool RebootRequired { get; set; }
        public bool IsOptional { get; set; }
        public string RebootLabel { get; set; }
        public int Index { get; set; }

        private bool _isSelected = true;
        public bool IsSelected { get { return _isSelected; } set { SetField(ref _isSelected, value); } }

        public string MetaText
        {
            get { return string.Format("{0}{1}", SizeText, RebootRequired ? ("  •  " + (RebootLabel ?? "Yeniden Başlatma Gerekebilir")) : ""); }
        }
    }

    public class UpdateCheckResult
    {
        public List<WindowsUpdateItem> WindowsUpdates { get; set; }
        public List<WindowsUpdateItem> OptionalWindowsUpdates { get; set; }
        public string WindowsStatus { get; set; }
        public bool AppUpdateAvailable { get; set; }
        public string LatestAppVersion { get; set; }
        public string AppDownloadUrl { get; set; }
        public string AppStatus { get; set; }

        public UpdateCheckResult()
        {
            WindowsUpdates = new List<WindowsUpdateItem>();
            OptionalWindowsUpdates = new List<WindowsUpdateItem>();
        }
    }

    public class UpdateService
    {
        public const string CurrentVersion = "1.3.0";
        // GitHub Releases API or custom JSON manifest URL
        public static string AppManifestUrl = "https://api.github.com/repos/yafermys/PulseGlass/releases/latest";

        private dynamic _cachedSearchResult;

        public UpdateCheckResult CheckAllUpdates(bool isEn = false)
        {
            var res = new UpdateCheckResult();
            CheckWindowsUpdates(res, isEn);
            CheckAppUpdate(res, isEn);
            return res;
        }

        private void CheckWindowsUpdates(UpdateCheckResult res, bool isEn)
        {
            try
            {
                Type sessionType = Type.GetTypeFromProgID("Microsoft.Update.Session");
                if (sessionType == null)
                {
                    res.WindowsStatus = isEn
                        ? "Windows Update service is unavailable on this system."
                        : "Windows Update servisi bu sistemde kullanılamıyor.";
                    return;
                }
                dynamic session = Activator.CreateInstance(sessionType);
                dynamic searcher = session.CreateUpdateSearcher();
                dynamic searchResult = searcher.Search("IsInstalled=0 and IsHidden=0");
                _cachedSearchResult = searchResult;

                int count = (int)searchResult.Updates.Count;
                for (int i = 0; i < count; i++)
                {
                    dynamic upd = searchResult.Updates.Item(i);
                    double maxMb = Convert.ToDouble(upd.MaxDownloadSize) / (1024.0 * 1024.0);
                    bool reboot = false;
                    try { reboot = Convert.ToInt32(upd.InstallationBehavior.RebootBehavior) > 0; } catch { }

                    bool browseOnly = false;
                    bool autoSelect = true;
                    bool isMandatory = false;
                    try { browseOnly = Convert.ToBoolean(upd.BrowseOnly); } catch { }
                    try { autoSelect = Convert.ToBoolean(upd.AutoSelectOnWebSites); } catch { }
                    try { isMandatory = Convert.ToBoolean(upd.IsMandatory); } catch { }
                    bool isOptional = !isMandatory && (browseOnly || !autoSelect);

                    var item = new WindowsUpdateItem
                    {
                        Index = i,
                        UpdateId = upd.Identity != null ? (string)upd.Identity.UpdateID : i.ToString(),
                        Title = (string)upd.Title,
                        SizeText = maxMb > 0 ? string.Format("{0:0.0} MB", maxMb) : (isEn ? "Size: Variable" : "Boyut: Değişken"),
                        RebootRequired = reboot,
                        RebootLabel = isEn ? "Restart May Be Required" : "Yeniden Başlatma Gerekebilir",
                        IsOptional = isOptional,
                        IsSelected = !isOptional
                    };

                    if (isOptional)
                        res.OptionalWindowsUpdates.Add(item);
                    else
                        res.WindowsUpdates.Add(item);
                }

                int impCount = res.WindowsUpdates.Count;
                int optCount = res.OptionalWindowsUpdates.Count;
                if (impCount == 0 && optCount == 0)
                {
                    res.WindowsStatus = isEn ? "Your Windows system is up to date." : "Windows sisteminiz güncel.";
                }
                else
                {
                    res.WindowsStatus = isEn
                        ? string.Format("{0} important, {1} optional Windows update(s) found.", impCount, optCount)
                        : string.Format("{0} önemli, {1} isteğe bağlı Windows güncellemesi bulundu.", impCount, optCount);
                }
            }
            catch (Exception ex)
            {
                res.WindowsStatus = (isEn ? "Windows Update check failed: " : "Windows Update denetlenemedi: ") + ex.Message;
            }
        }

        private void CheckAppUpdate(UpdateCheckResult res, bool isEn)
        {
            if (string.IsNullOrEmpty(AppManifestUrl))
            {
                res.LatestAppVersion = CurrentVersion;
                res.AppUpdateAvailable = false;
                res.AppStatus = string.Format(isEn ? "PulseGlass v{0} (Up to date)" : "PulseGlass v{0} (Güncel)", CurrentVersion);
                return;
            }

            try
            {
                ServicePointManager.SecurityProtocol = SecurityProtocolType.Tls12;
                using (var wc = new WebClient())
                {
                    wc.Headers.Add("User-Agent", "PulseGlassWidget/" + CurrentVersion);
                    string json = wc.DownloadString(AppManifestUrl);

                    // Support both GitHub Releases API ("tag_name" / "browser_download_url") and custom manifest ("version" / "url")
                    var vMatch = Regex.Match(json, "\"(?:version|tag_name)\"\\s*:\\s*\"([^\"]+)\"");
                    // Only accept an .exe link. In the GitHub API response the first "url" is the
                    // release's own API endpoint, so a non-exe fallback would overwrite the app with JSON.
                    var uMatch = Regex.Match(json, "\"browser_download_url\"\\s*:\\s*\"([^\"]+/PulseGlassWidget\\.exe)\"", RegexOptions.IgnoreCase);
                    if (!uMatch.Success)
                        uMatch = Regex.Match(json, "\"(?:browser_download_url|url)\"\\s*:\\s*\"([^\"]+\\.exe)\"", RegexOptions.IgnoreCase);

                    if (vMatch.Success)
                    {
                        string remoteVer = vMatch.Groups[1].Value.TrimStart('v', 'V');
                        res.LatestAppVersion = remoteVer;
                        res.AppDownloadUrl = uMatch.Success ? uMatch.Groups[1].Value : "";
                        Version vRemote, vLocal;
                        if (Version.TryParse(remoteVer, out vRemote) && Version.TryParse(CurrentVersion, out vLocal) && vRemote > vLocal)
                        {
                            res.AppUpdateAvailable = true;
                            res.AppStatus = isEn
                                ? string.Format("New version available: v{0} (Current: v{1})", remoteVer, CurrentVersion)
                                : string.Format("Yeni sürüm mevcut: v{0} (Mevcut: v{1})", remoteVer, CurrentVersion);
                            return;
                        }
                    }
                }
                res.AppStatus = string.Format(isEn ? "PulseGlass v{0} (Up to date)" : "PulseGlass v{0} (Güncel)", CurrentVersion);
            }
            catch
            {
                res.AppStatus = string.Format(isEn ? "PulseGlass v{0} (Could not reach update server)" : "PulseGlass v{0} (Uzak sunucu kontrol edilemedi)", CurrentVersion);
            }
        }

        public string InstallSelectedWindowsUpdates(List<WindowsUpdateItem> selected, Action<string> onProgress, bool isEn = false)
        {
            if (selected == null || selected.Count == 0)
                return isEn ? "No updates selected." : "Kurulacak güncelleme seçilmedi.";
            if (_cachedSearchResult == null)
                return isEn ? "Please check for updates first." : "Önce güncelleme taraması yapın.";

            try
            {
                Type sessionType = Type.GetTypeFromProgID("Microsoft.Update.Session");
                Type collType = Type.GetTypeFromProgID("Microsoft.Update.UpdateColl");
                dynamic session = Activator.CreateInstance(sessionType);
                dynamic updatesToDownload = Activator.CreateInstance(collType);

                foreach (var item in selected)
                {
                    dynamic upd = _cachedSearchResult.Updates.Item(item.Index);
                    if (upd.EulaAccepted == false) upd.AcceptEula();
                    updatesToDownload.Add(upd);
                }

                if (onProgress != null) onProgress(isEn ? "Downloading updates..." : "Güncellemeler indiriliyor...");
                dynamic downloader = session.CreateUpdateDownloader();
                downloader.Updates = updatesToDownload;
                dynamic dlResult = downloader.Download();

                if (onProgress != null) onProgress(isEn ? "Installing updates (Admin rights may be required)..." : "Güncellemeler kuruluyor (Yönetici izni gerekebilir)...");
                dynamic installer = session.CreateUpdateInstaller();
                installer.Updates = updatesToDownload;
                dynamic instResult = installer.Install();

                int code = (int)instResult.ResultCode;
                bool reboot = (bool)instResult.RebootRequired;
                if (code == 2 || code == 3)
                {
                    if (reboot)
                        return isEn
                            ? "Installation complete! Please restart your PC to apply changes."
                            : "Kurulum tamamlandı! Değişiklikler için bilgisayarı yeniden başlatmanız gerekiyor.";
                    return isEn
                        ? "Selected updates installed successfully!"
                        : "Seçili güncellemeler başarıyla kuruldu!";
                }
                return isEn
                    ? "Installation did not complete (Code: " + code + "). Try running as Administrator."
                    : "Kurulum tamamlanamadı (Kod: " + code + "). Yönetici olarak çalıştırmayı deneyin.";
            }
            catch (Exception ex)
            {
                return (isEn ? "Installation error (Admin rights may be required): " : "Kurulum hatası (Yönetici izni gerekebilir): ") + ex.Message;
            }
        }

        public string DownloadAndApplyAppUpdate(string downloadUrl, bool isEn = false)
        {
            if (string.IsNullOrEmpty(downloadUrl))
                return isEn ? "Download URL not found." : "İndirme bağlantısı bulunamadı.";
            try
            {
                ServicePointManager.SecurityProtocol = SecurityProtocolType.Tls12;
                string currentExe = Process.GetCurrentProcess().MainModule.FileName;
                string dir = Path.GetDirectoryName(currentExe);
                string newExe = Path.Combine(dir, "PulseGlassWidget.new.exe");
                using (var wc = new WebClient())
                {
                    wc.Headers.Add("User-Agent", "PulseGlassWidget/" + CurrentVersion);
                    wc.DownloadFile(downloadUrl, newExe);
                }
                if (!HasPeHeader(newExe))
                {
                    try { File.Delete(newExe); } catch { }
                    return isEn
                        ? "The downloaded file is not a valid program. Update cancelled."
                        : "İndirilen dosya geçerli bir program değil. Güncelleme iptal edildi.";
                }
                string cmdPath = Path.Combine(Path.GetTempPath(), "pulseglass_update.cmd");
                string script = string.Format(
                    "@echo off\r\ntimeout /t 2 /nobreak >nul\r\nmove /y \"{0}\" \"{1}\"\r\nstart \"\" \"{1}\"\r\ndel \"%~f0\"",
                    newExe, currentExe);
                File.WriteAllText(cmdPath, script);
                Process.Start(new ProcessStartInfo("cmd.exe", "/c \"" + cmdPath + "\"") { CreateNoWindow = true, UseShellExecute = false });
                return "OK";
            }
            catch (Exception ex)
            {
                return (isEn ? "App update error: " : "Uygulama güncelleme hatası: ") + ex.Message;
            }
        }

        private static bool HasPeHeader(string path)
        {
            using (var fs = File.OpenRead(path))
            {
                return fs.Length > 2 && fs.ReadByte() == 'M' && fs.ReadByte() == 'Z';
            }
        }
    }
}
