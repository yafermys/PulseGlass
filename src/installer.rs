#![allow(dead_code)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Deletes leftover LibreHardwareMonitorLib.dll from v1.3.0 next to the exe (PR #2 by misutesu-desu).
pub fn delete_stale_libraries() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let stale = dir.join("LibreHardwareMonitorLib.dll");
            if stale.exists() {
                let _ = fs::remove_file(stale);
            }
        }
    }
}

/// Checks if PawnIO driver is installed on Windows (PR #2 by misutesu-desu).
pub fn is_pawnio_installed() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        hklm.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO")
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        true // Linux uses native kernel hwmon/rapl, does not need PawnIO
    }
}

/// Opens a URL without inheriting elevated Admin privileges on Windows (PR #2 by misutesu-desu).
pub fn open_url_non_elevated(url: &str) {
    #[cfg(windows)]
    {
        let _ = Command::new("explorer.exe").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = url;
    }
}

/// Opens the folder containing a given executable path.
pub fn open_file_location(path: &str) {
    if path.is_empty() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("explorer.exe")
            .arg(format!("/select,\"{}\"", path))
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = Path::new(path).parent() {
            let _ = Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

pub fn is_system_installed() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;
        let sub = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PulseGlass";
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(sub).is_ok()
            || RegKey::predef(HKEY_CURRENT_USER).open_subkey(sub).is_ok()
    }
    #[cfg(target_os = "linux")]
    {
        let desktop = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from(".local/share"))
            .join("applications")
            .join("pulseglass.desktop");
        desktop.exists()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

pub fn is_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") {
            let v: Result<String, _> = key.get_value("PulseGlass");
            return v.is_ok();
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        let p = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("autostart")
            .join("pulseglass.desktop");
        p.exists()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu
            .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_WRITE)
            .map_err(|e| e.to_string())?;
        if enabled {
            key.set_value("PulseGlass", &format!("\"{}\"", exe.display()))
                .map_err(|e| e.to_string())?;
        } else {
            let _ = key.delete_value("PulseGlass");
            let _ = key.delete_value("PulseGlassWidget");
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("autostart");
        let file = dir.join("pulseglass.desktop");
        if enabled {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let content = format!(
                "[Desktop Entry]\nType=Application\nName=PulseGlass Hardware Monitor\nExec=\"{}\"\nIcon=pulseglass\nX-GNOME-Autostart-enabled=true\n",
                exe.display()
            );
            fs::write(&file, content).map_err(|e| e.to_string())?;
        } else if file.exists() {
            let _ = fs::remove_file(&file);
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (enabled, exe);
        Ok(())
    }
}

/// Installs a source binary (or the current executable) into the OS Apps list + Start Menu / Desktop.
pub fn install_binary_to_system(
    source_exe: &Path,
    create_desktop_shortcut: bool,
    en: bool,
) -> Result<String, String> {
    #[cfg(windows)]
    {
        install_windows(source_exe, create_desktop_shortcut, en)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = create_desktop_shortcut;
        install_linux(source_exe, en)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (source_exe, create_desktop_shortcut, en);
        Err("Unsupported OS".to_string())
    }
}

pub fn install_current_to_system(create_desktop_shortcut: bool, en: bool) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    install_binary_to_system(&exe, create_desktop_shortcut, en)
}

pub fn uninstall_from_system(en: bool) -> Result<String, String> {
    #[cfg(windows)]
    {
        uninstall_windows(en)
    }
    #[cfg(target_os = "linux")]
    {
        uninstall_linux(en)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = en;
        Err("Unsupported OS".to_string())
    }
}

#[cfg(windows)]
fn is_process_elevated() -> bool {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_WRITE};
    use winreg::RegKey;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", KEY_WRITE)
        .is_ok()
}

#[cfg(windows)]
fn create_windows_shortcut(lnk_path: &Path, target: &Path, workdir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let ps = format!(
        "$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut('{}'); $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{},0'; $s.Description = 'PulseGlass Hardware Monitor'; $s.Save()",
        lnk_path.display().to_string().replace('\'', "''"),
        target.display().to_string().replace('\'', "''"),
        workdir.display().to_string().replace('\'', "''"),
        target.display().to_string().replace('\'', "''"),
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(windows)]
fn install_windows(source_exe: &Path, create_desktop: bool, en: bool) -> Result<String, String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let admin = is_process_elevated();
    let install_dir = if admin {
        PathBuf::from(r"C:\Program Files\PulseGlass")
    } else {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Programs")
            .join("PulseGlass")
    };

    fs::create_dir_all(&install_dir).map_err(|e| format!("Dir error: {}", e))?;
    let target_exe = install_dir.join("PulseGlass.exe");

    if source_exe.canonicalize().ok() != target_exe.canonicalize().ok() {
        fs::copy(source_exe, &target_exe).map_err(|e| format!("Copy error: {}", e))?;
    }

    let size_kb = fs::metadata(&target_exe)
        .map(|m| (m.len() / 1024).max(1) as u32)
        .unwrap_or(5000);

    // Register in Windows Installed Apps (Ayarlar -> Uygulamalar)
    let root = if admin {
        RegKey::predef(HKEY_LOCAL_MACHINE)
    } else {
        RegKey::predef(HKEY_CURRENT_USER)
    };
    let (key, _) = root
        .create_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PulseGlass")
        .map_err(|e| format!("Registry error: {}", e))?;

    let exe_str = target_exe.display().to_string();
    let dir_str = install_dir.display().to_string();
    let _ = key.set_value("DisplayName", &"PulseGlass Hardware Monitor");
    let _ = key.set_value("DisplayVersion", &APP_VERSION);
    let _ = key.set_value("Publisher", &"yafermys");
    let _ = key.set_value("DisplayIcon", &format!("{},0", exe_str));
    let _ = key.set_value("InstallLocation", &dir_str);
    let _ = key.set_value("UninstallString", &format!("\"{}\" --uninstall", exe_str));
    let _ = key.set_value("QuietUninstallString", &format!("\"{}\" --uninstall", exe_str));
    let _ = key.set_value("URLInfoAbout", &"https://github.com/yafermys/PulseGlass");
    let _ = key.set_value("EstimatedSize", &size_kb);
    let _ = key.set_value("NoModify", &1u32);
    let _ = key.set_value("NoRepair", &1u32);

    // Start Menu shortcut
    let start_menu = if admin {
        PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs")
    } else {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(r"Microsoft\Windows\Start Menu\Programs")
    };
    let _ = fs::create_dir_all(&start_menu);
    create_windows_shortcut(&start_menu.join("PulseGlass.lnk"), &target_exe, &install_dir);

    if create_desktop {
        if let Some(desktop) = dirs::desktop_dir() {
            create_windows_shortcut(&desktop.join("PulseGlass.lnk"), &target_exe, &install_dir);
        }
    }

    Ok(if en {
        format!(
            "Installed to {} and registered in Windows Installed Apps & Start Menu.",
            install_dir.display()
        )
    } else {
        format!(
            "{} konumuna kuruldu, Windows Uygulamalar listesine ve Başlat menüsüne eklendi.",
            install_dir.display()
        )
    })
}

