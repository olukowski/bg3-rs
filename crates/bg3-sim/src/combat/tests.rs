//! Install-free combat tests on synthetic data (our own stats text, not the game's).

use std::sync::Arc;

use super::data::{CombatData, ObjectCombat};
use super::*;
use crate::sim::{GameData, PlacedObject, TemplateInfo};

const HERO1: u128 = 0x1000_0000_0000_0000_0000_0000_0000_0001;
const HERO2: u128 = 0x1000_0000_0000_0000_0000_0000_0000_0002;
const IMP1: u128 = 0x2000_0000_0000_0000_0000_0000_0000_0001;
const IMP2: u128 = 0x2000_0000_0000_0000_0000_0000_0000_0002;
const TMPL: u128 = 0x3000_0000_0000_0000_0000_0000_0000_0001;
const F_HERO: u128 = 0x4000_0000_0000_0000_0000_0000_0000_0001;
const F_IMP: u128 = 0x4000_0000_0000_0000_0000_0000_0000_0002;

const STATS: &str = r#"
new entry "_TBase"
type "Character"
data "Level" "1"
data "Strength" "10"
data "Dexterity" "10"
data "Vitality" "5"
data "Armor" "10"
data "ActionResources" "ActionPoint:1;BonusActionPoint:1;Movement:9;ReactionActionPoint:1"

new entry "T_Hero"
type "Character"
using "_TBase"
data "Strength" "16"
data "Dexterity" "14"
data "Vitality" "30"
data "Proficiency Group" "SimpleWeapons;MartialWeapons"

new entry "T_Imp"
type "Character"
using "_TBase"
data "Strength" "5"
data "Dexterity" "8"
data "Vitality" "6"
data "Armor" "8"
data "UnarmedAttackAbility" "Dexterity"

new entry "WPN_T_Sword"
type "Weapon"
data "Damage" "1d8"
data "Damage Type" "Slashing"
data "WeaponRange" "150"
data "Slot" "Melee Main Weapon"
data "Weapon Properties" "Melee"
data "Proficiency Group" "Longswords;MartialWeapons"

new entry "ARM_T_Chain"
type "Armor"
data "ArmorClass" "16"
data "Armor Class Ability" "None"
data "Slot" "Breast"

new entry "ARM_T_Leather"
type "Armor"
data "ArmorClass" "11"
data "Armor Class Ability" "Dexterity"
data "Ability Modifier Cap" "1"
data "Slot" "Breast"

new entry "ARM_T_Shield"
type "Armor"
data "ArmorClass" "2"
data "Shield" "Yes"
data "Slot" "Melee Offhand Weapon"

new entry "Target_MainHandAttack"
type "SpellData"
data "SpellType" "Target"
data "TargetRadius" "MeleeMainWeaponRange"
data "SpellRoll" "Attack(AttackType.MeleeWeaponAttack)"
data "SpellSuccess" "DealDamage(max(1,MainMeleeWeapon), MainMeleeWeaponDamageType)"
data "TargetConditions" "not Dead()"
data "UseCosts" "ActionPoint:1"
data "SpellFlags" "IsAttack;IsMelee;IsHarmful"

new entry "Target_T_Claws"
type "SpellData"
data "SpellType" "Target"
data "TargetRadius" "1.5"
data "SpellRoll" "Attack(AttackType.MeleeUnarmedAttack)"
data "SpellSuccess" "DealDamage(max(1,1d4+UnarmedMeleeAbilityModifier),Slashing)"
data "TargetConditions" "not Dead()"
data "UseCosts" "ActionPoint:1"
data "SpellFlags" "IsAttack;IsMelee;IsHarmful"

new entry "T_SLOW_START"
type "StatusData"
data "StatusType" "BOOST"
data "Boosts" "Initiative(-20)"

new equipment "EQP_T_Hero"
add initialweaponset "Melee"
add equipmentgroup
add equipment entry "WPN_T_Sword"
add equipmentgroup
add equipment entry "ARM_T_Chain"
"#;

fn g(n: u128) -> Guid {
    Guid(n)
}

fn placed(id: u128, name: &str, stats: &str, faction: u128, x: f32) -> PlacedObject {
    PlacedObject {
        id: g(id),
        name: name.to_owned(),
        kind: EntityKind::Character,
        template: Some(g(TMPL)),
        level: Some("TestLevel".to_owned()),
        global: true,
        position: [x, 0.0, 0.0],
        tags: Vec::new(),
        stats: Some(stats.to_owned()),
        statuses: Vec::new(),
        race: None,
        faction: Some(g(faction)),
        level_override: None,
    }
}

