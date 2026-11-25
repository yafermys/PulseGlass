fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Assets/app.ico");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    use embed_manifest::manifest::{DpiAwareness, ExecutionLevel, SupportedOS};
    use embed_manifest::new_manifest;

    let level = if cfg!(feature = "no-admin-manifest") {
        ExecutionLevel::AsInvoker
    } else {
        ExecutionLevel::RequireAdministrator
    };

    let manifest = new_manifest("PulseGlass.App")
        .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10)
        .dpi_awareness(DpiAwareness::PerMonitorV2)
        .requested_execution_level(level);
    let manifest_xml = manifest.to_string();

    let ico_bytes = std::fs::read("Assets/app.ico").expect("unable to read Assets/app.ico");
    let target = std::env::var("TARGET").unwrap_or_default();
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));

    let arch = if target.starts_with("i686-") {
        win_rsrc::MachineType::I386
    } else if target.starts_with("aarch64-") {
        win_rsrc::MachineType::Aarch64
    } else {
        win_rsrc::MachineType::X86_64
    };

    let obj_bytes = win_rsrc::build_rsrc_coff(&ico_bytes, manifest_xml.as_bytes(), arch)
        .expect("unable to build Windows resource COFF object");

    let is_msvc = target.ends_with("-windows-msvc");
    let obj_path = out_dir.join(if is_msvc {
        "pulseglass-rsrc.obj"
    } else {
        "pulseglass-rsrc.o"
    });
    std::fs::write(&obj_path, obj_bytes).expect("unable to write Windows resource object");

    if is_msvc {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    }
    println!("cargo:rustc-link-arg-bins={}", obj_path.display());
}

mod win_rsrc {
    use std::io::{self, Cursor, Seek, SeekFrom, Write};
    use std::time::SystemTime;

    #[derive(Clone, Copy)]
    pub enum MachineType {
        I386,
        X86_64,
        Aarch64,
    }

    impl MachineType {
        fn machine(self) -> u16 {
            match self {
                Self::I386 => 0x014c,
                Self::X86_64 => 0x8664,
                Self::Aarch64 => 0xaa64,
            }
        }

        fn relocation_type(self) -> u16 {
            match self {
                Self::I386 => 7,
                Self::X86_64 => 3,
                Self::Aarch64 => 2,
            }
        }
    }

