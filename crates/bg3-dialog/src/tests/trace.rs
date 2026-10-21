//! Consistency check against a runtime log of the original game (the libOsiris log format,
//! `docs/research/osiris-runtime-log.md`). The log is never committed; point `BG3_TRACE` at it
//! (default: `.cache/traces/full-nautiloid.log` in the workspace).
//!
//! For every dialog instance in the log, the test rebuilds the world state from the events
//! before it (`FlagSet`, `TagSet`, `TagCleared`), fills the speaker slots from its
//! `DialogActorJoined` events, takes roll outcomes from its `RollResult` events, and searches
//! the player's choices for a walk whose Osiris events — start, actors, flags, rolls, end —
//! equal the logged ones, in order.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::defs::Definitions;
use crate::library::{DialogLibrary, split_uuid_suffix};
use crate::{
    DialogEvent, DialogRunner, DialogWorld, FlagType, Guid, MemoryWorld, RollOutcome, StartParams,
    Step,
};

fn trace_path() -> std::path::PathBuf {
    match std::env::var_os("BG3_TRACE") {
        Some(p) => p.into(),
        None => std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.cache/traces/full-nautiloid.log"),
    }
}

/// `Name_uuid` → the uuid (also accepts a bare uuid).
fn uuid_of(s: &str) -> Option<Guid> {
    let s = s.trim().trim_matches('"');
    split_uuid_suffix(s)
        .map(|(_, g)| g)
        .or_else(|| Guid::parse(s))
        .or_else(|| s.get(s.len().checked_sub(36)?..).and_then(Guid::parse))
}

/// `Name(a, b, c)` → (`Name`, [a, b, c]). Arguments never contain commas in these events.
fn split_event(e: &str) -> Option<(&str, Vec<&str>)> {
    let (name, rest) = e.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    Some((name, args.split(", ").collect()))
}

struct Instance {
    dialog: String,
    id: u32,
    /// Index of the start event in the event list.
    start: usize,
    /// The logged events that belong to this instance.
    expected: Vec<String>,
    /// Speaker slot → (actor uuid, logged name).
    actors: Vec<(i32, Guid, String)>,
    /// Roll outcomes in order: (roll node uuid, success).
    rolls: Vec<(Guid, bool)>,
}

fn instances(events: &[String]) -> Vec<Instance> {
    let mut out = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let Some((name, args)) = split_event(e) else {
            continue;
        };
        if name != "DialogStarted" && name != "AutomatedDialogStarted" {
            continue;
        }
        let (dialog, id) = (args[0].to_owned(), args[1].parse::<u32>().unwrap());
        let suffix = format!("_{id}\"");
        let mut inst = Instance {
            dialog: dialog.clone(),
            id,
            start: i,
            expected: vec![e.clone()],
            actors: Vec::new(),
            rolls: Vec::new(),
        };
        for e in &events[i + 1..] {
            let Some((name, args)) = split_event(e) else {
                continue;
            };
            let ours = match name {
                "DialogActorJoined" | "DialogActorLeft" => {
                    args[0] == dialog && args[1] == id.to_string()
                }
                "FlagSet" | "FlagCleared" => args[2] == id.to_string(),
                "RollResult" => args[0].ends_with(&suffix),
                "DialogRollResult" => args[2] == dialog,
                "NestedDialogPlayed" => args[1] == id.to_string(),
                "InstanceDialogChanged" => args[0] == id.to_string(),
                "DialogEnded" | "AutomatedDialogEnded" => {
                    args[0] == dialog && args[1] == id.to_string()
                }
                _ => false,
            };
            if !ours {
                continue;
            }
            inst.expected.push(e.clone());
            match name {
                "DialogActorJoined" => inst.actors.push((
                    args[3].parse().unwrap(),
                    uuid_of(args[2]).unwrap(),
                    args[2].to_owned(),
                )),
                "RollResult" => {
                    let node = args[0].trim_matches('"').rsplit_once('_').unwrap().0;
                    inst.rolls
                        .push((Guid::parse(node).unwrap(), args[3] == "1"));
                }
                "DialogEnded" | "AutomatedDialogEnded" => break,
                _ => {}
            }
        }
        out.push(inst);
    }
    out
}

