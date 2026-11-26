use crate::installer::APP_VERSION;
use crate::models::{OsUpdateEntry, UpdateCheckResult};
use chrono::Local;
use serde::Deserialize;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::process::Command;

const GITHUB_LATEST_API: &str = "https://api.github.com/repos/yafermys/PulseGlass/releases/latest";

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: Option<String>,
    body: Option<String>,
    assets: Option<Vec<GitHubAsset>>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

fn build_http_agent() -> Result<ureq::Agent, String> {
    #[cfg(windows)]
    {
        let tls = ureq::native_tls::TlsConnector::new()
            .map_err(|e| format!("TLS başlatılamadı: {}", e))?;
        Ok(ureq::AgentBuilder::new()
            .tls_connector(std::sync::Arc::new(tls))
            .build())
    }
    #[cfg(not(windows))]
    {
        Ok(ureq::AgentBuilder::new().build())
    }
}

/// Checks only PulseGlass GitHub Releases (fast, <1s) so UI updates immediately before slow OS update scan.
pub fn check_app_update_only(en: bool) -> UpdateCheckResult {
    let mut res = UpdateCheckResult {
        checked_at: Local::now().format("%H:%M:%S").to_string(),
        os_status_msg: if en {
            "Checking Windows/OS updates...".to_string()
        } else {
            "İşletim sistemi güncellemeleri denetleniyor...".to_string()
        },
        ..Default::default()
    };
    check_app_update(&mut res, en);
    res
}

/// Checks OS updates (Windows Important/Optional or Linux package manager) into an existing result.
pub fn populate_os_updates(res: &mut UpdateCheckResult, en: bool) {
    check_os_updates(res, en);
    res.checked_at = Local::now().format("%H:%M:%S").to_string();
}

/// Checks both PulseGlass GitHub Releases (with PR #1 strict binary asset selection)
/// and OS updates (Windows Important/Optional or Linux package manager).
#[allow(dead_code)]
pub fn check_all_updates(en: bool) -> UpdateCheckResult {
    let mut res = UpdateCheckResult {
        checked_at: Local::now().format("%H:%M:%S").to_string(),
        ..Default::default()
    };

    check_app_update(&mut res, en);
    check_os_updates(&mut res, en);
    res
}

/// PR #1 (misutesu-desu): Strict asset selection — NEVER falls back to release API JSON URL!
pub fn select_release_asset_url(assets: &[(&str, &str)], is_windows: bool) -> Option<String> {
    if is_windows {
        // Prefer exact portable executable name, never Setup or non-exe
        for preferred in ["pulseglass.exe", "pulseglasswidget.exe"] {
            if let Some((_, url)) = assets
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(preferred))
            {
                return Some((*url).to_string());
            }
        }
        // Fallback to any non-setup .exe
        if let Some((_, url)) = assets.iter().find(|(name, _)| {
            let l = name.to_ascii_lowercase();
            l.ends_with(".exe") && !l.contains("setup")
        }) {
            return Some((*url).to_string());
        }
        None
    } else {
        // Linux binary asset
        if let Some((_, url)) = assets.iter().find(|(name, _)| {
            let l = name.to_ascii_lowercase();
            l == "pulseglass-linux-x86_64" || l == "pulseglass-linux" || l == "pulseglass"
        }) {
            return Some((*url).to_string());
        }
        None
    }
}