    struct ParsedIco<'a> {
        grp_icon_dir: Vec<u8>,
        images: Vec<&'a [u8]>,
    }

    fn parse_ico(ico: &[u8]) -> io::Result<ParsedIco<'_>> {
        if ico.len() < 6 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ICO too small"));
        }
        let ico_type = u16::from_le_bytes([ico[2], ico[3]]);
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        if ico_type != 1 || count == 0 || ico.len() < 6 + count * 16 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid ICO header"));
        }

        let mut grp = Vec::with_capacity(6 + count * 14);
        grp.extend_from_slice(&0u16.to_le_bytes()); // idReserved
        grp.extend_from_slice(&1u16.to_le_bytes()); // idType = ICON
        grp.extend_from_slice(&(count as u16).to_le_bytes()); // idCount

        let mut images = Vec::with_capacity(count);
        for i in 0..count {
            let off = 6 + i * 16;
            let size = u32::from_le_bytes(ico[off + 8..off + 12].try_into().unwrap()) as usize;
            let img_off = u32::from_le_bytes(ico[off + 12..off + 16].try_into().unwrap()) as usize;
            if img_off + size > ico.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "ICO image out of bounds"));
            }
            // GRPICONDIRENTRY (14 bytes): first 12 bytes copied directly + 2-byte resource ID (1-based)
            grp.extend_from_slice(&ico[off..off + 12]);
            grp.extend_from_slice(&((i + 1) as u16).to_le_bytes());
            images.push(&ico[img_off..img_off + size]);
        }

        Ok(ParsedIco {
            grp_icon_dir: grp,
            images,
        })
    }

    fn dir_table(num_id_entries: u16) -> [u8; 16] {
        let mut t = [0u8; 16];
        t[14..16].copy_from_slice(&num_id_entries.to_le_bytes());
        t
    }

    fn dir_entry(id: u32, offset: u32, is_subdir: bool) -> [u8; 8] {
        let mut e = [0u8; 8];
        e[0..4].copy_from_slice(&id.to_le_bytes());
        let v = (offset & 0x7FFF_FFFF) | if is_subdir { 0x8000_0000 } else { 0 };
        e[4..8].copy_from_slice(&v.to_le_bytes());
        e
    }

    fn data_entry(rva: u32, size: u32) -> [u8; 16] {
        let mut e = [0u8; 16];
        e[0..4].copy_from_slice(&rva.to_le_bytes());
        e[4..8].copy_from_slice(&size.to_le_bytes());
        e
    }

    fn align8(v: u32) -> u32 {
        (v + 7) & !7
    }

    /// Builds a single COFF `.rsrc` object containing:
    /// - RT_ICON (3), IDs 1..=N (all resolutions from Assets/app.ico)
    /// - RT_GROUP_ICON (14), ID 1 (default application executable icon)
    /// - RT_MANIFEST (24), ID 1 (Windows UAC + DPI manifest)
    pub fn build_rsrc_coff(ico: &[u8], manifest: &[u8], arch: MachineType) -> io::Result<Vec<u8>> {
        let parsed = parse_ico(ico)?;
        let n_icons = parsed.images.len();
        let total_leaves = n_icons + 2; // N icons + 1 group_icon + 1 manifest

        // Precompute exact section-relative offsets for every directory table, data entry, and payload.
        // Level 1 (Root): 16 + 3 * 8 = 40
        let root_off: u32 = 0;
        let type_icon_dir_off: u32 = root_off + 16 + 3 * 8;
        let type_grp_dir_off: u32 = type_icon_dir_off + 16 + (n_icons as u32) * 8;
        let type_man_dir_off: u32 = type_grp_dir_off + 16 + 8;

        // Level 3 (Language tables: one 24-byte table per leaf)
        let lang_base_off: u32 = type_man_dir_off + 16 + 8;
        let data_entries_base_off: u32 = lang_base_off + (total_leaves as u32) * 24;
        let payloads_base_off: u32 = align8(data_entries_base_off + (total_leaves as u32) * 16);

        let mut payloads: Vec<&[u8]> = Vec::with_capacity(total_leaves);
        for img in &parsed.images {
            payloads.push(img);
        }
        payloads.push(&parsed.grp_icon_dir);
        payloads.push(manifest);

        let mut payload_offsets = Vec::with_capacity(total_leaves);
        let mut cur = payloads_base_off;
        for p in &payloads {
            payload_offsets.push(cur);
            cur = align8(cur + p.len() as u32);
        }
        let rsrc_size = cur;

        let mut rsrc = Vec::with_capacity(rsrc_size as usize);

        // 1. Root directory (Types must be sorted ascending: 3, 14, 24)
        rsrc.extend_from_slice(&dir_table(3));
        rsrc.extend_from_slice(&dir_entry(3, type_icon_dir_off, true)); // RT_ICON
        rsrc.extend_from_slice(&dir_entry(14, type_grp_dir_off, true)); // RT_GROUP_ICON
        rsrc.extend_from_slice(&dir_entry(24, type_man_dir_off, true)); // RT_MANIFEST
        debug_assert_eq!(rsrc.len() as u32, type_icon_dir_off);

        // 2. Type -> Name directories
        rsrc.extend_from_slice(&dir_table(n_icons as u16));
        for i in 0..n_icons {
            rsrc.extend_from_slice(&dir_entry(
                (i + 1) as u32,
                lang_base_off + (i as u32) * 24,
                true,
            ));
        }
        debug_assert_eq!(rsrc.len() as u32, type_grp_dir_off);

        rsrc.extend_from_slice(&dir_table(1));
        rsrc.extend_from_slice(&dir_entry(
            1,
            lang_base_off + (n_icons as u32) * 24,
            true,
        ));
        debug_assert_eq!(rsrc.len() as u32, type_man_dir_off);

        rsrc.extend_from_slice(&dir_table(1));
        rsrc.extend_from_slice(&dir_entry(
            1,
            lang_base_off + ((n_icons + 1) as u32) * 24,
            true,
        ));
        debug_assert_eq!(rsrc.len() as u32, lang_base_off);

        // 3. Language directories (Lang ID 1033 = en-US)
        for i in 0..total_leaves {
            rsrc.extend_from_slice(&dir_table(1));
            rsrc.extend_from_slice(&dir_entry(
                1033,
                data_entries_base_off + (i as u32) * 16,
                false,
            ));
        }
        debug_assert_eq!(rsrc.len() as u32, data_entries_base_off);

        // 4. Data entries + relocation addresses
        let mut reloc_addrs = Vec::with_capacity(total_leaves);
        for i in 0..total_leaves {
            let entry_off = rsrc.len() as u32;
            reloc_addrs.push(entry_off);
            rsrc.extend_from_slice(&data_entry(payload_offsets[i], payloads[i].len() as u32));
        }
        while (rsrc.len() as u32) < payloads_base_off {
            rsrc.push(0);
        }

        // 5. Raw payloads (8-byte aligned)
        for (i, p) in payloads.iter().enumerate() {
            debug_assert_eq!(rsrc.len() as u32, payload_offsets[i]);
            rsrc.extend_from_slice(p);
            while (rsrc.len() & 7) != 0 {
                rsrc.push(0);
            }
        }
        debug_assert_eq!(rsrc.len() as u32, rsrc_size);

        // Assemble standard COFF object file (matching embed-manifest's CoffWriter format)
        let mut cur_w = Cursor::new(Vec::with_capacity(rsrc.len() + 512));
        cur_w.write_all(&[0u8; 60])?; // header (20) + 1 section header (40)
        cur_w.write_all(&rsrc)?;

        // Relocations (10 bytes each)
        for addr in &reloc_addrs {
            cur_w.write_all(&addr.to_le_bytes())?;
            cur_w.write_all(&0u32.to_le_bytes())?; // symbol index 0 (.rsrc)
            cur_w.write_all(&arch.relocation_type().to_le_bytes())?;
        }

        let ptr_symtab = cur_w.stream_position()? as u32;
        let num_relocs = reloc_addrs.len() as u16;

        // Symbol table: .rsrc section symbol + 1 aux symbol
        cur_w.write_all(b".rsrc\0\0\0")?;
        cur_w.write_all(&0u32.to_le_bytes())?; // value
        cur_w.write_all(&1u16.to_le_bytes())?; // section 1
        cur_w.write_all(&[0, 0, 3, 1])?; // type=0, class=STATIC(3), aux=1
        cur_w.write_all(&rsrc_size.to_le_bytes())?;
        cur_w.write_all(&num_relocs.to_le_bytes())?;
        cur_w.write_all(&[0u8; 12])?;
        // Empty string table
        cur_w.write_all(&4u32.to_le_bytes())?;

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as u32);

        cur_w.seek(SeekFrom::Start(0))?;
        // COFF File Header (20 bytes)
        cur_w.write_all(&arch.machine().to_le_bytes())?;
        cur_w.write_all(&1u16.to_le_bytes())?; // NumberOfSections
        cur_w.write_all(&timestamp.to_le_bytes())?;
        cur_w.write_all(&ptr_symtab.to_le_bytes())?;
        cur_w.write_all(&2u32.to_le_bytes())?; // NumberOfSymbols
        cur_w.write_all(&0u16.to_le_bytes())?; // SizeOfOptionalHeader
        cur_w.write_all(&0u16.to_le_bytes())?; // Characteristics

        // Section Header: .rsrc (40 bytes)
        cur_w.write_all(b".rsrc\0\0\0")?;
        cur_w.write_all(&0u32.to_le_bytes())?; // VirtualSize
        cur_w.write_all(&0u32.to_le_bytes())?; // VirtualAddress
        cur_w.write_all(&rsrc_size.to_le_bytes())?; // SizeOfRawData
        cur_w.write_all(&60u32.to_le_bytes())?; // PointerToRawData
        cur_w.write_all(&(60 + rsrc_size).to_le_bytes())?; // PointerToRelocations
        cur_w.write_all(&0u32.to_le_bytes())?; // PointerToLinenumbers
        cur_w.write_all(&num_relocs.to_le_bytes())?; // NumberOfRelocations
        cur_w.write_all(&0u16.to_le_bytes())?; // NumberOfLinenumbers
        // IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ (0x40000040)
        cur_w.write_all(&0x4000_0040u32.to_le_bytes())?;

        Ok(cur_w.into_inner())
    }
}