/// World state from the events before `upto`.
fn world_before(events: &[String], upto: usize, defs: &Definitions) -> MemoryWorld {
    let mut w = MemoryWorld::with_definitions(defs);
    for e in &events[..upto] {
        let Some((name, args)) = split_event(e) else {
            continue;
        };
        match name {
            "FlagSet" | "FlagCleared" => {
                let (Some(flag), target) = (uuid_of(args[0]), uuid_of(args[1])) else {
                    continue;
                };
                let Some(ty) = w.flag_type(&flag) else {
                    continue;
                };
                let target = target.filter(|t| *t != Guid::default());
                let target = if ty.per_object() { target } else { None };
                w.set_flag(&flag, ty, target.as_ref(), name == "FlagSet", 0);
            }
            "TagSet" | "TagCleared" => {
                if let (Some(a), Some(t)) = (uuid_of(args[0]), uuid_of(args[1])) {
                    w.set_dialog_tag(&a, &t, name == "TagSet");
                }
            }
            _ => {}
        }
    }
    w
}

enum Outcome {
    /// The walk diverged from the log after matching this many events; the first wrong one.
    Mismatch(usize, Option<String>),
    /// The walk reproduced the log completely.
    Match,
    /// Choices are on offer: how many (and a state signature for de-duplication).
    Choose(usize, u64),
}

struct Search<'a> {
    lib: Arc<DialogLibrary>,
    defs: &'a Definitions,
    events: &'a [String],
    inst: &'a Instance,
    names: HashMap<Guid, String>,
}

impl Search<'_> {
    fn name(&self, g: &Guid) -> String {
        if let Some(n) = self.names.get(g) {
            return n.clone();
        }
        match self.defs.flags.get(g) {
            Some(f) => f.osiris_name(),
            None => g.to_string(),
        }
    }

    /// Replays the dialog taking `path` at the choice points.
    fn replay(&self, path: &[usize]) -> Outcome {
        let dialog = self.lib.load(&self.inst.dialog).expect("dialog in library");
        let mut world = world_before(self.events, self.inst.start, self.defs);
        let rolls = Arc::new(Mutex::new(
            self.inst.rolls.iter().copied().collect::<VecDeque<_>>(),
        ));
        let r2 = rolls.clone();
        world.set_roller(move |_req| RollOutcome {
            success: r2.lock().unwrap().pop_front().is_some_and(|(_, s)| s),
            critical: None,
        });
        let slots = self.inst.actors.iter().map(|a| a.0 + 1).max().unwrap_or(0) as usize;
        let mut speakers = vec![None; slots];
        for (slot, actor, _) in &self.inst.actors {
            speakers[*slot as usize] = Some(*actor);
        }
        let mut run = DialogRunner::new(
            dialog,
            self.lib.clone(),
            StartParams {
                instance: self.inst.id,
                speakers,
            },
        );
        run.start(&mut world);
        let mut produced: Vec<String> = Vec::new();
        let mut path = path.iter();
        loop {
            for e in run.take_events() {
                produced.extend(e.osiris(&|g| self.name(g)));
            }
            let matched = produced
                .iter()
                .zip(&self.inst.expected)
                .take_while(|(a, b)| a == b)
                .count();
            if matched < produced.len() {
                return Outcome::Mismatch(matched, produced.get(matched).cloned());
            }
            match run.step().clone() {
                Step::Line(_) => {
                    run.advance(&mut world).unwrap();
                }
                Step::Choices(options) => match path.next() {
                    Some(&i) => {
                        if run.choose(&mut world, i).is_err() {
                            return Outcome::Mismatch(produced.len(), None);
                        }
                    }
                    None => {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        produced.len().hash(&mut h);
                        run.current_node().hash(&mut h);
                        options.iter().for_each(|o| o.node.hash(&mut h));
                        let mut flags: Vec<_> = world.flags().collect();
                        flags.sort_by_key(|f| (f.0, f.1));
                        flags.hash(&mut h);
                        return Outcome::Choose(options.len(), h.finish());
                    }
                },
                Step::Ended => {
                    return if produced == self.inst.expected {
                        Outcome::Match
                    } else {
                        Outcome::Mismatch(produced.len(), Some("(ended)".into()))
                    };
                }
            }
        }
    }

    /// Depth-first search over choice paths; returns the first that reproduces the log, else
    /// prints the walk that got furthest.
    fn find(&self, budget: &mut usize) -> Option<Vec<usize>> {
        let mut seen = HashSet::new();
        let mut stack = vec![Vec::new()];
        let mut best: (usize, Vec<usize>, Option<String>) = (0, Vec::new(), None);
        while let Some(path) = stack.pop() {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            match self.replay(&path) {
                Outcome::Match => return Some(path),
                Outcome::Mismatch(n, wrong) => {
                    if n > best.0 {
                        best = (n, path, wrong);
                    }
                }
                Outcome::Choose(n, sig) => {
                    if !seen.insert(sig) {
                        continue;
                    }
                    for i in (0..n).rev() {
                        let mut p = path.clone();
                        p.push(i);
                        stack.push(p);
                    }
                }
            }
        }
        let (n, path, wrong) = best;
        eprintln!(
            "  best walk {:?} matched {n}/{}; next logged: {:?}; produced instead: {wrong:?}",
            path.iter().map(|i| i + 1).collect::<Vec<_>>(),
            self.inst.expected.len(),
            self.inst.expected.get(n)
        );
        None
    }
}

