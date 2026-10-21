//! Boots the shipped campaign story with a mock host. Needs the game install.

use bg3_osiris::{EventOutcome, GoalState, RecordingHost, Value, Vm};
use bg3_story::Story;

fn read_story(pak_stem: &str, path: &str) -> Option<Vec<u8>> {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let pak = bg3_pak::Package::open(install.data_dir().join(format!("{pak_stem}.pak")))
        .expect("open pak");
    let entry = pak
        .find(path)
        .unwrap_or_else(|| panic!("{path} not in {pak_stem}"));
    Some(pak.read(entry).expect("read story"))
}

fn summary(vm: &Vm, host: &RecordingHost, label: &str) {
    let goals = vm.goals();
    let count = |s| goals.iter().filter(|(_, g)| *g == s).count();
    let dbs = vm.databases();
    let rows: usize = dbs.iter().map(|d| d.2).sum();
    let non_empty = dbs.iter().filter(|d| d.2 > 0).count();
    let stats = vm.stats();
    let (errors, n_errors) = vm.errors();
    eprintln!(
        "[{label}] goals: {} active, {} sleeping, {} completed | databases: {} ({non_empty} non-empty, {rows} rows) | \
         calls {} (recorded {}), engine queries {}, user queries {}, internal queries {}, rules fired {}, \
         actions {}, facts +{} -{}, max nesting {} | VM errors {n_errors}",
        count(GoalState::Active),
        count(GoalState::Sleeping),
        count(GoalState::Completed),
        dbs.len(),
        stats.calls,
        host.calls.len(),
        stats.engine_queries,
        stats.user_queries,
        stats.internal_queries,
        stats.rules_fired,
        stats.actions_fired,
        stats.facts_inserted,
        stats.facts_deleted,
        stats.max_rule_nesting,
    );
    for e in errors.iter().take(20) {
        eprintln!("  error: {e}");
    }
}

#[test]
#[ignore = "needs BG3 install"]
fn boot_gustav_dev() {
    let Some(data) = read_story("Gustav", "Mods/GustavDev/Story/story.div.osi") else {
        return;
    };
    // Rule chains nest deeply; give the VM a big stack like the game's main thread would need.
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || run(&data))
        .expect("spawn")
        .join()
        .expect("boot thread");
}

fn run(data: &[u8]) {
    let story = Story::parse(data).expect("parse story");
    let t = std::time::Instant::now();
    let mut vm = Vm::new(&story);
    let mut host = RecordingHost::new();
    vm.init_game(&mut host);
    eprintln!("init took {:?}", t.elapsed());
    summary(&vm, &host, "after InitGame");
    assert_eq!(vm.errors().1, 0, "VM errors during InitGame");
    assert!(vm.goals().iter().any(|(_, g)| *g == GoalState::Active));

    // Startup events the engine sends once a level is up. The real order and arguments still
    // have to be confirmed with a runtime trace (see docs/research/osiris-vm.md); this only
    // exercises the VM on them.
    let campaign = vm.string("Campaign");
    let level = vm.string("TUT_Avernus_C");
    let events: [(&str, Vec<Value>); 3] = [
        (
            "GameModeStarted",
            vec![campaign, Value::int(0), Value::int(0)],
        ),
        ("LevelLoaded", vec![level]),
        ("LevelGameplayStarted", vec![level, Value::int(0)]),
    ];
    for (name, args) in events {
        let outcome = vm.event(&mut host, name, &args);
        eprintln!("event {name}: {outcome:?}");
        assert_ne!(outcome, EventOutcome::Unknown, "{name}");
    }
    summary(&vm, &host, "after startup events");
    let mut names: Vec<&str> = host.call_names();
    names.sort_unstable();
    names.dedup();
    eprintln!("distinct engine calls issued: {}", names.len());
    assert_eq!(vm.errors().1, 0, "VM errors during startup events");
}