fn data() -> GameData {
    let mut stats = bg3_stats::Stats::new();
    stats.load_str("Test", "Test.txt", STATS).unwrap();
    stats.finish();
    let mut d = GameData {
        characters: crate::character::character_defs(&stats),
        ..GameData::default()
    };
    let mut c = CombatData::new();
    c.add_stats(&stats);
    c.factions.relations.insert((g(F_IMP), g(F_HERO)), 0);
    c.factions.relations.insert((g(F_HERO), g(F_IMP)), 0);
    for h in [HERO1, HERO2] {
        c.objects.insert(
            g(h),
            ObjectCombat {
                equipment: Some("EQP_T_Hero".to_owned()),
                ..Default::default()
            },
        );
    }
    for i in [IMP1, IMP2] {
        c.objects.insert(
            g(i),
            ObjectCombat {
                skills: vec!["Target_T_Claws".to_owned()],
                combat_group: Some("imps".to_owned()),
                ..Default::default()
            },
        );
    }
    d.combat = c;
    d.templates.insert(
        g(TMPL),
        TemplateInfo {
            id: g(TMPL),
            name: "T_Base".to_owned(),
            kind: EntityKind::Character,
            parent: None,
            tags: Vec::new(),
            race: None,
            stats: None,
            statuses: Vec::new(),
            faction: None,
            level_override: None,
        },
    );
    d.globals = vec![
        placed(HERO1, "S_Hero1", "T_Hero", F_HERO, 0.0),
        placed(HERO2, "S_Hero2", "T_Hero", F_HERO, 1.0),
        placed(IMP1, "S_Imp1", "T_Imp", F_IMP, 5.0),
        placed(IMP2, "S_Imp2", "T_Imp", F_IMP, 12.0),
    ];
    d
}

fn sim(seed: u64) -> Sim {
    let mut s = Sim::with_data(Arc::new(data()));
    s.party = vec![g(HERO1), g(HERO2)];
    s.dice = crate::rules::SimDice::new(seed);
    s.take_events();
    s
}

fn names(s: &mut Sim) -> Vec<String> {
    s.take_events().iter().map(|e| e.name.clone()).collect()
}

#[test]
fn combatant_setup_and_armor_class() {
    let mut s = sim(1);
    assert!(s.ensure_combatant(g(HERO1)));
    let c = s.combatant(g(HERO1)).unwrap();
    assert_eq!(c.equipment.melee_main.as_deref(), Some("WPN_T_Sword"));
    assert_eq!(c.equipment.body.as_deref(), Some("ARM_T_Chain"));
    assert_eq!(c.spells, ["Target_MainHandAttack"]);
    assert_eq!(c.resources.get("Movement"), 9.0);
    // Chain: 16, no ability.
    assert_eq!(
        s.entity(g(HERO1))
            .unwrap()
            .sheet
            .as_ref()
            .unwrap()
            .armor_class,
        16
    );
    // Leather (11 + Dex, capped at +1) and a shield (+2).
    let c = s.combatant_mut(g(HERO1)).unwrap();
    c.equipment.body = Some("ARM_T_Leather".to_owned());
    c.equipment.shield = Some("ARM_T_Shield".to_owned());
    s.update_armor_class(g(HERO1));
    assert_eq!(
        s.entity(g(HERO1))
            .unwrap()
            .sheet
            .as_ref()
            .unwrap()
            .armor_class,
        14
    );
    // No armour: Armor 8 + Dex modifier (-1).
    assert!(s.ensure_combatant(g(IMP1)));
    assert_eq!(
        s.entity(g(IMP1))
            .unwrap()
            .sheet
            .as_ref()
            .unwrap()
            .armor_class,
        7
    );
    let c = s.combatant(g(IMP1)).unwrap();
    assert_eq!(c.spells, ["Target_T_Claws"]);
    assert_eq!(c.unarmed_ability, Ability::Dexterity);
}

#[test]
fn relations_and_initiative_bonus() {
    let mut s = sim(1);
    assert!(s.is_hostile(g(HERO1), g(IMP1)));
    assert!(!s.is_hostile(g(HERO1), g(HERO2)));
    assert_eq!(s.relation(g(IMP1), g(IMP2)), data::RELATION_ALLIED);
    s.ensure_combatant(g(HERO1));
    s.ensure_combatant(g(IMP1));
    assert_eq!(s.initiative_bonus(g(HERO1)), 2);
    assert_eq!(s.initiative_bonus(g(IMP1)), -1);
    s.apply_status(g(IMP1), "T_SLOW_START", -1.0, Guid::NULL);
    assert_eq!(s.initiative_bonus(g(IMP1)), -21);
    for _ in 0..20 {
        let r = s.roll_initiative(g(HERO1));
        assert!((3..=6).contains(&r), "1d4 + 2, got {r}");
    }
}