#[test]
#[ignore = "needs BG3 install and a runtime log (BG3_TRACE)"]
fn trace_dialogs_reproduce_logged_events() {
    let path = trace_path();
    let Ok(log) = std::fs::read_to_string(&path) else {
        eprintln!("skipping: no trace at {}", path.display());
        return;
    };
    let Ok(install) = bg3_install::Install::locate() else {
        eprintln!("skipping: no BG3 install");
        return;
    };
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install).unwrap());
    let defs = Definitions::load(&vfs).unwrap();
    let lib = Arc::new(DialogLibrary::new(vfs).unwrap());
    let events: Vec<String> = log
        .lines()
        .filter_map(|l| l.strip_prefix(">>> event "))
        .map(str::to_owned)
        .collect();
    let insts = instances(&events);
    assert!(!insts.is_empty(), "no dialogs in the trace");
    let mut failed = Vec::new();
    for inst in &insts {
        let complete = inst
            .expected
            .last()
            .is_some_and(|e| e.contains("DialogEnded("));
        let names = inst
            .actors
            .iter()
            .map(|(_, g, n)| (*g, n.clone()))
            .collect();
        let s = Search {
            lib: lib.clone(),
            defs: &defs,
            events: &events,
            inst,
            names,
        };
        let mut budget = 20_000;
        let found = s.find(&mut budget);
        let flags = inst
            .expected
            .iter()
            .filter(|e| e.starts_with("FlagSet"))
            .count();
        eprintln!(
            "{} #{}: {} logged events ({flags} FlagSet, {} rolls){} → {}",
            inst.dialog,
            inst.id,
            inst.expected.len(),
            inst.rolls.len(),
            if complete {
                ""
            } else {
                ", log ends mid-dialog"
            },
            match &found {
                Some(p) => format!(
                    "reproduced, choices {:?}",
                    p.iter().map(|i| i + 1).collect::<Vec<_>>()
                ),
                None => "NOT reproduced".into(),
            }
        );
        if found.is_none() && complete {
            failed.push(inst.dialog.clone());
        }
    }
    assert!(failed.is_empty(), "not reproduced: {failed:?}");
}

#[test]
fn event_lines_split() {
    let (n, a) = split_event("FlagSet(A_00000000-0000-0000-0000-000000000001, NULL_00000000-0000-0000-0000-000000000000, 7)").unwrap();
    assert_eq!(n, "FlagSet");
    assert_eq!(a.len(), 3);
    assert_eq!(
        uuid_of(a[0]).unwrap().to_string(),
        "00000000-0000-0000-0000-000000000001"
    );
    assert!(matches!(
        DialogEvent::FlagSet {
            flag: Guid::default(),
            target: None,
            instance: 7
        }
        .osiris(&|g| g.to_string())
        .as_slice(),
        [s] if s == "FlagSet(00000000-0000-0000-0000-000000000000, NULL_00000000-0000-0000-0000-000000000000, 7)"
    ));
    let _ = FlagType::Global;
}
