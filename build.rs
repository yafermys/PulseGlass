fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        use embed_manifest::manifest::{DpiAwareness, ExecutionLevel, SupportedOS};
        use embed_manifest::{embed_manifest, new_manifest};

        let level = if cfg!(feature = "no-admin-manifest") {
            ExecutionLevel::AsInvoker
        } else {
            ExecutionLevel::RequireAdministrator
        };

        let manifest = new_manifest("PulseGlass.App")
            .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10)
            .dpi_awareness(DpiAwareness::PerMonitorV2)
            .requested_execution_level(level);

        embed_manifest(manifest).expect("unable to embed Windows manifest");
        println!("cargo:rerun-if-changed=build.rs");
    }
}
