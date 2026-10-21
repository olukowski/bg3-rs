//! Decode every animation in `Models.pak` and `English_Animations.pak` (needs the game; run with
//! `--ignored`, ideally `--release`, ~1 min).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use bg3_gr2::Animation;

const PAKS: &[&str] = &["Models", "English_Animations"];

#[test]
#[ignore = "needs BG3 install"]
fn decode_all_animations() {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: no BG3 install ({e})");
            return;
        }
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut animations = 0usize;
    let mut tracks = 0usize;
    let mut failures: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut seen = Vec::new();
    for pak_path in install.paks().expect("list paks") {
        let stem = pak_path.file_stem().unwrap().to_string_lossy().into_owned();
        if !PAKS.contains(&stem.as_str()) {
            continue;
        }
        seen.push(stem.clone());
        let pak = bg3_pak::Package::open(&pak_path).expect("open pak");
        let entries: Vec<_> = pak
            .entries()
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().ends_with(".gr2"))
            .collect();
        let next = AtomicUsize::new(0);
        let totals = Mutex::new((0usize, 0usize, BTreeMap::<String, (usize, String)>::new()));
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    let (mut a, mut t) = (0, 0);
                    let mut f = BTreeMap::<String, (usize, String)>::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(e) = entries.get(i) else { break };
                        let result = pak
                            .read(e)
                            .map_err(|err| format!("pak read: {err}"))
                            .and_then(|d| Animation::from_bytes(&d).map_err(|e| e.to_string()));
                        match result {
                            Ok(anims) => {
                                for anim in &anims {
                                    a += 1;
                                    for g in &anim.track_groups {
                                        t += g.transform_tracks.len();
                                        // Every track must sample to a finite transform.
                                        for tr in &g.transform_tracks {
                                            let x = tr.sample(anim.duration * 0.5);
                                            let ok = x
                                                .position
                                                .iter()
                                                .chain(&x.orientation)
                                                .chain(x.scale_shear.iter().flatten())
                                                .all(|v| v.is_finite());
                                            if !ok {
                                                f.entry("non-finite sample".into())
                                                    .or_insert((0, e.name.clone()))
                                                    .0 += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                f.entry(err).or_insert((0, e.name.clone())).0 += 1;
                            }
                        }
                    }
                    let mut tot = totals.lock().unwrap();
                    tot.0 += a;
                    tot.1 += t;
                    for (k, (n, ex)) in f {
                        tot.2.entry(k).or_insert((0, ex)).0 += n;
                    }
                });
            }
        });
        let (a, t, f) = totals.into_inner().unwrap();
        eprintln!(
            "{stem}: {} GR2 files, {a} animations, {t} transform tracks",
            entries.len()
        );
        animations += a;
        tracks += t;
        for (k, (n, ex)) in f {
            failures.entry(k).or_insert((0, ex)).0 += n;
        }
    }
    if seen.is_empty() {
        eprintln!("skipping: none of {PAKS:?} found");
        return;
    }
    eprintln!("total: {animations} animations, {tracks} transform tracks");
    for (k, (n, ex)) in &failures {
        eprintln!("  {n:>8} {k} (e.g. {ex})");
    }
    assert!(failures.is_empty(), "{} failure kinds", failures.len());
    assert!(animations > 0);
}
