//! Replays a runtime log captured from the original game (libOsiris' own log, see
//! `docs/research/osiris-runtime-log.md`) through the VM and compares. Needs the game install
//! and a trace in `.cache/traces/` (never committed); skips cleanly without them.
//!
//! `BG3_OSIRIS_TRACE` selects the trace (default `.cache/traces/full-nautiloid.log` at the
//! workspace root, else `.cache/traces/snapshot-early.log`). `BG3_OSIRIS_VM_LOG=path` also
//! writes the VM's own log in libOsiris' format, for diffing.

use std::path::PathBuf;

use bg3_osiris::replay::replay_with;
use bg3_story::Story;

/// The story a new campaign game runs (goal ids in the log match this one, not GustavDev).
const STORY: (&str, &str) = ("GustavX", "Mods/GustavX/Story/story.div.osi");

/// Known trace limitations that are not VM bugs, as `(event name, reason)`: a diverging
/// segment for one of these events is tolerated. Empty: the captured traces replay exactly.
/// Candidates if a future trace needs them: events the engine raises from inside a call
/// (none seen so far: every `>>> call COsiris::Event` is at top level), or engine queries
/// whose answer the log does not contain (zero-parameter queries print no `Query returns`).
const ALLOWED: &[(&str, &str)] = &[];

fn trace_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BG3_OSIRIS_TRACE") {
        return PathBuf::from(p);
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".cache")
        .join("traces");
    let full = dir.join("full-nautiloid.log");
    if full.exists() {
        full
    } else {
        dir.join("snapshot-early.log")
    }
}

fn read_story() -> Option<Vec<u8>> {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let pak = bg3_pak::Package::open(install.data_dir().join(format!("{}.pak", STORY.0)))
        .expect("open pak");
    let entry = pak.find(STORY.1).expect("story in pak");
    Some(pak.read(entry).expect("read story"))
}

#[test]
#[ignore = "needs BG3 install and a runtime trace"]
fn replay_trace() {
    let path = trace_path();
    let Ok(log) = std::fs::read(&path) else {
        eprintln!("skipping: no trace at {}", path.display());
        return;
    };
    let log = String::from_utf8_lossy(&log).into_owned();
    let Some(data) = read_story() else {
        return;
    };
    let report = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let story = Story::parse(&data).expect("parse story");
            replay_with(
                &story,
                &log,
                std::env::var_os("BG3_OSIRIS_VM_LOG").is_some(),
            )
        })
        .expect("spawn")
        .join()
        .expect("replay thread");
    eprintln!("{}", report.summary());
    if let (Some(path), Some(text)) = (std::env::var_os("BG3_OSIRIS_VM_LOG"), &report.vm_log) {
        std::fs::write(path, text).expect("write VM log");
    }
    assert_eq!(report.vm_errors, 0, "VM errors during replay");
    let unexplained: Vec<_> = report
        .diverging
        .iter()
        .filter(|(_, event, _, _)| !ALLOWED.iter().any(|(e, _)| e == event))
        .collect();
    assert!(
        unexplained.is_empty(),
        "replay diverges from the trace: {unexplained:#?}"
    );
    assert_eq!(report.host.unanswered, 0, "queries the trace cannot answer");
    assert_eq!(
        report.type_mismatches, 0,
        "values agree but print different (TYPE) annotations"
    );
}
