//! Install-backed data checks; no game content or extracted fixtures are committed.
use bg3_sim::{GameData, Sim};

#[test]
#[ignore = "needs BG3 install"]
fn imp_names_and_shadowheart_level_one_slots() {
    let install = match bg3_install::Install::locate() {
        Ok(install) => install,
        Err(error) => {
            eprintln!("skipping: {error}");
            return;
        }
    };
    let vfs = bg3_vfs::Vfs::from_install(&install).unwrap();
    let paks: Vec<_> = install
        .paks()
        .unwrap()
        .iter()
        .map(|path| bg3_pak::Package::open(path).unwrap())
        .collect();
    let stats = bg3_stats::install::load(&paks, bg3_stats::install::BASE_LOAD_ORDER).unwrap();
    let data = GameData::load(&vfs, &stats, &["TUT_Avernus_C"]).unwrap();
    let mut sim = Sim::new(data);
    sim.load_level("TUT_Avernus_C");
    let imps: Vec<_> = sim
        .entities
        .iter()
        .filter(|e| e.name.starts_with("S_FirstFight_Imp_"))
        .map(|e| (e.id, e.name.clone()))
        .collect();
    assert!(!imps.is_empty());
    for (id, object_name) in imps {
        let display = sim.display_name(id).expect("translated imp name");
        assert!(!display.trim().is_empty());
        assert_ne!(display, object_name);
    }
    let shadowheart = sim
        .entities
        .iter()
        .find(|e| e.name == "S_Player_ShadowHeart")
        .unwrap()
        .id;
    assert_eq!(
        sim.entity(shadowheart)
            .unwrap()
            .sheet
            .as_ref()
            .unwrap()
            .level,
        1
    );
    assert!(sim.ensure_combatant(shadowheart));
    let resource = sim.data.combat.resource("SpellSlot").unwrap();
    assert_eq!(resource.max_level, 9);
    let slots = &sim.combatant(shadowheart).unwrap().resources;
    assert_eq!(
        slots.max_at_level("SpellSlot", 1),
        2.0,
        "Cleric L1 progression grants two"
    );
    assert_eq!(slots.get_at_level("SpellSlot", 1), 2.0);
    for level in 2..=9 {
        assert_eq!(slots.max_at_level("SpellSlot", level), 0.0);
    }
}
