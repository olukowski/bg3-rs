//! Survey GR2 headers across the install. `cargo run --release -p bg3-gr2 --example gr2_survey`
use std::collections::BTreeMap;

fn main() {
    let install = bg3_install::Install::locate().expect("BG3 install");
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for pak in install.paks().unwrap() {
        let Ok(p) = bg3_pak::Package::open(&pak) else {
            continue;
        };
        let name = pak.file_stem().unwrap().to_string_lossy().to_string();
        for e in p.entries() {
            if !e.name.to_ascii_lowercase().ends_with(".gr2") {
                continue;
            }
            let data = match p.read(e) {
                Ok(d) => d,
                Err(_) => {
                    *by.entry(format!("{name} readerr")).or_default() += 1;
                    continue;
                }
            };
            *by.entry(format!("{name} files")).or_default() += 1;
            match bg3_gr2::Header::parse(&data) {
                Err(err) => {
                    eprintln!(
                        "{name}: {} {} {:02x?}",
                        e.name,
                        data.len(),
                        &data[..data.len().min(16)]
                    );
                    *by.entry(format!("{name} err {err}")).or_default() += 1
                }
                Ok(h) => {
                    *by.entry(format!(
                        "hdr fmt={:?} hsize={:#x} hfmt={} rev={} tag={:#x} extra={:?} nsec={}",
                        h.format,
                        h.header_size,
                        h.header_format,
                        h.revision,
                        h.type_tag,
                        h.extra_tags,
                        h.sections.len()
                    ))
                    .or_default() += 1;
                    for (i, s) in h.sections.iter().enumerate() {
                        if s.expanded_size == 0 {
                            *by.entry(format!("sec{i} empty")).or_default() += 1;
                            continue;
                        }
                        *by.entry(format!("{name} codec {:?}", s.compression))
                            .or_default() += 1;
                        *by.entry(format!(
                            "sec{i} codec {:?} align {}",
                            s.compression, s.alignment
                        ))
                        .or_default() += 1;
                        if s.marshalling_count > 0 {
                            *by.entry("marshalling>0".into()).or_default() += 1;
                        }
                        if s.first_16bit != s.expanded_size || s.first_8bit != s.expanded_size {
                            *by.entry(format!("stops != size ({:?})", s.compression))
                                .or_default() += 1;
                        }
                    }
                }
            }
        }
    }
    for (k, v) in by {
        println!("{v:8}  {k}");
    }
}