#[test]
fn turn_order_sorts_and_groups_players() {
    let mut s = sim(1);
    for x in [HERO1, HERO2, IMP1, IMP2] {
        s.ensure_combatant(g(x));
    }
    let set = |s: &mut Sim, x: u128, i: i32| s.combatant_mut(g(x)).unwrap().initiative = i;
    set(&mut s, HERO1, 5);
    set(&mut s, HERO2, 3);
    set(&mut s, IMP1, 4);
    set(&mut s, IMP2, 3);
    let order = s.build_order(&[g(IMP2), g(HERO2), g(IMP1), g(HERO1)]);
    let flat: Vec<Guid> = order.iter().flat_map(|t| t.members.clone()).collect();
    // HERO2 and IMP2 tie at 3: the higher Dexterity (14 vs 8) goes first.
    assert_eq!(flat, [g(HERO1), g(IMP1), g(HERO2), g(IMP2)]);
    assert_eq!(order.len(), 4, "players separated by an imp don't share");
    set(&mut s, IMP1, 2);
    let order = s.build_order(&[g(IMP2), g(HERO2), g(IMP1), g(HERO1)]);
    assert_eq!(
        order[0].members,
        [g(HERO1), g(HERO2)],
        "adjacent players share"
    );
    assert_eq!(order[0].share, Share::Player);
    assert_eq!(order[1].members, [g(IMP2)]);
    assert_eq!(
        order[2].members,
        [g(IMP1)],
        "non-players never share (no swarms)"
    );
    // Equal initiative and Dexterity: player first, then the lower UUID.
    set(&mut s, IMP1, 3);
    s.party.retain(|x| *x != g(HERO2));
    let order = s.build_order(&[g(IMP2), g(HERO2), g(IMP1)]);
    let flat: Vec<Guid> = order.iter().flat_map(|t| t.members.clone()).collect();
    assert_eq!(flat, [g(HERO2), g(IMP1), g(IMP2)]);
}

#[test]
fn enter_combat_pulls_in_group_and_allies_and_raises_events() {
    let mut s = sim(7);
    let cid = s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    let c = s.combat_by_id(cid).unwrap();
    let mut parts = c.participants.clone();
    parts.sort_by_key(|x| x.0);
    assert_eq!(parts, [g(HERO1), g(HERO2), g(IMP1), g(IMP2)]);
    assert_eq!(c.round, 1);
    // Heroes (1d4+2 ≥ 3, Dex 14) always act before imps (1d4-1 ≤ 3, Dex 8), together.
    assert_eq!(c.order[0].members.len(), 2);
    let ev = names(&mut s);
    assert_eq!(ev.iter().filter(|e| *e == "EnteredCombat").count(), 4);
    let start = ev.iter().position(|e| e == "CombatStarted").unwrap();
    assert_eq!(ev[start + 1], "CombatRoundStarted");
    // As in the reference trace: the shared group starts, ShareInitiative after all but the
    // last member in round 1.
    assert_eq!(
        ev[start + 2..start + 5],
        ["TurnStarted", "ShareInitiative", "TurnStarted"]
    );
    assert!(s.in_combat(g(IMP2)));
}

#[test]
fn actions_check_turn_range_and_resources() {
    let mut s = sim(3);
    let cid = s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    s.take_combat_events();
    assert_eq!(
        s.combat_cast(g(IMP1), "Target_T_Claws", g(HERO1)),
        Err(CombatError::NotYourTurn)
    );
    assert!(matches!(
        s.combat_cast(g(HERO1), "Target_MainHandAttack", g(IMP1)),
        Err(CombatError::OutOfRange { .. })
    ));
    assert_eq!(
        s.combat_move(g(HERO1), 20.0, None),
        Err(CombatError::NotEnoughResources("Movement".into()))
    );
    s.combat_move(g(HERO1), 4.0, Some([4.0, 0.0, 0.0])).unwrap();
    assert_eq!(
        s.combatant(g(HERO1)).unwrap().resources.get("Movement"),
        5.0
    );
    let r = s
        .combat_cast(g(HERO1), "Target_MainHandAttack", g(IMP1))
        .unwrap();
    assert!(!r.log.is_empty());
    assert_eq!(
        s.combatant(g(HERO1)).unwrap().resources.get("ActionPoint"),
        0.0
    );
    if s.in_combat(g(HERO1)) {
        assert_eq!(
            s.combat_cast(g(HERO1), "Target_MainHandAttack", g(IMP1)),
            Err(CombatError::NotEnoughResources("ActionPoint".into()))
        );
    }
    let ev = s.take_combat_events();
    assert!(ev.iter().any(|e| matches!(e, CombatEvent::Roll { .. })));
    let names = names(&mut s);
    assert!(names.contains(&"UsingSpellOnTarget".to_owned()));
    assert!(names.contains(&"CastedSpell".to_owned()));
    let _ = cid;
}

