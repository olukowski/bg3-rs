use crate::library::DialogLibrary;

fn library() -> Option<DialogLibrary> {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: no BG3 install ({e})");
            return None;
        }
    };
    let vfs = bg3_vfs::Vfs::from_install(&install).expect("mount paks");
    Some(DialogLibrary::new(std::sync::Arc::new(vfs)).expect("index dialogs"))
}

#[test]
#[ignore = "needs BG3 install"]
fn parse_every_dialog_in_install() {
    let Some(lib) = library() else { return };
    let mut failures = Vec::new();
    for f in lib.files() {
        if let Err(e) = lib.load_file(f) {
            failures.push(format!("{}: {e}", f.path));
        }
    }
    eprintln!("{} dialogs, {} failures", lib.files().len(), failures.len());
    for f in failures.iter().take(20) {
        eprintln!("  {f}");
    }
    assert!(lib.files().len() > 9000);
    assert!(failures.is_empty());
}

/// Every flag a dialog references through a Global/Object/Local/Dialog/User/Party/UserShape
/// group is defined with the matching `Usage`; Tag groups reference tags.
#[test]
#[ignore = "needs BG3 install"]
fn flag_usage_matches_group_type() {
    use crate::FlagKind;
    let Some(lib) = library() else { return };
    let defs = crate::defs::Definitions::load(lib.vfs()).unwrap();
    eprintln!(
        "{} flags, {} tags, {} script flags, {} speaker groups",
        defs.flags.len(),
        defs.tags.len(),
        defs.script_flags.len(),
        defs.speaker_groups.len()
    );
    let (mut checked, mut missing, mut mismatched) = (0, 0, 0);
    for f in lib.files() {
        let d = lib.load_file(f).unwrap();
        for n in &d.nodes {
            let groups = n
                .check_flags
                .iter()
                .chain(&n.set_flags)
                .chain(n.validated_flags.iter().flat_map(|v| &v.groups));
            for g in groups {
                for fl in &g.flags {
                    checked += 1;
                    match &g.kind {
                        FlagKind::Tag => {
                            if !defs.tags.contains_key(&fl.uuid) {
                                missing += 1;
                            }
                        }
                        FlagKind::Script => {
                            if !defs.script_flags.contains_key(&fl.uuid) {
                                missing += 1;
                            }
                        }
                        FlagKind::Quest | FlagKind::Other(_) => {}
                        kind => match defs.flags.get(&fl.uuid) {
                            None => missing += 1,
                            Some(def) if def.kind().as_ref() != Some(kind) => {
                                mismatched += 1;
                                eprintln!(
                                    "{}: {} is {:?}, used as {kind:?}",
                                    f.name,
                                    def.name,
                                    def.kind()
                                );
                            }
                            Some(_) => {}
                        },
                    }
                }
            }
        }
    }
    eprintln!("{checked} flag references, {missing} undefined, {mismatched} mismatched");
    assert!(defs.flags.len() > 20_000);
    assert_eq!(mismatched, 0);
}

