use super::*;

#[test]
fn round_trip() {
    let data = write([
        ("habc", 1, "Hello"),
        ("hdef", 3, "Wörld <br>"),
        ("hempty", 2, ""),
    ]);
    let loca = Loca::parse(&data).unwrap();
    assert_eq!(loca.len(), 3);
    assert_eq!(loca.text("habc"), Some("Hello"));
    assert_eq!(loca.text("hdef;3"), Some("Wörld <br>"));
    assert_eq!(loca.get("hdef").unwrap().version, 3);
    assert_eq!(loca.text("hempty"), Some(""));
    assert_eq!(loca.text("hmissing"), None);
}

#[test]
fn empty_stub() {
    // The game ships 12-byte stub files: header only, zero entries.
    let stub = [b'L', b'O', b'C', b'A', 0, 0, 0, 0, 12, 0, 0, 0];
    let loca = Loca::parse(&stub).unwrap();
    assert!(loca.is_empty());
}

#[test]
fn duplicates_later_wins() {
    let data = write([("h1", 1, "old"), ("h1", 2, "new")]);
    let loca = Loca::parse(&data).unwrap();
    assert_eq!(loca.len(), 1);
    assert_eq!(loca.duplicates(), 1);
    assert_eq!(loca.text("h1"), Some("new"));

    let mut merged = Loca::parse(&write([("h2", 1, "a")])).unwrap();
    merged.merge(Loca::parse(&write([("h2", 1, "b")])).unwrap());
    assert_eq!(merged.text("h2"), Some("b"));
}

#[test]
fn rejects_bad_input() {
    assert!(matches!(Loca::parse(b"LOC"), Err(Error::Corrupt(_))));
    assert!(matches!(
        Loca::parse(b"XXXX\0\0\0\0\x0c\0\0\0"),
        Err(Error::BadMagic)
    ));
    let mut data = write([("h1", 1, "text")]);
    data.truncate(data.len() - 2);
    assert!(matches!(Loca::parse(&data), Err(Error::Corrupt(_))));
    // Count claims more entries than fit.
    let mut data = write([("h1", 1, "x")]);
    data[4] = 50;
    assert!(Loca::parse(&data).is_err());
}

#[test]
fn reference_split() {
    assert_eq!(split_reference("habc;2"), ("habc", Some(2)));
    assert_eq!(split_reference("habc"), ("habc", None));
}

#[test]
#[ignore = "needs BG3 install"]
fn load_english() {
    let Ok(install) = bg3_install::Install::locate() else {
        eprintln!("BG3 install not found; skipping");
        return;
    };
    let pak = bg3_pak::Package::open(install.localization_dir().join("English.pak")).unwrap();
    let mut total = Loca::new();
    for entry in pak.entries().iter().filter(|e| e.name.ends_with(".loca")) {
        let data = pak.read(entry).unwrap();
        let loca = Loca::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", entry.name));
        eprintln!(
            "{}: {} entries, {} duplicate handles",
            entry.name,
            loca.len(),
            loca.duplicates()
        );
        total.merge(loca);
    }
    eprintln!("total: {} handles", total.len());
    assert!(total.len() > 100_000, "only {} entries", total.len());
    assert!(total.iter().all(|(h, _)| h.starts_with('h')));
}