/// PR #1 (misutesu-desu): Validates PE ('MZ') header on Windows or ELF ('\x7fELF') on Linux.
pub fn has_valid_executable_header(path: &Path, is_windows: bool) -> bool {
    let mut buf = [0u8; 4];
    if let Ok(mut f) = File::open(path) {
        if f.read_exact(&mut buf).is_ok() {
            if is_windows {
                return buf[0] == b'M' && buf[1] == b'Z';
            } else {
                return buf == [0x7F, b'E', b'L', b'F'];
            }
        }
    }
    false
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let clean = v.trim().trim_start_matches(['v', 'V']);
    let mut parts = clean.split('.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(0)
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn check_app_update(res: &mut UpdateCheckResult, en: bool) {
    let agent = match build_http_agent() {
        Ok(a) => a,
        Err(e) => {
            res.app_status_msg = if en {
                format!("Update check failed: {}", e)
            } else {
                format!("Güncelleme denetimi başarısız: {}", e)
            };
            return;
        }
    };

    let resp = agent
        .get(GITHUB_LATEST_API)
        .set("User-Agent", &format!("PulseGlass/{}", APP_VERSION))
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(10))
        .call();

    match resp {
        Ok(r) => match r.into_json::<GitHubRelease>() {
            Ok(rel) => {
                let latest = rel.tag_name.unwrap_or_default();
                res.app_latest_version = latest.trim_start_matches(['v', 'V']).to_string();
                res.app_release_notes = rel.body.unwrap_or_default();

                let assets_vec = rel.assets.unwrap_or_default();
                let asset_refs: Vec<(&str, &str)> = assets_vec
                    .iter()
                    .map(|a| (a.name.as_str(), a.browser_download_url.as_str()))
                    .collect();

                let is_win = cfg!(windows);
                if let Some(url) = select_release_asset_url(&asset_refs, is_win) {
                    res.app_download_url = url;
                }

                if parse_version(&latest) > parse_version(APP_VERSION) {
                    res.app_update_available = true;
                    res.app_status_msg = if en {
                        format!(
                            "New version available: v{} (current: v{})",
                            res.app_latest_version, APP_VERSION
                        )
                    } else {
                        format!(
                            "Yeni sürüm mevcut: v{} (mevcut: v{})",
                            res.app_latest_version, APP_VERSION
                        )
                    };
                } else {
                    res.app_status_msg = if en {
                        format!("PulseGlass is up to date (v{}).", APP_VERSION)
                    } else {
                        format!("PulseGlass güncel (v{}).", APP_VERSION)
                    };
                }
            }
            Err(e) => {
                res.app_status_msg = if en {
                    format!("Release parse error: {}", e)
                } else {
                    format!("Sürüm bilgisi okunamadı: {}", e)
                };
            }
        },
        Err(e) => {
            res.app_status_msg = if en {
                format!("Update check failed: {}", e)
            } else {
                format!("Güncelleme denetimi başarısız: {}", e)
            };
        }
    }
}

pub fn download_and_apply_app_update(download_url: &str, en: bool) -> Result<String, String> {
    if download_url.is_empty() {
        return Err(if en {
            "No valid executable asset found in this release.".to_string()
        } else {
            "Bu sürümde geçerli bir uygulama dosyası bulunamadı.".to_string()
        });
    }

    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new_exe = std::env::temp_dir().join(if cfg!(windows) {
        "PulseGlass.new.exe"
    } else {
        "pulseglass.new"
    });

    let agent = build_http_agent()?;
    let resp = agent
        .get(download_url)
        .set("User-Agent", &format!("PulseGlass/{}", APP_VERSION))
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| format!("Download failed: {}", e))?;

    let mut reader = resp.into_reader();
    let mut out = File::create(&new_exe).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
    drop(out);

    // PR #1 magic header validation (MZ on Windows, ELF on Linux)
    if !has_valid_executable_header(&new_exe, cfg!(windows)) {
        let _ = fs::remove_file(&new_exe);
        return Err(if en {
            "The downloaded file is not a valid program. Update cancelled.".to_string()
        } else {
            "İndirilen dosya geçerli bir program değil. Güncelleme iptal edildi.".to_string()
        });
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let cmd_path = std::env::temp_dir().join("pulseglass_update.cmd");
        let script = format!(
            "@echo off\r\ntimeout /t 2 /nobreak >nul\r\nmove /y \"{}\" \"{}\"\r\nstart \"\" \"{}\"\r\ndel \"%~f0\"\r\n",
            new_exe.display(),
            current_exe.display(),
            current_exe.display()
        );
        fs::write(&cmd_path, script).map_err(|e| e.to_string())?;
        Command::new("cmd.exe")
            .args(["/C", &cmd_path.display().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&new_exe)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&new_exe, perms).map_err(|e| e.to_string())?;
        fs::rename(&new_exe, &current_exe).map_err(|e| e.to_string())?;
        let _ = Command::new(&current_exe).spawn();
    }

    Ok(if en {
        "Update downloaded! Restarting PulseGlass...".to_string()
    } else {
        "Güncelleme indirildi! PulseGlass yeniden başlatılıyor...".to_string()
    })
}