#[cfg(windows)]
fn uninstall_windows(en: bool) -> Result<String, String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let sub = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PulseGlass";
    let mut install_dir: Option<PathBuf> = None;

    for root in [
        RegKey::predef(HKEY_LOCAL_MACHINE),
        RegKey::predef(HKEY_CURRENT_USER),
    ] {
        if let Ok(key) = root.open_subkey(sub) {
            if let Ok(loc) = key.get_value::<String, _>("InstallLocation") {
                install_dir = Some(PathBuf::from(loc));
            }
        }
        let _ = root.delete_subkey_all(sub);
    }

    let _ = set_autostart(false);

    // Remove Start Menu & Desktop shortcuts
    let candidates = [
        PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\PulseGlass.lnk"),
        dirs::data_dir()
            .unwrap_or_default()
            .join(r"Microsoft\Windows\Start Menu\Programs\PulseGlass.lnk"),
        dirs::desktop_dir()
            .unwrap_or_default()
            .join("PulseGlass.lnk"),
    ];
    for c in &candidates {
        if c.exists() {
            let _ = fs::remove_file(c);
        }
    }

    // Schedule folder cleanup after process exits (safe even when uninstalling itself)
    if let Some(dir) = install_dir {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let cmd_path = std::env::temp_dir().join("pulseglass_uninstall.cmd");
        let script = format!(
            "@echo off\r\ntimeout /t 2 /nobreak >nul\r\nrmdir /s /q \"{}\"\r\ndel \"%~f0\"\r\n",
            dir.display()
        );
        if fs::write(&cmd_path, script).is_ok() {
            let _ = Command::new("cmd.exe")
                .args(["/C", &cmd_path.display().to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
    }

    Ok(if en {
        "PulseGlass has been uninstalled from Windows Apps & Start Menu.".to_string()
    } else {
        "PulseGlass, Windows Uygulamalar listesinden ve Başlat menüsünden kaldırıldı.".to_string()
    })
}

#[cfg(target_os = "linux")]
fn install_linux(source_exe: &Path, en: bool) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let bin_dir = home.join(".local/bin");
    let app_dir = home.join(".local/share/applications");
    let icon_dir = home.join(".local/share/icons/hicolor/256x256/apps");

    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&icon_dir).map_err(|e| e.to_string())?;

    let target_bin = bin_dir.join("pulseglass");
    if source_exe.canonicalize().ok() != target_bin.canonicalize().ok() {
        fs::copy(source_exe, &target_bin).map_err(|e| e.to_string())?;
        let mut perms = fs::metadata(&target_bin)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&target_bin, perms).map_err(|e| e.to_string())?;
    }

    let icon_bytes = include_bytes!("../Assets/app.png");
    let _ = fs::write(icon_dir.join("pulseglass.png"), icon_bytes);

    let desktop_entry = format!(
        "[Desktop Entry]\nType=Application\nName=PulseGlass Hardware Monitor\nComment=Lightweight glassmorphic hardware monitor & OSD overlay\nExec=\"{}\"\nIcon=pulseglass\nCategories=System;Monitor;Utility;\nTerminal=false\n",
        target_bin.display()
    );
    fs::write(app_dir.join("pulseglass.desktop"), desktop_entry).map_err(|e| e.to_string())?;

    Ok(if en {
        format!(
            "Installed to {} and added desktop menu entry.",
            target_bin.display()
        )
    } else {
        format!(
            "{} konumuna kuruldu ve uygulama menüsüne eklendi.",
            target_bin.display()
        )
    })
}

#[cfg(target_os = "linux")]
fn uninstall_linux(en: bool) -> Result<String, String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let _ = fs::remove_file(home.join(".local/bin/pulseglass"));
    let _ = fs::remove_file(home.join(".local/share/applications/pulseglass.desktop"));
    let _ = fs::remove_file(home.join(".local/share/icons/hicolor/256x256/apps/pulseglass.png"));
    let _ = set_autostart(false);
    Ok(if en {
        "PulseGlass has been uninstalled.".to_string()
    } else {
        "PulseGlass sistemden kaldırıldı.".to_string()
    })
}
