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

new entry "Projectile_T_Bolt"
type "SpellData"
data "SpellType" "Projectile"
data "TargetRadius" "18"
data "SpellRoll" "Attack(AttackType.RangedSpellAttack)"
data "SpellSuccess" "DealDamage(1d10,Fire)"
data "TargetConditions" "not Dead()"
data "UseCosts" "ActionPoint:1"
data "SpellFlags" "IsSpell;IsHarmful"

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
fn single_class_progression_does_not_add_multiclass_alternatives() {
    let mut s = sim(1);
    let class = Guid(101);
    let table = Guid(102);
    let d = Arc::make_mut(&mut s.data);
    d.combat.classes.insert(
        class,
        data::ClassDef {
            progression_table: Some(table),
            ..Default::default()
        },
    );
    d.combat.origins.insert(
        g(HERO1),
        data::OriginDef {
            class: Some(class),
            ..Default::default()
        },
    );
    d.combat.progressions.insert(
        table,
        vec![
            data::ProgressionDef {
                id: Guid(103),
                level: 1,
                boosts: "ActionResource(SpellSlot,2,1)".into(),
                ..Default::default()
            },
            data::ProgressionDef {
                id: Guid(104),
                level: 1,
                is_multiclass: true,
                boosts: "ActionResource(SpellSlot,2,1)".into(),
                ..Default::default()
            },
            data::ProgressionDef {
                id: Guid(105),
                level: 2,
                boosts: "ActionResource(SpellSlot,1,1);ActionResource(SpellSlot,2,2)".into(),
                ..Default::default()
            },
        ],
    );
    let c = s.build_combatant(g(HERO1)).unwrap();
    assert_eq!(c.resources.max_at_level("SpellSlot", 1), 2.0);
    assert_eq!(c.resources.max_at_level("SpellSlot", 2), 0.0);
}

#[test]
fn cast_api_spends_selected_slot_and_uses_installed_variant_expressions() {
    let mut s = sim(3);
    let d = Arc::make_mut(&mut s.data);
    d.combat
        .resource_groups
        .insert("SpellSlotsGroup".into(), vec!["SpellSlot".into()]);
    for (name, level, healing) in [("Target_TestHeal", 1, 1), ("Target_TestHeal_3", 3, 3)] {
        d.combat.spells.insert(
            name.into(),
            data::SpellDef {
                name: name.into(),
                spell_type: "Target".into(),
                target_radius: "20".into(),
                use_costs: data::parse_costs(&format!("ActionPoint:1;SpellSlotsGroup:1:1:{level}")),
                fields: vec![
                    ("Level".into(), level.to_string()),
                    (
                        "SpellProperties".into(),
                        format!("RegainHitPoints({healing})"),
                    ),
                ],
                ..Default::default()
            },
        );
    }
    s.ensure_combatant(g(HERO1));
    s.ensure_combatant(g(HERO2));
    s.entities
        .get_mut(g(HERO2))
        .unwrap()
        .sheet
        .as_mut()
        .unwrap()
        .hp = 1;
    let c = s.combatant_mut(g(HERO1)).unwrap();
    c.spells.push("Target_TestHeal".into());
    c.resources
        .entries
        .extend([1, 3].map(|level| ResourceAmount {
            name: "SpellSlot".into(),
            level,
            amount: 1.0,
            max: 1.0,
        }));
    s.combat_cast_with_slot(g(HERO1), "Target_TestHeal", g(HERO2), Some(3))
        .unwrap();
    let resources = &s.combatant(g(HERO1)).unwrap().resources;
    assert_eq!(resources.get_at_level("SpellSlot", 1), 1.0);
    assert_eq!(resources.get_at_level("SpellSlot", 3), 0.0);
    assert_eq!(s.entity(g(HERO2)).unwrap().sheet.as_ref().unwrap().hp, 4);
    let resources = resources.clone();
    assert!(
        s.combat_cast_with_slot(g(HERO1), "Target_TestHeal", g(HERO2), Some(3))
            .is_err()
    );
    assert_eq!(s.combatant(g(HERO1)).unwrap().resources, resources);
}

