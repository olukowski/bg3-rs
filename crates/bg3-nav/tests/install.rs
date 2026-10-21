//! Tests against the real BG3 install. Run with
//! `cargo test -p bg3-nav --release -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use bg3_nav::{Agent, AiGridFile, NavCloud, NavGrid};
use bg3_vfs::Vfs;
use rayon::prelude::*;

fn open_vfs() -> Option<Vfs> {
    match bg3_install::Install::locate() {
        Ok(install) => Some(Vfs::from_install(&install).expect("mount paks")),
        Err(e) => {
            eprintln!("skipping: BG3 install not found ({e})");
            None
        }
    }
}

#[test]
#[ignore = "needs BG3 install"]
fn every_aigrid_parses() {
    let Some(vfs) = open_vfs() else { return };
    let paths: Vec<String> = vfs
        .files_under("Mods/")
        .map(|f| f.entry.name.clone())
        .filter(|p| p.ends_with("/Ai/aigrid.data"))
        .collect();
    assert!(paths.len() > 100, "only {} AI grids", paths.len());
    let failures = Mutex::new(Vec::new());
    let legacy = Mutex::new(Vec::<String>::new());
    let stats = Mutex::new((0usize, 0usize, 0usize, BTreeMap::<usize, usize>::new()));
    paths.par_iter().for_each(|p| {
        match vfs
            .read(p)
            .map_err(|e| e.to_string())
            .and_then(|d| AiGridFile::parse(&d).map_err(|e| e.to_string()))
        {
            Ok(g) => {
                let mut s = stats.lock().unwrap();
                s.0 += g.subgrids.len();
                s.1 += g.subgrids.iter().map(|s| s.tiles.len()).sum::<usize>();
                s.2 += g
                    .subgrids
                    .iter()
                    .flat_map(|s| &s.tiles)
                    .filter(|t| !t.is_empty())
                    .count();
                *s.3.entry(g.layers.len()).or_default() += 1;
            }
            Err(e) if e.contains("unsupported version") => {
                legacy.lock().unwrap().push(format!("{p}: {e}"))
            }
            Err(e) => failures.lock().unwrap().push(format!("{p}: {e}")),
        }
    });
    let s = stats.into_inner().unwrap();
    eprintln!(
        "{} AI grids: {} subgrids, {} tiles ({} non-empty); files by layer count {:?}",
        paths.len(),
        s.0,
        s.1,
        s.2,
        s.3
    );
    // Older file versions (different header and tile layouts, converted by the game on load)
    // aren't supported; they are sub-levels, platforms and cinematic levels.
    let legacy = legacy.into_inner().unwrap();
    let mut versions = BTreeMap::<String, usize>::new();
    for l in &legacy {
        *versions
            .entry(l.rsplit(' ').next().unwrap().to_owned())
            .or_default() += 1;
    }
    eprintln!("{} legacy-version grids: {versions:?}", legacy.len());
    for main in [
        "TUT_Avernus_C",
        "WLD_Main_A",
        "SCL_Main_A",
        "BGO_Main_A",
        "CTY_Main_A",
    ] {
        assert!(
            !legacy.iter().any(|l| l.contains(&format!("/{main}/"))),
            "{main} is a legacy grid"
        );
    }
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
#[ignore = "needs BG3 install"]
fn every_navcloud_parses() {
    let Some(vfs) = open_vfs() else { return };
    let metas: Vec<String> = vfs
        .files_under("Mods/")
        .map(|f| f.entry.name.clone())
        .filter(|p| p.ends_with("/NavCloud/navcloud.meta"))
        .collect();
    assert!(!metas.is_empty());
    for meta in &metas {
        let data = meta.replace("navcloud.meta", "navcloud.data");
        let nc = NavCloud::parse(&vfs.read(meta).unwrap(), &vfs.read(&data).unwrap())
            .unwrap_or_else(|e| panic!("{meta}: {e}"));
        let fine = nc.regions.iter().filter(|r| r.kind == 1).count();
        let open: usize = nc
            .regions
            .iter()
            .map(|r| r.voxels.iter().filter(|&&v| v != 0).count())
            .sum();
        let values: std::collections::BTreeSet<u8> = nc
            .regions
            .iter()
            .flat_map(|r| r.voxels.iter().copied())
            .collect();
        eprintln!(
            "{meta}: {} regions ({fine} fine), {open} non-zero voxels, values {values:?}",
            nc.regions.len()
        );
    }
}

#[test]
#[ignore = "needs BG3 install"]
fn nautiloid_deck_path() {
    let Some(vfs) = open_vfs() else { return };
    let grid = NavGrid::new(bg3_nav::load_aigrid(&vfs, "TUT_Avernus_C").expect("aigrid"));
    let agent = Agent::humanoid();
    let walkable = grid
        .file
        .subgrids
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (0..s.height)
                .flat_map(|y| (0..s.width).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    grid.is_standable(
                        bg3_nav::TileRef {
                            subgrid: i as u32,
                            x: x as u16,
                            y: y as u16,
                        },
                        &agent,
                    )
                })
                .count()
        })
        .sum::<usize>();
    eprintln!("TUT_Avernus_C: {walkable} tiles standable for a humanoid");
    assert!(walkable > 5_000);

    // From the raised walkway down to the deck below the viewer's default Nautiloid camera
    // (BG3 world coordinates; both in the second-largest connected region).
    let from = [-42.75, 22.06, -372.75];
    let to = [-60.0, 17.84, -389.0];
    let started = std::time::Instant::now();
    let path = grid.find_path(from, to, &agent).expect("path on the deck");
    eprintln!(
        "path: {} tiles, {} waypoints, {:.1} m, cost {}, {} expanded in {:.2?}",
        path.tiles.len(),
        path.waypoints.len(),
        path.length(),
        path.cost,
        path.expanded,
        started.elapsed()
    );
    for w in &path.waypoints {
        eprintln!("  {:.2} {:.2} {:.2}", w[0], w[1], w[2]);
    }
    let straight = ((to[0] - from[0]).powi(2) + (to[2] - from[2]).powi(2)).sqrt();
    assert!(path.length() >= straight - 1.0);
    assert!(path.length() < straight * 2.0);
    for w in path.tiles.windows(2) {
        assert!((grid.height(w[0]) - grid.height(w[1])).abs() <= agent.step_height);
    }
}