fn decode_base64_utf8(input: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for b in input.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    String::from_utf8(bytes).ok()
}

fn check_os_updates(res: &mut UpdateCheckResult, en: bool) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let ps = r#"
$ErrorActionPreference='Stop'
$OutputEncoding = [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$s = New-Object -ComObject Microsoft.Update.Session
$sr = $s.CreateUpdateSearcher()
$r = $sr.Search("IsInstalled=0 and Type='Software' and IsHidden=0")
$out = @()
foreach ($u in $r.Updates) {
    $opt = (-not $u.AutoSelectOnWebSites) -or ($u.BrowseOnly)
    $cat = if ($u.Categories.Count -gt 0) { $u.Categories.Item(0).Name } else { 'Windows' }
    $out += [PSCustomObject]@{
        id = $u.Identity.UpdateID
        title = $u.Title
        size_mb = [math]::Round($u.MaxDownloadSize / 1MB, 1)
        is_optional = [bool]$opt
        category = $cat
    }
}
$json = if ($out.Count -eq 0) { '[]' } else { ($out | ConvertTo-Json -Compress) }
[Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($json))
"#;
        match Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", ps])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(out) if out.status.success() => {
                let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let text = decode_base64_utf8(&raw).unwrap_or(raw).trim().to_string();
                if text.is_empty() || text == "null" || text == "[]" {
                    res.os_status_msg = if en {
                        "Windows is up to date.".to_string()
                    } else {
                        "Windows güncel.".to_string()
                    };
                    return;
                }
                let parsed: Vec<OsUpdateEntry> = if text.starts_with('[') {
                    serde_json::from_str(&text).unwrap_or_default()
                } else {
                    serde_json::from_str::<OsUpdateEntry>(&text)
                        .map(|u| vec![u])
                        .unwrap_or_default()
                };
                for u in parsed {
                    if u.is_optional {
                        res.optional_updates.push(u);
                    } else {
                        res.important_updates.push(u);
                    }
                }
                res.os_status_msg = if en {
                    format!(
                        "{} important, {} optional Windows updates found.",
                        res.important_updates.len(),
                        res.optional_updates.len()
                    )
                } else {
                    format!(
                        "{} önemli, {} isteğe bağlı Windows güncellemesi bulundu.",
                        res.important_updates.len(),
                        res.optional_updates.len()
                    )
                };
            }
            _ => {
                res.os_status_msg = if en {
                    "Could not query Windows Update Agent.".to_string()
                } else {
                    "Windows Update servisi sorgulanamadı.".to_string()
                };
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try apt (Debian/Ubuntu), dnf (Fedora), pacman (Arch) non-intrusively
        if let Ok(out) = Command::new("apt").args(["list", "--upgradable"]).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                for line in s.lines().skip(1) {
                    let pkg = line.split('/').next().unwrap_or("").trim();
                    if !pkg.is_empty() {
                        res.important_updates.push(OsUpdateEntry {
                            id: pkg.to_string(),
                            title: line.to_string(),
                            size_mb: 0.0,
                            is_optional: false,
                            category: "apt".to_string(),
                        });
                    }
                }
            }
        } else if let Ok(out) = Command::new("checkupdates").output() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                if !line.trim().is_empty() {
                    res.important_updates.push(OsUpdateEntry {
                        id: line.split_whitespace().next().unwrap_or("").to_string(),
                        title: line.to_string(),
                        size_mb: 0.0,
                        is_optional: false,
                        category: "pacman".to_string(),
                    });
                }
            }
        }
        res.os_status_msg = if res.important_updates.is_empty() {
            if en {
                "Linux packages are up to date.".to_string()
            } else {
                "Linux paketleri güncel.".to_string()
            }
        } else if en {
            format!("{} package updates available.", res.important_updates.len())
        } else {
            format!("{} paket güncellemesi mevcut.", res.important_updates.len())
        };
    }
}

