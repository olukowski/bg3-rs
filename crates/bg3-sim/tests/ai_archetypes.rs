//! The AI archetypes load from the install and resolve through their parents.
//! Needs the game install; skips cleanly without it.

use bg3_sim::combat::ai::archetype::{
    Archetypes, BASE, DEFAULT_LETHALITY, MODIFIER_HIT_CHANCE_STUPIDITY, SCORE_MOD,
};

#[test]
#[ignore = "needs BG3 install"]
fn archetypes_load_and_resolve() {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let vfs = bg3_vfs::Vfs::from_install(&install).expect("vfs");
    let a = Archetypes::load(&vfs, bg3_vfs::modules::BASE_LOAD_ORDER, DEFAULT_LETHALITY);
    for name in [BASE, "melee", "ranged", "imp_melee", "imp_ranged"] {
        assert!(a.defs.contains_key(name), "{name}");
    }
    let base = a.resolve(BASE);
    assert!(base.get(SCORE_MOD) > 0.0);
    assert!(base.values.len() > 100, "{} modifiers", base.values.len());
    // The imps build on melee / ranged, which build on base: every base key is there.
    for imp in ["imp_melee", "imp_ranged"] {
        let m = a.resolve(imp);
        assert_eq!(m.archetype, imp);
        assert!(
            base.values.keys().all(|k| m.values.contains_key(k)),
            "{imp}"
        );
        let s = m.get(MODIFIER_HIT_CHANCE_STUPIDITY);
        assert!((0.0..=2.0).contains(&s), "{imp}: {s}");
    }
    // `melee` adjusts base's hit-chance reasoning with a relative entry.
    assert_ne!(
        a.resolve("melee").get(MODIFIER_HIT_CHANCE_STUPIDITY),
        base.get(MODIFIER_HIT_CHANCE_STUPIDITY)
    );
}