/// Heroes attack the nearest imp each turn, the AI plays the imps, until the fight ends.
fn fight(seed: u64) -> (Sim, Vec<String>) {
    let mut s = sim(seed);
    let cid = s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    let mut events = names(&mut s);
    for _ in 0..200 {
        if s.combat_by_id(cid).unwrap().finished {
            break;
        }
        s.combat_run_ai(cid, 16);
        let Some(c) = s.combat_by_id(cid).filter(|c| !c.finished) else {
            break;
        };
        let up: Vec<Guid> = c
            .current_group()
            .unwrap()
            .members
            .iter()
            .copied()
            .filter(|m| c.is_active(*m))
            .collect();
        for h in up {
            if let Some(&(t, d)) = s.hostiles_by_distance(h).first() {
                let step = (d - 1.4).clamp(0.0, 9.0);
                if step > 0.0 {
                    let to = s.entity(t).unwrap().position;
                    let from = s.entity(h).unwrap().position;
                    let k = step / d;
                    let p = [
                        from[0] + (to[0] - from[0]) * k,
                        from[1],
                        from[2] + (to[2] - from[2]) * k,
                    ];
                    s.combat_move(h, step, Some(p)).ok();
                }
                s.combat_cast(h, "Target_MainHandAttack", t).ok();
            }
            if s.combat_of(h).is_some_and(|c| c.is_active(h)) {
                s.combat_end_turn(h).unwrap();
            }
        }
        events.extend(names(&mut s));
    }
    events.extend(names(&mut s));
    (s, events)
}

#[test]
fn a_fight_runs_to_the_end_deterministically() {
    let (s, ev) = fight(11);
    assert!(s.combat.combats[0].finished, "fight ended");
    assert!(ev.contains(&"CombatEnded".to_owned()));
    let imps_dead = [IMP1, IMP2]
        .iter()
        .all(|i| s.combatant(g(*i)).unwrap().dead);
    let heroes_down = [HERO1, HERO2]
        .iter()
        .all(|h| s.combatant(g(*h)).unwrap().downed || s.combatant(g(*h)).unwrap().dead);
    assert!(imps_dead || heroes_down);
    if imps_dead {
        assert_eq!(ev.iter().filter(|e| *e == "Died").count(), 2);
        let dying = ev.iter().position(|e| e == "Dying").unwrap();
        // The order of the reference trace (combat-20261021-183321).
        let tail: Vec<&str> = ev[dying..].iter().map(String::as_str).take(8).collect();
        assert_eq!(
            tail,
            [
                "Dying",
                "KilledBy",
                "CastedSpell",
                "StatusAttempt",
                "LeftCombat",
                "AttackedBy",
                "StatusApplied",
                "Died"
            ][..tail.len()]
        );
    }
    assert!(ev.contains(&"AttackedBy".to_owned()));
    assert!(ev.contains(&"TurnEnded".to_owned()));
    let (_, again) = fight(11);
    assert_eq!(ev, again, "same seed, same fight");
}

#[test]
fn hostiles_in_sight_start_a_fight() {
    let mut s = sim(3);
    for (id, x) in [(IMP1, 30.0), (IMP2, 40.0)] {
        s.entities.get_mut(g(id)).unwrap().position = [x, 0.0, 0.0];
    }
    assert_eq!(s.start_combat_on_sight(), None, "out of sight (16 m)");
    s.entities.get_mut(g(IMP1)).unwrap().position = [10.0, 0.0, 0.0];
    let cid = s.start_combat_on_sight().expect("the imp sees the party");
    let mut parts = s.combat_by_id(cid).unwrap().participants.clone();
    parts.sort_by_key(|p| p.0);
    // The second imp joins through the combat group, the second hero as an ally nearby.
    assert_eq!(parts, [g(HERO1), g(HERO2), g(IMP1), g(IMP2)]);
    assert_eq!(s.start_combat_on_sight(), None, "everyone is in the fight");
}
