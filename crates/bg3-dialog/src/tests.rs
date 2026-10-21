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
    Some(DialogLibrary::new(std::sync::Arc::new(vfs)))
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
