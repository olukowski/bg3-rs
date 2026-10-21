//! Replays a runtime log of the original game with `bg3-sim` as the engine (see
//! `docs/research/osiris-engine-api.md`). Needs the game install and a trace in
//! `.cache/traces/` (never committed); skips cleanly without them.
//!
//! `BG3_OSIRIS_TRACE` selects the trace (default `.cache/traces/full-nautiloid.log` at the
//! workspace root).

use std::path::PathBuf;

use bg3_sim::GameData;
use bg3_sim::replay::{Mode, Options, sim_replay};
use bg3_story::Story;

const STORY: (&str, &str) = ("GustavX", "Mods/GustavX/Story/story.div.osi");

fn trace_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BG3_OSIRIS_TRACE") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.cache/traces/full-nautiloid.log")
}

fn load() -> Option<(Vec<u8>, GameData)> {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let pak = bg3_pak::Package::open(install.data_dir().join(format!("{}.pak", STORY.0)))
        .expect("open pak");
    let story = pak
        .read(pak.find(STORY.1).expect("story in pak"))
        .expect("read");
    let vfs = bg3_vfs::Vfs::from_install(&install).expect("vfs");
    let paks: Vec<bg3_pak::Package> = install
        .paks()
        .expect("paks")
        .iter()
        .map(|p| bg3_pak::Package::open(p).expect("open"))
        .collect();
    let stats =
        bg3_stats::install::load(&paks, bg3_stats::install::BASE_LOAD_ORDER).expect("stats");
    // No levels given: the module's character-creation and startup levels.
    let data = GameData::load(&vfs, &stats, &[]).expect("game data");
    Some((story, data))
}

#[test]
#[ignore = "needs BG3 install and a runtime trace"]
fn sim_replay_trace() {
    let path = trace_path();
    let Ok(log) = std::fs::read(&path) else {
        eprintln!("skipping: no trace at {}", path.display());
        return;
    };
    let log = String::from_utf8_lossy(&log).into_owned();
    let Some((story, data)) = load() else {
        return;
    };
    let reports = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let story = Story::parse(&story).expect("parse story");
            [Mode::Check, Mode::Sim].map(|mode| {
                let opts = Options {
                    mode,
                    character: Some(bg3_sim::start::trace_choice()),
                    ..Options::default()
                };
                sim_replay(&story, &log, data.clone(), &opts)
            })
        })
        .expect("spawn")
        .join()
        .expect("replay thread");
    for r in &reports {
        eprintln!("{}", r.summary());
    }
    let [check, sim] = &reports;
    // Graded sim answers agree with the game.
    assert!(
        check.disagreements.is_empty(),
        "{:?}",
        &check.disagreements[..check.disagreements.len().min(5)]
    );
    // The story behaves identically when the sim answers.
    assert_eq!(
        sim.vm.segments_matched, sim.vm.segments,
        "first divergence: {:?}",
        sim.vm.first
    );
    // Every story timer the game fired, the sim had running.
    for name in ["TimerFinished", "ObjectTimerFinished"] {
        let e = sim.events[name];
        assert_eq!(e.generated, e.expected, "{name}");
    }
}
