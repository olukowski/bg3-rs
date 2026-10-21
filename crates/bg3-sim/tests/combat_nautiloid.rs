//! The Nautiloid's first fight (the imps on the lower deck, `TUT_Avernus_C`), headless on
//! bg3-sim with the story: Lae'zel and Shadowheart against `S_FirstFight_Imp_*`.
//!
//! Checked against the reference combat trace (`combat-20261021-183321`, recorded on
//! Balanced; not committed): the imps' initiative bonus (-21: Dexterity -1 and the story's
//! `TUT_LOWERDECK_INITIATIVEBOOST`), the imp's AC 7 and 6 HP, Lae'zel's 12 HP, the
//! fourth imp staying off stage with one player, the party acting first as one shared turn,
//! the combat start events, and the story reacting to the imps' deaths
//! (`TUT_LowerDeck_State_ImpsDead`). Needs the install; skips cleanly without it.

use std::sync::Arc;

use bg3_sim::{Game, GameData, Guid, Sim};
use bg3_story::Story;

const STORY: (&str, &str) = ("GustavX", "Mods/GustavX/Story/story.div.osi");
const LEVEL: &str = "TUT_Avernus_C";

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
    let data = GameData::load(&vfs, &stats, &[LEVEL]).expect("game data");
    Some((story, data))
}

fn find(sim: &Sim, name: &str) -> Guid {
    sim.entities
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.id)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
#[ignore = "needs BG3 install"]
fn nautiloid_imp_fight() {
    let Some((story, data)) = load() else {
        return;
    };
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || run(&story, data))
        .expect("spawn")
        .join()
        .expect("fight thread");
}

fn run(story: &[u8], data: GameData) {
    let story = Story::parse(story).expect("parse story");
    let mut sim = Sim::with_data(Arc::new(data));
    sim.load_level(LEVEL);
    sim.dice = bg3_sim::rules::SimDice::new(1);
    let laezel = find(&sim, "S_Player_Laezel");
    let shadowheart = find(&sim, "S_Player_ShadowHeart");
    let imps: Vec<Guid> = ["Melee_01", "Ranged_01", "Melee_02"]
        .iter()
        .map(|n| find(&sim, &format!("S_FirstFight_Imp_{n}")))
        .collect();
    let imp4 = find(&sim, "S_FirstFight_Imp_Ranged_02");
    let party = vec![laezel, shadowheart];
    sim.party = party.clone();
    let at = sim.entity(imps[0]).unwrap().position;
    for (i, p) in party.iter().enumerate() {
        sim.entities.get_mut(*p).unwrap().position = [at[0] - 3.0, at[1], at[2] + 1.5 * i as f32];
    }
    let mut game = Game::new(&story, sim);
    game.init();
    game.start_level(LEVEL);
    game.set_companion_factions(&party);

    // With one player the story keeps the fourth imp off stage (TUT_LowerDeck INIT).
    assert!(game.sim.combat.off_stage.contains(&imp4));
    // Character sheets: the trace's tooltips and probes.
    for &i in &imps {
        assert!(game.sim.ensure_combatant(i));
        let s = game.sim.entity(i).unwrap().sheet.as_ref().unwrap();
        assert_eq!((s.armor_class, s.max_hp), (7, 6), "imp AC and HP");
        assert_eq!(game.sim.initiative_bonus(i), -21, "imp initiative bonus");
    }
    assert!(game.sim.ensure_combatant(laezel));
    let s = game.sim.entity(laezel).unwrap().sheet.as_ref().unwrap();
    assert_eq!(s.max_hp, 12, "Lae'zel's HP");
    assert_eq!(
        game.sim
            .combatant(laezel)
            .unwrap()
            .resources
            .get("Movement"),
        9.0
    );

    let cid = game
        .sim
        .enter_combat(laezel, imps[0])
        .expect("fight starts");
    let names: Vec<String> = game.sim.pending_events().map(|e| e.name.clone()).collect();
    game.tick(0.0);
    let c = game.sim.combat_by_id(cid).unwrap();
    let mut parts = c.participants.clone();
    parts.sort_by_key(|g| g.0);
    let mut want: Vec<Guid> = party.iter().chain(&imps).copied().collect();
    want.sort_by_key(|g| g.0);
    assert_eq!(parts, want, "the party and the three imps, not the fourth");
    assert_eq!(c.order.len(), 4);
    let mut first = c.order[0].members.clone();
    first.sort_by_key(|g| g.0);
    let mut p2 = party.clone();
    p2.sort_by_key(|g| g.0);
    assert_eq!(first, p2, "the party acts first, as one shared turn");
    // The start of the trace's fight: EnteredCombat per participant, CombatStarted,
    // CombatRoundStarted, TurnStarted, ShareInitiative, TurnStarted.
    let fight: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| {
            [
                "EnteredCombat",
                "CombatStarted",
                "CombatRoundStarted",
                "TurnStarted",
                "ShareInitiative",
            ]
            .contains(n)
        })
        .collect();
    assert_eq!(
        fight,
        [
            "EnteredCombat",
            "EnteredCombat",
            "EnteredCombat",
            "EnteredCombat",
            "EnteredCombat",
            "CombatStarted",
            "CombatRoundStarted",
            "TurnStarted",
            "ShareInitiative",
            "TurnStarted"
        ]
    );

    // A cantrip with a saving throw, as the trace's cleric cast it: Sacred Flame (cantrip
    // choices are not modelled, so it is given to Shadowheart here).
    game.sim
        .combatant_mut(shadowheart)
        .unwrap()
        .spells
        .push("Target_SacredFlame".to_owned());
    game.sim.take_events();
    let report = game
        .sim
        .combat_cast(shadowheart, "Target_SacredFlame", imps[1])
        .expect("Sacred Flame");
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            bg3_rules::exec::ExecEvent::Roll(bg3_rules::natives::RollEvent::SavingThrow(s))
                if s.ability == bg3_rules::context::Ability::Dexterity
        )),
        "a Dexterity saving throw: {:?}",
        report.log
    );
    let ev: Vec<String> = game.sim.pending_events().map(|e| e.key()).collect();
    assert!(
        ev.iter()
            .any(|e| e.starts_with("UsingSpell(") && e.contains("\"target\", \"Evocation\"")),
        "{ev:?}"
    );
    assert!(ev.iter().any(|e| e.starts_with("AttackedBy(")), "{ev:?}");
    game.tick(0.0);

    let turns = game.combat_auto(cid, 200);
    let log = game.take_combat_events();
    let c = game.sim.combat_by_id(cid).unwrap();
    assert!(c.finished, "fight over after {} turns", turns.len());
    let won = imps.iter().all(|i| game.sim.combatant(*i).unwrap().dead);
    eprintln!(
        "{} turns, {} combat events, party won: {won}",
        turns.len(),
        log.len()
    );
    if won {
        let flag = game
            .sim
            .data
            .flags
            .iter()
            .find(|f| f.name == "TUT_LowerDeck_State_ImpsDead")
            .expect("flag")
            .id;
        assert_eq!(
            game.sim.get_flag(flag, Guid::NULL),
            Ok(true),
            "the story counted the imps' deaths"
        );
    }
}
