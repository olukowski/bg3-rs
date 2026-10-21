//! End to end: the Nautiloid opening runs on the sim with the sim playing the story's dialogs
//! (`bg3_dialog::DialogRunner` with the sim as its `DialogWorld`, rolls through `bg3_rules`),
//! and every event the dialogs raise — start, actors, flags, rolls, end — must equal the
//! game's, in a replay where the sim's answers drive the story. Needs the install and a trace
//! in `.cache/traces/` (never committed); skips cleanly without them.
//!
//! The player's choices are not in the trace: the harness searches for them (the walk that
//! raises the logged events) and plays them as the player's input. Roll outcomes are taken
//! from the trace and checked for consistency with the rules' modifiers and DC.

use std::path::PathBuf;
use std::sync::Arc;

use bg3_sim::replay::{Mode, Options, sim_replay};
use bg3_sim::{DialogBank, GameData, Guid};
use bg3_story::Story;

const STORY: (&str, &str) = ("GustavX", "Mods/GustavX/Story/story.div.osi");

fn trace_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BG3_OSIRIS_TRACE") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.cache/traces/full-nautiloid.log")
}

fn load() -> Option<(Vec<u8>, GameData, Arc<DialogBank>)> {
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
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install).expect("vfs"));
    let paks: Vec<bg3_pak::Package> = install
        .paks()
        .expect("paks")
        .iter()
        .map(|p| bg3_pak::Package::open(p).expect("open"))
        .collect();
    let stats =
        bg3_stats::install::load(&paks, bg3_stats::install::BASE_LOAD_ORDER).expect("stats");
    let data = GameData::load(&vfs, &stats, &[]).expect("game data");
    let lib = Arc::new(bg3_dialog::library::DialogLibrary::new(vfs).expect("dialogs"));
    Some((story, data, Arc::new(DialogBank::from_library(lib))))
}

fn uuid(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

#[test]
#[ignore = "needs BG3 install and a runtime trace"]
fn nautiloid_dialogs_run_on_the_sim() {
    let path = trace_path();
    let Ok(log) = std::fs::read(&path) else {
        eprintln!("skipping: no trace at {}", path.display());
        return;
    };
    let log = String::from_utf8_lossy(&log).into_owned();
    let Some((story, data, bank)) = load() else {
        return;
    };
    let report = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let story = Story::parse(&story).expect("parse story");
            let opts = Options {
                mode: Mode::Sim,
                dialogs: Some(bank),
                ..Options::default()
            };
            sim_replay(&story, &log, data, &opts)
        })
        .expect("spawn")
        .join()
        .expect("replay thread");
    eprintln!("{}", report.summary());

    // The story behaves exactly as in the game, on the sim's answers.
    assert_eq!(
        report.vm.segments_matched, report.vm.segments,
        "first divergence: {:?}",
        report.vm.first
    );
    assert!(
        report.disagreements.is_empty(),
        "{:?}",
        &report.disagreements[..report.disagreements.len().min(5)]
    );

    // Every complete dialog the story started ran on the sim and raised exactly the logged
    // events: the six instances of the opening.
    let complete: Vec<_> = report
        .dialogs
        .iter()
        .filter(|d| d.owned && !d.cut_off)
        .collect();
    assert_eq!(complete.len(), 6, "{:#?}", report.dialogs);
    for d in &complete {
        assert_eq!(d.generated, d.logged, "{} #{}", d.dialog, d.instance);
    }
    // The recorded dialog with the most going on: 10 flags, 2 passive and 3 active rolls.
    let trapped = complete
        .iter()
        .find(|d| d.dialog.starts_with("TUT_Lab_TrappedDevourer_"))
        .expect("TUT_Lab_TrappedDevourer in the trace");
    assert_eq!(trapped.logged, 28);
    assert_eq!(trapped.rolls.len(), 5);
    assert_eq!(
        trapped.rolls.iter().map(|r| r.success).collect::<Vec<_>>(),
        [true, false, true, true, false]
    );
    assert!(trapped.rolls.iter().all(|r| r.check.is_some()));

    // Dialog flags are now raised by the sim; only the two `SetMagicPocketsFlag` ones are not.
    for name in [
        "DialogStarted",
        "AutomatedDialogEnded",
        "DialogActorJoined",
        "RollResult",
        "DialogRollResult",
    ] {
        let e = report.events[name];
        assert_eq!(e.fed_gap, 0, "{name}");
    }
    assert_eq!(report.events["FlagSet"].fed_outside, 0);

    // The sim's state after the dialogs: flags they set, on whom the game set them.
    let sim = &report.sim;
    let avatar = uuid("1bcd4cdb-0e71-8163-9b10-70cbd5b467a4");
    for (flag, holder) in [
        // TUT_Lab_Hasmet_Devourer (object flag on the avatar)
        ("a6ba178c-3ecd-4220-836d-d523564fc257", avatar),
        // TUT_Lab_State_FreedBrain (global)
        ("2a0d14f5-661b-46d5-8910-98cc4f85c3e3", Guid::NULL),
        // TUT_Start_State_InvestigatedPool (object flag on the avatar, Brinepool)
        ("62211be4-0313-4bce-b14a-3e4bfd409235", avatar),
    ] {
        assert!(sim.flags.get(holder, uuid(flag)), "flag {flag} on {holder}");
    }
    let sheet = sim.entity(avatar).and_then(|e| e.sheet.clone());
    eprintln!("avatar sheet: {sheet:?}");
}