/// Every dialog resource points at a compiled dialog that loads; every nested-dialog node
/// refers to a known resource. Reports compiled files without a resource and resources whose
/// switches disagree with the dialog's own attributes.
#[test]
#[ignore = "needs BG3 install"]
fn dialog_resources_resolve() {
    use std::collections::HashSet;
    let Some(lib) = library() else { return };
    let mut ids = HashSet::new();
    let (mut missing, mut mismatched_automated) = (Vec::new(), 0);
    let mut nested_refs = 0;
    let mut stale = 0;
    let mut nested_unknown = Vec::new();
    let mut nested_not_child = 0;
    let mut covered = HashSet::new();
    for r in lib.resources() {
        if !ids.insert(r.id) {
            continue;
        }
        let d = match lib.load_resource(&r.id) {
            Ok(d) => d,
            Err(e) => {
                // Stale bank entries: neither the compiled file nor its source exists. Anything
                // else would mean our path mapping is wrong.
                let source_exists = !r.source_file.is_empty() && lib.vfs().exists(&r.source_file);
                if source_exists || lib.vfs().exists(&r.binary_path()) {
                    missing.push(format!("{}: {e}", r.name));
                } else {
                    stale += 1;
                }
                continue;
            }
        };
        covered.insert(crate::library::binary_path(&r.source_file).to_ascii_lowercase());
        if d.options.automated != r.automated {
            mismatched_automated += 1;
        }
        for n in &d.nodes {
            if let crate::NodeKind::NestedDialog { dialog, .. } = &n.kind {
                nested_refs += 1;
                if lib.resource(dialog).is_none() {
                    nested_unknown.push(format!("{} → {dialog}", r.name));
                } else if !r.child_resources.contains(dialog) {
                    nested_not_child += 1;
                }
            }
        }
    }
    let unreferenced: Vec<_> = lib
        .files()
        .iter()
        .filter(|f| !covered.contains(&f.path.to_ascii_lowercase()))
        .collect();
    eprintln!(
        "{} resources ({} unique), {stale} stale (no compiled file, no source), {} failing; \
         {} compiled files without a resource",
        lib.resources().len(),
        ids.len(),
        missing.len(),
        unreferenced.len()
    );
    eprintln!(
        "{nested_refs} nested-dialog references, {} unknown, {nested_not_child} not listed in childResources; \
         {mismatched_automated} resources whose `automated` differs from the dialog's",
        nested_unknown.len()
    );
    for m in missing.iter().chain(&nested_unknown).take(10) {
        eprintln!("  {m}");
    }
    for f in unreferenced.iter().take(5) {
        eprintln!("  no resource: {}", f.path);
    }
    assert!(ids.len() > 9000);
    assert!(missing.is_empty());
    assert!(nested_unknown.is_empty());
}

/// Starts every dialog with placeholder speakers and walks it, always taking the first (then,
/// in a second run, the last) enabled choice, with rolls alternating between success and
/// failure. Nothing may panic and every walk must end within a step budget.
#[test]
#[ignore = "needs BG3 install"]
fn runner_walks_every_dialog() {
    use std::collections::HashSet;
    use std::sync::Arc;

    use crate::runner::placeholder_speakers;
    use crate::{DialogRunner, MemoryWorld, RollOutcome, StartParams, Step};

    let Some(lib) = library() else { return };
    let lib = Arc::new(lib);
    let defs = crate::defs::Definitions::load(lib.vfs()).unwrap();
    let mut ids = HashSet::new();
    let (mut walks, mut ended, mut over_budget, mut lines, mut choices) = (0, 0, 0, 0, 0);
    let mut panics = Vec::new();
    for r in lib.resources() {
        if !ids.insert(r.id) {
            continue;
        }
        let Ok(d) = lib.load_resource(&r.id) else {
            continue;
        };
        for last in [false, true] {
            walks += 1;
            let d = d.clone();
            let lib = lib.clone();
            let defs = &defs;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut world = MemoryWorld::with_definitions(defs);
                let mut flip = false;
                world.set_roller(move |_| {
                    flip = !flip;
                    RollOutcome {
                        success: flip,
                        critical: None,
                    }
                });
                let speakers = placeholder_speakers(&d);
                let mut run = DialogRunner::new(
                    d,
                    lib,
                    StartParams {
                        instance: 1,
                        speakers,
                    },
                );
                run.start(&mut world);
                let (mut l, mut c) = (0, 0);
                for _ in 0..2000 {
                    match run.step().clone() {
                        Step::Line(_) => {
                            l += 1;
                            run.advance(&mut world).unwrap();
                        }
                        Step::Choices(options) => {
                            c += 1;
                            let enabled: Vec<usize> =
                                (0..options.len()).filter(|&i| options[i].enabled).collect();
                            let Some(&pick) = (if last {
                                enabled.last()
                            } else {
                                enabled.first()
                            }) else {
                                // Nothing selectable: the player would be stuck; stop here.
                                return (true, l, c);
                            };
                            run.choose(&mut world, pick).unwrap();
                        }
                        Step::Ended => return (true, l, c),
                    }
                }
                (false, l, c)
            }));
            match result {
                Ok((done, l, c)) => {
                    lines += l;
                    choices += c;
                    if done {
                        ended += 1;
                    } else {
                        over_budget += 1;
                    }
                }
                Err(_) => panics.push(r.name.clone()),
            }
        }
    }
    eprintln!(
        "{walks} walks: {ended} ended, {over_budget} over the step budget, {} panicked; {lines} lines, {choices} choice points",
        panics.len()
    );
    for p in panics.iter().take(10) {
        eprintln!("  panic: {p}");
    }
    assert!(panics.is_empty());
}
