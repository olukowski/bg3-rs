//! Parse every `.GR2` in the install (needs the game; run with `--ignored`, ideally `--release`).
//!
//! Set `BG3_GR2_PAKS=Models,Gustav` to restrict the paks scanned.

use std::collections::BTreeMap;
use std::sync::Mutex;

use bg3_gr2::{Error, Scene};

fn category(e: &Error) -> String {
    match e {
        Error::Decompress { what, source, .. } => format!("decompress {what}: {source:?}")
            .split('{')
            .next()
            .unwrap()
            .trim()
            .to_owned(),
        Error::Corrupt(s) => format!("corrupt: {s}"),
        Error::Truncated(s) => format!("truncated: {s}"),
        Error::MissingMember(m) => format!("missing member {m}"),
        Error::WrongMemberType { name, found } => format!("wrong type {name}: {found:?}"),
        Error::UnsupportedVertexComponent { name, kind, count } => {
            format!("vertex component {name} {kind:?}x{count}")
        }
        other => format!("{other}"),
    }
}

#[test]
#[ignore = "needs BG3 install"]
fn parse_all_gr2_in_install() {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: no BG3 install ({e})");
            return;
        }
    };
    let filter: Option<Vec<String>> = std::env::var("BG3_GR2_PAKS")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());

    let mut total = 0usize;
    let mut failures: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    for pak_path in install.paks().expect("list paks") {
        let stem = pak_path.file_stem().unwrap().to_string_lossy().into_owned();
        if let Some(f) = &filter
            && !f.contains(&stem)
        {
            continue;
        }
        let pak = bg3_pak::Package::open(&pak_path).expect("open pak");
        let entries: Vec<_> = pak
            .entries()
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().ends_with(".gr2"))
            .collect();
        if entries.is_empty() {
            continue;
        }
        total += entries.len();
        let fails = Mutex::new(BTreeMap::<String, (usize, String)>::new());
        let st = Mutex::new(BTreeMap::<String, usize>::new());
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    let mut local_stats = BTreeMap::<String, usize>::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(e) = entries.get(i) else { break };
                        let result = pak
                            .read(e)
                            .map_err(|err| format!("pak read: {err}"))
                            .and_then(|data| Scene::from_bytes(&data).map_err(|e| category(&e)));
                        match result {
                            Ok(scene) => {
                                let kind = match (
                                    scene.meshes.is_empty(),
                                    scene.skeletons.is_empty(),
                                    scene.animation_count,
                                ) {
                                    (false, _, _) => "meshes",
                                    (true, false, 0) => "skeleton only",
                                    (true, _, n) if n > 0 => "animation",
                                    _ => "empty",
                                };
                                *local_stats.entry(format!("files: {kind}")).or_default() += 1;
                                let meshes = scene.meshes.len();
                                *local_stats.entry("meshes".into()).or_default() += meshes;
                            }
                            Err(cat) => {
                                let mut f = fails.lock().unwrap();
                                let slot = f
                                    .entry(format!("{stem}: {cat}"))
                                    .or_insert((0, e.name.clone()));
                                slot.0 += 1;
                            }
                        }
                    }
                    let mut g = st.lock().unwrap();
                    for (k, v) in local_stats {
                        *g.entry(k).or_default() += v;
                    }
                });
            }
        });
        for (k, v) in fails.into_inner().unwrap() {
            failures.insert(k, v);
        }
        for (k, v) in st.into_inner().unwrap() {
            *stats.entry(k).or_default() += v;
        }
    }

    eprintln!("parsed {total} GR2 files");
    for (k, v) in &stats {
        eprintln!("  {v:8}  {k}");
    }
    let failed: usize = failures.values().map(|(n, _)| n).sum();
    for (cat, (n, example)) in &failures {
        eprintln!("FAIL {n:6}  {cat}   e.g. {example}");
    }
    eprintln!("{failed} failures");
    assert!(total > 0, "no GR2 files found");
    assert_eq!(failed, 0, "{failed} of {total} GR2 files failed to parse");
}
