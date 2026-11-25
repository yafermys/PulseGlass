#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkeys;
mod installer;
mod models;
mod settings;
mod telemetry;
mod ui;
mod updater;

use std::sync::Arc;

fn load_app_icon() -> Option<Arc<egui::IconData>> {
    let bytes = include_bytes!("../Assets/app.png");
    let img = image::load_from_memory(bytes)
        .ok()?
        .resize(64, 64, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let (width, height) = img.dimensions();
    Some(Arc::new(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }))
}

#[cfg(windows)]
fn acquire_single_instance_mutex() -> Option<isize> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = OsStr::new("PulseGlass_SingleInstance_Mutex_V2")
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if h.is_null() {
            return None;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(h as isize)
    }
}

#[cfg(windows)]
fn show_windows_message(title: &str, msg: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let t: Vec<u16> = OsStr::new(title).encode_wide().chain(Some(0)).collect();
    let m: Vec<u16> = OsStr::new(msg).encode_wide().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // CLI install / uninstall support (also invoked by Windows Installed Apps -> Uninstall)
    if args.iter().any(|a| a.eq_ignore_ascii_case("--install")) {
        let res = installer::install_current_to_system(true, false);
        #[cfg(windows)]
        match res {
            Ok(msg) => show_windows_message("PulseGlass Kurulum", &msg),
            Err(err) => show_windows_message("PulseGlass Hata", &err),
        }
        #[cfg(not(windows))]
        match res {
            Ok(msg) => println!("{}", msg),
            Err(err) => eprintln!("{}", err),
        }
        return Ok(());
    }

    if args.iter().any(|a| a.eq_ignore_ascii_case("--uninstall")) {
        let res = installer::uninstall_from_system(false);
        #[cfg(windows)]
        match res {
            Ok(msg) => show_windows_message("PulseGlass Kaldırma", &msg),
            Err(err) => show_windows_message("PulseGlass Hata", &err),
        }
        #[cfg(not(windows))]
        match res {
            Ok(msg) => println!("{}", msg),
            Err(err) => eprintln!("{}", err),
        }
        return Ok(());
    }

    // PR #2 (misutesu-desu): Remove stale LibreHardwareMonitorLib.dll from v1.3.0 if present
    installer::delete_stale_libraries();

    #[cfg(windows)]
    let _mutex_guard = match acquire_single_instance_mutex() {
        Some(h) => h,
        None => {
            show_windows_message(
                "PulseGlass",
                "PulseGlass zaten çalışıyor! (Ctrl+Enter ile Özet Modunu açıp kapatabilirsiniz.)",
            );
            return Ok(());
        }
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PulseGlass Hardware Monitor")
        .with_inner_size([480.0, 294.0])
        .with_min_inner_size([120.0, 24.0])
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(true);

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "PulseGlass Hardware Monitor",
        options,
        Box::new(|cc| Ok(Box::new(ui::PulseGlassApp::new(cc)))),
    )
}
