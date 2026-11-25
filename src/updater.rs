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

/// Checks both PulseGlass GitHub Releases (with PR #1 strict binary asset selection)
/// and OS updates (Windows Important/Optional or Linux package manager).
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
    let resp = ureq::get(GITHUB_LATEST_API)
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

    let resp = ureq::get(download_url)
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

fn check_os_updates(res: &mut UpdateCheckResult, en: bool) {
    #[cfg(windows)]
    {
        let ps = r#"
$ErrorActionPreference='Stop'
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
$out | ConvertTo-Json -Compress
"#;
        match Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", ps])
            .output()
        {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if text.is_empty() || text == "null" {
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
        let filter = if include_optional {
            "$true"
        } else {
            "((-not $u.AutoSelectOnWebSites) -or $u.BrowseOnly) -eq $false"
        };
        let ps = format!(
            r#"
$ErrorActionPreference='Stop'
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
    fn pr1_header_validation_rejects_json() {
        let tmp = std::env::temp_dir().join("pg_test_json_fake.exe");
        fs::write(&tmp, b"{\"url\":\"https://api.github.com/\"}").unwrap();
        assert!(!has_valid_executable_header(&tmp, true));
        fs::write(&tmp, b"MZ\x90\x00").unwrap();
        assert!(has_valid_executable_header(&tmp, true));
        let _ = fs::remove_file(&tmp);
    }
}