#[test]
fn story_resource_queries_spell_removal_and_tutorial_notifications() {
    use crate::{Entity, Origin, SimHost};
    use bg3_osiris::{Vm, builder::*};

    let mut sim = sim(1);
    sim.stage.live = true;
    let data = Arc::make_mut(&mut sim.data);
    data.combat.resources.push(data::ActionResourceDef {
        name: "SpellSlot".into(),
        id: Guid(100),
        replenish: data::Replenish::Rest,
        max_level: 9,
    });
    data.story.objects.entry(g(HERO1)).or_default().can_fight = Some(false);
    data.story
        .item_slots
        .insert("TestInstrument".into(), "MusicalInstrument".into());
    let item = Guid(200);
    let mut entity = Entity::new(item, "TestItem", EntityKind::Item, Origin::Data);
    entity.stats = Some("TestInstrument".into());
    sim.entities.insert(entity);
    let observed = |id: Guid| crate::ObservedArg {
        text: id.to_string(),
        ty: "GUIDSTRING".into(),
    };
    sim.observe("Equipped", &[observed(item), observed(g(HERO1))]);
    sim.ensure_combatant(g(HERO1));
    let c = sim.combatant_mut(g(HERO1)).unwrap();
    c.resources
        .entries
        .extend([1, 2].map(|level| ResourceAmount {
            name: "SpellSlot".into(),
            level,
            amount: level as f32,
            max: level as f32,
        }));
    assert!(c.resources.pay(&data::parse_costs("SpellSlot:1:2")));

    let hero = g(HERO1).to_string();
    let imp = g(IMP1).to_string();
    let mut b = StoryBuilder::new();
    b.event("Start", &[]);
    b.query(
        "GetActionResourceValuePersonal",
        &[GUIDSTRING, STRING, INT, REAL],
        &[false, false, false, true],
    );
    b.query("IsDead", &[GUIDSTRING, INT], &[false, true]);
    b.query(
        "IsEnemy",
        &[GUIDSTRING, GUIDSTRING, INT],
        &[false, false, true],
    );
    b.query("CanFight", &[GUIDSTRING, INT], &[false, true]);
    b.query(
        "GetEquippedItem",
        &[GUIDSTRING, STRING, GUIDSTRING],
        &[false, false, true],
    );
    b.call("RemoveSpell", &[GUIDSTRING, STRING, INT]);
    b.call("HideTutorial", &[GUIDSTRING, GUIDSTRING]);
    b.call("ShowTutorial", &[GUIDSTRING, GUIDSTRING]);
    b.db("DB_Resource", &[REAL]);
    b.db("DB_Equipped", &[GUIDSTRING]);
    for name in ["Dead", "Enemy", "CanFight"] {
        b.db(&format!("DB_{name}"), &[INT]);
    }
    let goal = b.goal("Main", &[]);
    b.rule(
        goal,
        Rule::when("Start", [])
            .and(
                "GetActionResourceValuePersonal",
                [guid(&hero), s("SpellSlot"), int(2), v("N")],
            )
            .then("DB_Resource", [v("N")]),
    );
    b.rule(
        goal,
        Rule::when("Start", [])
            .and(
                "GetEquippedItem",
                [guid(&hero), s("MusicalInstrument"), v("Item")],
            )
            .then("DB_Equipped", [v("Item")]),
    );
    b.rule(
        goal,
        Rule::when("Start", [])
            .and("IsDead", [guid(&hero), v("N")])
            .then("DB_Dead", [v("N")]),
    );
    b.rule(
        goal,
        Rule::when("Start", [])
            .and("IsEnemy", [guid(&hero), guid(&imp), v("N")])
            .then("DB_Enemy", [v("N")]),
    );
    b.rule(
        goal,
        Rule::when("Start", [])
            .and("CanFight", [guid(&hero), v("N")])
            .then("DB_CanFight", [v("N")]),
    );
    b.rule(
        goal,
        Rule::when("Start", [])
            .then(
                "RemoveSpell",
                [guid(&hero), s("Target_MainHandAttack"), int(1)],
            )
            .then("HideTutorial", [guid(&hero), guid(&imp)])
            .then("ShowTutorial", [guid(&hero), guid(&imp)]),
    );
    let story = b.build();
    let mut vm = Vm::new(&story);
    vm.init_game(&mut SimHost::new(&mut sim));
    vm.event(&mut SimHost::new(&mut sim), "Start", &[]);
    assert!(
        matches!(vm.db("DB_Resource", 1).unwrap()[0][0].data, bg3_osiris::Data::Real(n) if n == 1.0)
    );
    for (db, expected) in [("DB_Dead", 0), ("DB_Enemy", 1), ("DB_CanFight", 0)] {
        assert!(
            matches!(vm.db(db, 1).unwrap()[0][0].data, bg3_osiris::Data::Int(n) if n == expected)
        );
    }
    assert_eq!(
        Guid::from_osiris(vm.text(&vm.db("DB_Equipped", 1).unwrap()[0][0]).unwrap()),
        Some(item)
    );
    assert!(
        !sim.combatant(g(HERO1))
            .unwrap()
            .spells
            .iter()
            .any(|s| s == "Target_MainHandAttack")
    );
    assert!(
        !sim.build_combatant(g(HERO1))
            .unwrap()
            .spells
            .iter()
            .any(|s| s == "Target_MainHandAttack")
    );
    assert_eq!(
        sim.story
            .effects
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["HideTutorial", "ShowTutorial"]
    );
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
fn weapons_are_drawn_for_the_fight_by_spell_and_sheathed_after() {
    use bg3_world::unsheath::UnsheathState::{Melee, Ranged, Sheathed};
    let mut s = sim(3);
    assert_eq!(s.unsheath_state(g(HERO1)), Sheathed);
    let cid = s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    // Joining draws the melee set (the hero has a sword; an unarmed imp draws "melee" too).
    assert_eq!(s.unsheath_state(g(HERO1)), Melee);
    assert_eq!(s.unsheath_state(g(IMP1)), Melee);
    let ev = s.take_combat_events();
    let started = ev
        .iter()
        .position(|e| matches!(e, CombatEvent::CombatStarted { .. }))
        .unwrap();
    let drawn = ev
        .iter()
        .position(|e| {
            matches!(e, CombatEvent::Unsheathed { entity, state: Melee } if *entity == g(HERO1))
        })
        .unwrap();
    assert!(drawn > started);
    // A spell's `Sheathing` switches the state before the cast is shown.
    Arc::make_mut(&mut s.data)
        .combat
        .spells
        .get_mut("Target_MainHandAttack")
        .unwrap()
        .sheathing = "Ranged".into();
    s.combat_move(g(HERO1), 4.0, Some([4.0, 0.0, 0.0])).unwrap();
    s.combat_cast(g(HERO1), "Target_MainHandAttack", g(IMP1))
        .unwrap();
    assert_eq!(s.unsheath_state(g(HERO1)), Ranged);
    let ev = s.take_combat_events();
    let switched = ev
        .iter()
        .position(|e| matches!(e, CombatEvent::Unsheathed { state: Ranged, .. }))
        .unwrap();
    let cast = ev
        .iter()
        .position(|e| matches!(e, CombatEvent::SpellCast { .. }))
        .unwrap();
    assert!(switched < cast);
    // The end of the fight sheathes the survivors.
    for i in [IMP1, IMP2] {
        if let Some(c) = s.combat.combatants.get_mut(&g(i)) {
            c.dead = true;
        }
    }
    assert!(s.check_combat_end(cid));
    assert_eq!(s.unsheath_state(g(HERO1)), Sheathed);
    assert_eq!(s.unsheath_state(g(HERO2)), Sheathed);
}

#[test]
fn the_equipment_initial_weapon_set_is_drawn_for_the_fight() {
    use bg3_world::unsheath::{UnsheathState, WeaponSet};
    let mut s = sim(3);
    assert_eq!(s.weapon_set(g(HERO1)), WeaponSet::Melee);
    Arc::make_mut(&mut s.data)
        .combat
        .initial_weapon_sets
        .insert("EQP_T_Hero".to_owned(), WeaponSet::Ranged);
    assert_eq!(s.weapon_set(g(HERO1)), WeaponSet::Ranged);
    s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    assert_eq!(s.unsheath_state(g(HERO1)), UnsheathState::Ranged);
    assert_eq!(s.unsheath_state(g(IMP1)), UnsheathState::Melee);
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

struct WallPath;

impl Pathfinder for WallPath {
    fn find_path(&self, from: [f32; 3], to: [f32; 3], radius: f32) -> Option<Vec<[f32; 3]>> {
        assert_eq!(radius, 0.5);
        Some(vec![from, [from[0], 0.0, 4.0], [to[0], 0.0, 4.0], to])
    }
}

struct NoPath;

impl Pathfinder for NoPath {
    fn find_path(&self, _: [f32; 3], _: [f32; 3], _: f32) -> Option<Vec<[f32; 3]>> {
        None
    }
}

fn path_fight(pathfinder: Arc<dyn Pathfinder>) -> Sim {
    let mut s = sim(0);
    s.set_pathfinder(Some(pathfinder));
    s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    s
}

#[test]
fn ai_walks_around_wall_and_pays_polyline_length() {
    let mut s = path_fight(Arc::new(WallPath));
    s.combatant_mut(g(HERO1))
        .unwrap()
        .resources
        .entries
        .iter_mut()
        .find(|r| r.name == "Movement")
        .unwrap()
        .amount = 6.0;
    let action = s.ai_turn(g(HERO1)).unwrap();
    let AiAction::MovedOnly { moved, path, .. } = action else {
        panic!("cannot attack through wall")
    };
    assert_eq!(
        path,
        vec![[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [2.0, 0.0, 4.0]]
    );
    assert_eq!(s.entity(g(HERO1)).unwrap().position, [2.0, 0.0, 4.0]);
    assert_eq!(moved, 6.0);
    assert!(distance(path[0], *path.last().unwrap()) < moved);
    assert_eq!(
        s.combatant(g(HERO1)).unwrap().resources.get("Movement"),
        0.0
    );
    assert_eq!(
        s.combatant(g(HERO1)).unwrap().resources.get("ActionPoint"),
        1.0
    );
}

#[test]
fn ai_stops_on_path_in_attack_range() {
    let mut s = path_fight(Arc::new(WallPath));
    s.combatant_mut(g(HERO1))
        .unwrap()
        .resources
        .entries
        .iter_mut()
        .find(|r| r.name == "Movement")
        .unwrap()
        .amount = 20.0;
    let action = s.ai_turn(g(HERO1)).unwrap();
    let AiAction::Cast { moved, path, .. } = action else {
        panic!("attack after detour")
    };
    let end = *path.last().unwrap();
    assert_eq!(end[0], 5.0);
    assert!((end[2] - 1.45).abs() < 1e-4);
    assert!((moved - 11.55).abs() < 1e-4);
    assert!(
        (s.combatant(g(HERO1)).unwrap().resources.get("Movement") - (20.0 - moved)).abs() < 1e-4
    );
}

#[test]
fn ai_without_a_path_stays_put_and_ends_turn_but_can_attack_in_range() {
    let mut s = path_fight(Arc::new(NoPath));
    let from = s.entity(g(HERO1)).unwrap().position;
    // No route to anyone: the walk towards the imp doesn't move, which the AI rejects.
    let action = s.ai_turn(g(HERO1)).unwrap();
    assert_eq!(action, AiAction::Nothing);
    assert_eq!(s.entity(g(HERO1)).unwrap().position, from);
    assert_eq!(
        s.combatant(g(HERO1)).unwrap().resources.get("Movement"),
        9.0
    );
    assert!(!s.combat_of(g(HERO1)).unwrap().is_active(g(HERO1)));

    let mut s = path_fight(Arc::new(NoPath));
    s.entities.get_mut(g(HERO1)).unwrap().position = [4.0, 0.0, 0.0];
    assert!(matches!(
        s.ai_turn(g(HERO1)).unwrap(),
        AiAction::Cast { moved: 0.0, .. }
    ));
}

// ---- the scoring AI (our own archetype text, not the game's) -------------------------------

/// A fight with HERO1 on the AI, the imps placed around it, and `archetype` as `base`.
fn ai_fight(archetype: &str, imps: [[f32; 3]; 2]) -> Sim {
    let mut s = sim(0);
    Arc::make_mut(&mut s.data).combat.ai.add("base", archetype);
    s.entities.get_mut(g(HERO2)).unwrap().position = [-20.0, 0.0, 0.0];
    for (id, p) in [IMP1, IMP2].into_iter().zip(imps) {
        s.entities.get_mut(g(id)).unwrap().position = p;
    }
    s.enter_combat(g(HERO1), g(IMP1)).unwrap();
    s
}

const AI_BASE: &str = "SCORE_MOD 100\nMULTIPLIER_DAMAGE_ENEMY_POS 1\nMULTIPLIER_DAMAGE_SELF_NEG 1\n\
    MULTIPLIER_KILL_ENEMY 2\nMULTIPLIER_ACTION_RESOURCE_COST 0.01\n\
    MULTIPLIER_CANNOT_EXECUTE_THIS_TURN 0.2\nENABLE_MOVEMENT_AVOID_AOO 1\nBASE_NEARBY_SCORE 0.2\n\
    ENDPOS_ENEMIES_NEARBY_MIN_DISTANCE 2\nENDPOS_ENEMIES_NEARBY_MAX_DISTANCE 6\n";

#[test]
fn ai_hit_chance_is_exact_and_the_kill_wins() {
    let mut s = ai_fight(AI_BASE, [[1.2, 0.0, 0.0], [-1.2, 0.0, 0.0]]);
    // The expected hit (0.95 * 7) would kill a 6 HP imp: make the other one tougher.
    let sheet = s.entities.get_mut(g(IMP1)).unwrap().sheet.as_mut().unwrap();
    (sheet.max_hp, sheet.hp) = (20, 20);
    s.set_hitpoints(g(IMP2), 1);
    let plan = s.ai_plan(g(HERO1)).unwrap();
    let c = plan.chosen().expect("an attack");
    assert_eq!(c.target, g(IMP2), "the killing blow");
    assert!(c.breakdown.targets.iter().any(|t| t.kill));
    // d20 + 3 (Strength) + 2 (proficiency) against AC 7 (8, Dexterity -1): all but a 1.
    assert!((c.chance - 0.95).abs() < 1e-6, "{}", c.chance);
    // Stupidity 0 (not set): the chance counts as it is.
    assert_eq!(c.believed, c.chance);
}

#[test]
fn ai_walking_out_of_reach_provokes_and_costs() {
    let mut s = ai_fight(AI_BASE, [[1.2, 0.0, 0.0], [-6.0, 0.0, 0.0]]);
    let plan = s.ai_plan(g(HERO1)).unwrap();
    let stay = plan
        .candidates
        .iter()
        .find(|c| c.target == g(IMP1) && c.this_turn)
        .unwrap();
    let away = plan
        .candidates
        .iter()
        .find(|c| c.target == g(IMP2) && c.this_turn)
        .expect("the other imp is in reach");
    assert_eq!(stay.aoo_damage, 0.0);
    assert!(away.aoo_damage > 0.0, "leaving the imp's reach");
    assert!(away.score < stay.score);
    assert_eq!(plan.chosen().unwrap().target, g(IMP1));
}

#[test]
fn ai_ranged_archetype_steps_away_from_enemies() {
    let bolt_only = |s: &mut Sim| {
        let c = s.combatant_mut(g(HERO1)).unwrap();
        c.spells = vec!["Projectile_T_Bolt".to_owned()];
    };
    let ranged = format!("{AI_BASE}MULTIPLIER_ENDPOS_ENEMIES_NEARBY -0.5\n");
    let mut s = ai_fight(&ranged, [[1.2, 0.0, 0.0], [40.0, 0.0, 0.0]]);
    bolt_only(&mut s);
    // No attacks of opportunity: the imp has used its reaction.
    s.combatant_mut(g(IMP1))
        .unwrap()
        .resources
        .entries
        .retain(|r| r.name != "ReactionActionPoint");
    let from = s.entity(g(HERO1)).unwrap().position;
    let plan = s.ai_plan(g(HERO1)).unwrap();
    let c = plan.chosen().unwrap();
    assert_eq!(c.spell, "Projectile_T_Bolt");
    assert!(c.tile.cost > 0.0);
    let to_imp = |p: [f32; 3]| distance(p, [1.2, 0.0, 0.0]);
    assert!(to_imp(c.tile.position) > to_imp(from) + 3.0, "{:?}", c.tile);
    // The same fight with an archetype that likes enemies nearby stays put.
    let melee = format!("{AI_BASE}MULTIPLIER_ENDPOS_ENEMIES_NEARBY 0.5\n");
    let mut s = ai_fight(&melee, [[1.2, 0.0, 0.0], [40.0, 0.0, 0.0]]);
    bolt_only(&mut s);
    let c = s.ai_plan(g(HERO1)).unwrap().chosen().unwrap().clone();
    assert_eq!(c.tile.cost, 0.0);
}