pub fn install_windows_updates(include_optional: bool, en: bool) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let filter = if include_optional {
            "$true"
        } else {
            "((-not $u.AutoSelectOnWebSites) -or $u.BrowseOnly) -eq $false"
        };
        let ps = format!(
            r#"
$ErrorActionPreference='Stop'
$OutputEncoding = [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$s = New-Object -ComObject Microsoft.Update.Session
$sr = $s.CreateUpdateSearcher()
$r = $sr.Search("IsInstalled=0 and Type='Software' and IsHidden=0")
$col = New-Object -ComObject Microsoft.Update.UpdateColl
foreach ($u in $r.Updates) {{ if ({}) {{ [void]$col.Add($u) }} }}
if ($col.Count -eq 0) {{ Write-Output 'NONE'; exit 0 }}
$dl = $s.CreateUpdateDownloader(); $dl.Updates = $col; [void]$dl.Download()
$inst = $s.CreateUpdateInstaller(); $inst.Updates = $col; $ir = $inst.Install()
Write-Output ("OK:" + $col.Count + ":Reboot=" + $ir.RebootRequired)
"#,
            filter
        );
        match Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(out) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s == "NONE" {
                    if en {
                        "No matching updates to install.".to_string()
                    } else {
                        "Kurulacak eşleşen güncelleme yok.".to_string()
                    }
                } else if en {
                    format!("Windows Updates installed ({}).", s)
                } else {
                    format!("Windows güncellemeleri kuruldu ({}).", s)
                }
            }
            Ok(out) => format!(
                "{}: {}",
                if en { "Install error" } else { "Kurulum hatası" },
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => e.to_string(),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = include_optional;
        if en {
            "Run your distro package manager (e.g. sudo apt upgrade / sudo pacman -Syu) to install OS updates.".to_string()
        } else {
            "Sistem güncellemelerini kurmak için dağıtım paket yöneticisini (örn. sudo apt upgrade / sudo pacman -Syu) çalıştırın.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr1_updater_never_picks_json_or_zip() {
        let zip_only = vec![("PulseGlass-v1.3.0.zip", "https://example.com/a.zip")];
        assert_eq!(select_release_asset_url(&zip_only, true), None);

        let with_exe = vec![
            ("PulseGlass-Setup-v2.0.0.exe", "https://example.com/setup.exe"),
            ("PulseGlass.exe", "https://example.com/PulseGlass.exe"),
        ];
        assert_eq!(
            select_release_asset_url(&with_exe, true).as_deref(),
            Some("https://example.com/PulseGlass.exe")
        );
    }

    #[test]
    fn base64_utf8_preserves_turkish_update_titles() {
        // UTF-8 Base64 for: "Windows 11 Güvenlik Güncelleştirmesi — İsteğe Bağlı (ÖĞÜŞÇİı)"
        let original = "Windows 11 Güvenlik Güncelleştirmesi — İsteğe Bağlı (ÖĞÜŞÇİı)";
        let b64 = "V2luZG93cyAxMSBHw7x2ZW5saWsgR8O8bmNlbGxlxZ90aXJtZXNpIOKAlCDEsHN0ZcSfZSBCYcSfbMSxICjDlsSew5zFnsOHxLDEsSk=";
        let decoded = decode_base64_utf8(b64).expect("valid utf8");
        assert_eq!(decoded, original);
    }

    #[test]
    fn pr1_header_validation_rejects_json() {
        let tmp = std::env::temp_dir().join("pg_test_json_fake.exe");
        fs::write(&tmp, b"{\"url\":\"https://api.github.com/\"}").unwrap();
        assert!(!has_valid_executable_header(&tmp, true));
        fs::write(&tmp, b"MZ\x90\x00").unwrap();
        assert!(has_valid_executable_header(&tmp, true));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn live_github_update_check_uses_native_tls_successfully() {
        let res = check_app_update_only(false);
        println!(
            "UPDATER_LIVE: latest='{}' url='{}' msg='{}'",
            res.app_latest_version, res.app_download_url, res.app_status_msg
        );
        assert!(
            !res.app_latest_version.is_empty(),
            "GitHub API check failed: {}",
            res.app_status_msg
        );
        assert!(
            !res.app_status_msg.contains("başarısız"),
            "Unexpected failure: {}",
            res.app_status_msg
        );
    }
}
