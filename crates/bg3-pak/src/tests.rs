use super::*;

/// Build a single-part v18 archive in memory from `(name, flags, data)` tuples.
fn build(files: &[(&str, u8, &[u8])]) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_SIZE];
    let mut table = Vec::new();
    for (name, flags, data) in files {
        let stored = match Compression::from_flags(*flags).unwrap() {
            Compression::None => data.to_vec(),
            Compression::Lz4 => lz4_flex::block::compress(data),
            Compression::Zstd => zstd::bulk::compress(data, 3).unwrap(),
            Compression::Zlib => {
                use std::io::Write;
                let mut enc =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                enc.write_all(data).unwrap();
                enc.finish().unwrap()
            }
        };
        let offset = out.len() as u64;
        out.extend_from_slice(&stored);
        let mut entry = [0u8; ENTRY_SIZE];
        entry[..name.len()].copy_from_slice(name.as_bytes());
        entry[256..260].copy_from_slice(&(offset as u32).to_le_bytes());
        entry[260..262].copy_from_slice(&((offset >> 32) as u16).to_le_bytes());
        entry[263] = *flags;
        entry[264..268].copy_from_slice(&(stored.len() as u32).to_le_bytes());
        let uncompressed = if *flags & 0x0f == 0 {
            0
        } else {
            data.len() as u32
        };
        entry[268..272].copy_from_slice(&uncompressed.to_le_bytes());
        table.extend_from_slice(&entry);
    }
    let compressed = lz4_flex::block::compress(&table);
    let list_offset = out.len() as u64;
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    let list_size = (out.len() as u64 - list_offset) as u32;

    out[0..4].copy_from_slice(MAGIC);
    out[4..8].copy_from_slice(&SUPPORTED_VERSION.to_le_bytes());
    out[8..16].copy_from_slice(&list_offset.to_le_bytes());
    out[16..20].copy_from_slice(&list_size.to_le_bytes());
    out[0x26..0x28].copy_from_slice(&1u16.to_le_bytes());
    out
}

fn open_bytes(bytes: &[u8]) -> Package {
    let dir = std::env::temp_dir().join(format!("bg3-pak-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.pak", bytes.len()));
    std::fs::write(&path, bytes).unwrap();
    Package::open(&path).unwrap()
}

#[test]
fn roundtrip_all_compression_methods() {
    let text = b"hello hello hello hello hello world".repeat(20);
    let pak = open_bytes(&build(&[
        ("Public/a.txt", 0x00, b"stored"),
        ("Public/b.lsf", 0x02, &text),
        ("Public/c.lsx", 0x03, &text),
        ("Public/d.bin", 0x01, &text),
    ]));
    assert_eq!(pak.entries().len(), 4);
    assert_eq!(
        pak.read(pak.find("Public/a.txt").unwrap()).unwrap(),
        b"stored"
    );
    for name in ["Public/b.lsf", "Public/c.lsx", "Public/d.bin"] {
        let entry = pak.find(name).unwrap();
        assert_eq!(entry.size(), text.len() as u64);
        assert_eq!(pak.read(entry).unwrap(), text, "{name}");
    }
}

#[test]
fn rejects_bad_magic_and_version() {
    let mut bytes = build(&[]);
    bytes[4] = 17;
    assert!(matches!(
        parse_header(&bytes),
        Err(Error::UnsupportedVersion(17))
    ));
    bytes[0] = b'X';
    assert!(matches!(parse_header(&bytes), Err(Error::BadMagic)));
}

#[test]
fn part_paths() {
    assert_eq!(
        part_path(Path::new("/d/Textures.pak"), 3),
        PathBuf::from("/d/Textures_3.pak")
    );
}

#[test]
#[ignore = "needs BG3 install"]
fn open_every_pak_and_read_samples() {
    let install = bg3_install::Install::locate().expect("BG3 install");
    for path in install.paks().unwrap() {
        let pak = Package::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!pak.entries().is_empty(), "{}", path.display());
        // Read a spread of entries, skipping very large ones to keep the test fast.
        let step = (pak.entries().len() / 50).max(1);
        for entry in pak
            .entries()
            .iter()
            .step_by(step)
            .filter(|e| e.size() < 64 << 20)
        {
            let data = pak
                .read(entry)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(data.len() as u64, entry.size(), "{}", entry.name);
        }
    }
}
