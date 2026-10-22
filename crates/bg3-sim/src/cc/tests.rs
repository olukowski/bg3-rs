//! Character creation on synthetic data (made up here, shaped like the game's).

use bg3_rules::context::{Ability, Skill};

use super::*;
use crate::guid::Guid;

fn g(n: u128) -> Guid {
    Guid(n)
}

const RACE: u128 = 0x10;
const SUBRACE: u128 = 0x11;
const OTHER_SUB: u128 = 0x12;
const CLASS: u128 = 0x20;
const SUBCLASS: u128 = 0x21;
const PRIEST: u128 = 0x22;
const BACKGROUND: u128 = 0x30;
const GOD: u128 = 0x40;
const BANNED_GOD: u128 = 0x41;
const T_RACE: u128 = 0x100;
const T_SUB: u128 = 0x101;
const T_CLASS: u128 = 0x102;
const T_SUBCLASS: u128 = 0x103;
const T_PRIEST: u128 = 0x104;
const P_CLASS: u128 = 0x200;
const P_SUBCLASS: u128 = 0x201;
const P_SUB: u128 = 0x202;
const L_SKILLS: u128 = 0x300;
const L_CANTRIPS: u128 = 0x301;
const L_ABILITIES: u128 = 0x302;
const L_STYLES: u128 = 0x303;
const L_GRANTED: u128 = 0x304;
const TAG_BG: u128 = 0x400;
const TAG_CLASS: u128 = 0x401;
const TAG_GOD: u128 = 0x402;
const TAG_GENERIC: u128 = 0x403;
const TEMPLATE: u128 = 0x500;

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn row(id: u128, table: u128, boosts: &str, passives: &[&str], selectors: &str) -> Progression {
    Progression {
        id: g(id),
        table: g(table),
        name: format!("T{table:x}"),
        level: 1,
        boosts: boosts.to_owned(),
        passives_added: strings(passives),
        selectors: rules::parse_selectors(selectors),
        ..Default::default()
    }
}

fn item(name: &str, kind: &str, slot: &str) -> CcItem {
    CcItem {
        name: name.to_owned(),
        kind: kind.to_owned(),
        slot: slot.to_owned(),
        ..Default::default()
    }
}

/// A race with a subrace, a fighter-like class with a subclass at level 1, a priest class
/// with a deity, one background.
#[allow(clippy::field_reassign_with_default)]
fn data() -> CcData {
    let mut d = CcData::default();
    d.races = vec![
        CcRace {
            id: g(RACE),
            name: "Folk".into(),
            progression_table: Some(g(T_RACE)),
            excluded_gods: vec![g(BANNED_GOD)],
            equipment: Some("EQ_Underwear".into()),
            ..Default::default()
        },
        CcRace {
            id: g(SUBRACE),
            name: "Folk_Hill".into(),
            parent: Some(g(RACE)),
            progression_table: Some(g(T_SUB)),
            ..Default::default()
        },
        CcRace {
            id: g(OTHER_SUB),
            name: "Folk_Unplayable".into(),
            parent: Some(g(RACE)),
            ..Default::default()
        },
    ];
    d.presets = vec![CcPreset {
        id: g(0x600),
        race: g(RACE),
        subrace: Some(g(SUBRACE)),
        body_type: 0,
        body_shape: 0,
        root_template: g(TEMPLATE),
    }];
    d.classes = vec![
        CcClass {
            id: g(CLASS),
            name: "Warrior".into(),
            base_hp: 10,
            hp_per_level: 6,
            progression_table: Some(g(T_CLASS)),
            tags: vec![g(TAG_CLASS)],
            equipment: Some("EQP_Warrior".into()),
            ..Default::default()
        },
        CcClass {
            id: g(SUBCLASS),
            name: "Champion".into(),
            parent: Some(g(CLASS)),
            progression_table: Some(g(T_SUBCLASS)),
            ..Default::default()
        },
        CcClass {
            id: g(PRIEST),
            name: "Priest".into(),
            base_hp: 8,
            progression_table: Some(g(T_PRIEST)),
            spellcasting_ability: Some(Ability::Wisdom),
            has_god: true,
            ..Default::default()
        },
    ];
    d.backgrounds = vec![CcBackground {
        id: g(BACKGROUND),
        passives: strings(&["Background_Test"]),
        tags: vec![g(TAG_BG)],
        ..Default::default()
    }];
    d.gods = vec![
        CcGod {
            id: g(GOD),
            name: "Good".into(),
            tags: vec![g(TAG_GOD)],
            ..Default::default()
        },
        CcGod {
            id: g(BANNED_GOD),
            name: "Banned".into(),
            ..Default::default()
        },
    ];
    d.origins = vec![CcOrigin {
        id: g(0x700),
        name: "Generic".into(),
        passives: strings(&["DeathSavingThrows"]),
        appearance_tags: vec![g(TAG_GENERIC)],
        ..Default::default()
    }];
    let mut class_row = row(
        P_CLASS,
        T_CLASS,
        "ProficiencyBonus(SavingThrow,Strength);ProficiencyBonus(SavingThrow,Constitution);\
         Proficiency(MediumArmor);Proficiency(Shields);Proficiency(MartialWeapons)",
        &["SecondWind"],
        &format!(
            "SelectSkills({},2);SelectPassives({},1,FightingStyle);AddSpells({});\
             SelectAbilityBonus({},AbilityBonus,2,1)",
            g(L_SKILLS),
            g(L_STYLES),
            g(L_GRANTED),
            g(L_ABILITIES)
        ),
    );
    class_row.subclasses = vec![g(SUBCLASS)];
    let priest_row = row(
        0x203,
        T_PRIEST,
        "ProficiencyBonus(SavingThrow,Wisdom)",
        &[],
        &format!(
            "SelectSpells({},2,0,,,,AlwaysPrepared);SelectAbilityBonus({},AbilityBonus,2,1)",
            g(L_CANTRIPS),
            g(L_ABILITIES)
        ),
    );
    for r in [
        row(
            0x204,
            T_RACE,
            "ActionResource(Movement,9,0)",
            &["Darkvision"],
            "",
        ),
        row(P_SUB, T_SUB, "Ability(Constitution,1)", &[], ""),
        class_row,
        row(P_SUBCLASS, T_SUBCLASS, "AC(1)", &[], ""),
        priest_row,
    ] {
        d.progressions.entry(r.table).or_default().push(r);
    }
    d.skill_lists.insert(
        g(L_SKILLS),
        strings(&["Athletics", "Insight", "Perception"]),
    );
    d.spell_lists
        .insert(g(L_CANTRIPS), strings(&["Spark", "Light", "Ward"]));
    d.spell_lists
        .insert(g(L_GRANTED), strings(&["Shout_SecondWind"]));
    d.passive_lists
        .insert(g(L_STYLES), strings(&["Style_Defence", "Style_Archery"]));
    d.ability_lists.insert(
        g(L_ABILITIES),
        Ability::ALL.iter().map(|a| a.name().to_owned()).collect(),
    );
    d.ability_presets.insert(g(CLASS), [15, 13, 14, 8, 12, 10]);
    d.defaults = vec![
        DefaultValue {
            kind: DefaultKind::Abilities,
            table: g(T_CLASS),
            level: 1,
            selector_id: Some("AbilityBonus".into()),
            race: None,
            origin: None,
            add: strings(&["Strength", "Constitution"]),
        },
        // Insight comes from the background: the default skips it.
        DefaultValue {
            kind: DefaultKind::Skills,
            table: g(T_CLASS),
            level: 1,
            selector_id: None,
            race: None,
            origin: None,
            add: strings(&["Insight", "Perception", "Athletics"]),
        },
        DefaultValue {
            kind: DefaultKind::Passives,
            table: g(T_CLASS),
            level: 1,
            selector_id: None,
            race: None,
            origin: None,
            add: strings(&["Style_Defence"]),
        },
    ];
    d.passive_boosts.insert(
        "Background_Test".into(),
        "ProficiencyBonus(Skill,Insight);ProficiencyBonus(Skill,Religion)".into(),
    );
    d.passive_boosts
        .insert("Style_Defence".into(), "AC(1)".into());
    d.passive_conditions.insert(
        "Style_Defence".into(),
        "WearingArmor(context.Source)".into(),
    );
    d.equipment_sets
        .insert("EQ_Underwear".into(), strings(&["ARM_Underwear"]));
    d.equipment_sets.insert(
        "EQP_Warrior".into(),
        strings(&["WPN_Sword", "ARM_Chain", "ARM_Shield"]),
    );
    let mut sword = item("WPN_Sword", "Weapon", "Melee Main Weapon");
    sword.proficiency = strings(&["MartialWeapons"]);
    sword.unlocks = strings(&["Target_Cleave"]);
    let mut chain = item("ARM_Chain", "Armor", "Breast");
    chain.armor_class = 13;
    chain.ability = Some(Ability::Dexterity);
    chain.ability_cap = 2;
    chain.armor_type = "ChainShirt".into();
    let mut shield = item("ARM_Shield", "Armor", "Melee Offhand Weapon");
    shield.armor_class = 2;
    shield.shield = true;
    for i in [
        sword,
        chain,
        shield,
        item("ARM_Underwear", "Armor", "Underwear"),
    ] {
        d.items.insert(i.name.clone(), i);
    }
    d
}

fn warrior() -> CharacterChoice {
    CharacterChoice {
        race: g(RACE),
        subrace: Some(g(SUBRACE)),
        class: g(CLASS),
        background: g(BACKGROUND),
        ..Default::default()
    }
}

#[test]
fn point_buy_table() {
    let costs: Vec<Option<i32>> = (7..=16).map(point_buy_cost).collect();
    assert_eq!(
        costs,
        [
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
            Some(7),
            Some(9),
            None
        ]
    );
    // A standard array costs exactly the budget.
    assert_eq!(point_buy_spent(&[15, 14, 13, 12, 10, 8]), Some(27));
    assert_eq!(point_buy_spent(&[8; 6]), Some(0));
    assert_eq!(point_buy_spent(&[16, 8, 8, 8, 8, 8]), None);
}

#[test]
fn point_buy_validation() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    assert!(d.validate(&c).is_empty(), "{:?}", d.validate(&c));
    c.abilities = [15, 15, 15, 9, 8, 8];
    assert_eq!(d.validate(&c), [CcError::TooManyPoints(28)]);
    c.abilities = [16, 8, 8, 8, 8, 8];
    assert_eq!(
        d.validate(&c),
        [CcError::AbilityOutOfRange(Ability::Strength, 16)]
    );
    c.abilities = [8; 6];
    assert!(d.validate(&c).is_empty(), "unspent points are allowed");
    assert_eq!(CcData::unspent_points(&c), 27);
}

#[test]
fn selectors_parse() {
    let s = rules::parse_selectors(&format!(
        "SelectSkills({0},2);SelectSpells({0},3,0,,,,AlwaysPrepared);AddSpells({0},,Charisma);\
         SelectAbilityBonus({0},AbilityBonus,2,1);SelectAbilities({0},2,1,HalfElf);\
         SelectPassives({0},1,FightingStyle)",
        g(L_SKILLS)
    ));
    let kinds: Vec<(SelectorKind, usize)> = s.iter().map(|x| (x.kind.clone(), x.count)).collect();
    assert_eq!(
        kinds,
        [
            (SelectorKind::SelectSkills, 2),
            (SelectorKind::SelectSpells, 3),
            (SelectorKind::AddSpells, 0),
            (SelectorKind::SelectAbilityBonus, 2),
            (SelectorKind::SelectAbilities, 2),
            (SelectorKind::SelectPassives, 1),
        ]
    );
    assert_eq!(s[2].ability, Some(Ability::Charisma));
    assert_eq!(s[3].amounts, [2, 1]);
    assert_eq!(s[4].amounts, [1, 1]);
    assert_eq!(s[5].selector_id.as_deref(), Some("FightingStyle"));
    assert!(s.iter().all(|x| x.list == g(L_SKILLS)));
}

#[test]
fn defaults_fill_the_open_choices() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    assert_eq!(c.subclass, Some(g(SUBCLASS)), "the first level-1 subclass");
    assert_eq!(c.deity, None, "no deity for a class without one");
    assert_eq!(c.abilities, [15, 13, 14, 8, 12, 10], "the class preset");
    let picks = |p: u128, i: usize| {
        c.picks
            .iter()
            .find(|x| x.progression == g(p) && x.index == i)
            .map(|x| x.items.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        picks(P_CLASS, 0),
        ["Perception", "Athletics"],
        "Insight skipped"
    );
    assert_eq!(picks(P_CLASS, 1), ["Style_Defence"]);
    assert_eq!(picks(P_CLASS, 3), ["Strength", "Constitution"]);
}

#[test]
fn level_one_progressions_make_the_sheet() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    let ch = d.build(&c).expect("valid");
    assert_eq!(ch.template, g(TEMPLATE));
    // 15 + 2 Str; 14 + 1 (bonus) + 1 (subrace) Con.
    assert_eq!(ch.abilities, [17, 13, 16, 8, 12, 10]);
    assert_eq!(ch.max_hp, 10 + 3, "BaseHp + Con modifier");
    // Chain 13 + Dex 1, shield 2, subclass AC(1), Defence (wearing armour) 1.
    assert_eq!(ch.armor_class, 13 + 1 + 2 + 1 + 1);
    assert_eq!(ch.saving_throws, [Ability::Strength, Ability::Constitution]);
    assert_eq!(
        ch.skills,
        [
            Skill::Insight,
            Skill::Religion,
            Skill::Perception,
            Skill::Athletics
        ]
    );
    assert_eq!(
        ch.proficiencies,
        ["MediumArmor", "Shields", "MartialWeapons"]
    );
    assert_eq!(
        ch.passives,
        [
            "Darkvision",
            "SecondWind",
            "Background_Test",
            "DeathSavingThrows",
            "Style_Defence"
        ]
    );
    assert_eq!(
        ch.spells,
        ["Shout_SecondWind", "Target_Cleave"],
        "AddSpells, then the proficient weapon's action"
    );
    assert_eq!(
        ch.equipment,
        ["ARM_Underwear", "WPN_Sword", "ARM_Chain", "ARM_Shield"]
    );
    assert_eq!(ch.tags, [g(TAG_BG), g(TAG_CLASS), g(TAG_GENERIC)]);
}

#[test]
fn invalid_picks_are_reported() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    let skills = c
        .picks
        .iter_mut()
        .find(|p| p.progression == g(P_CLASS) && p.index == 0)
        .expect("skills");
    skills.items = strings(&["Insight", "Insight", "Stealth"]);
    let errs = d.validate(&c);
    assert!(
        errs.contains(&CcError::AlreadyProficient("Insight".into())),
        "{errs:?}"
    );
    assert!(errs.iter().any(|e| matches!(e, CcError::Duplicate { .. })));
    assert!(
        errs.iter()
            .any(|e| matches!(e, CcError::NotAnOption { .. }))
    );
    assert!(errs.iter().any(|e| matches!(e, CcError::PickCount { .. })));
    assert!(d.build(&c).is_err());
}

#[test]
fn identity_rules() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    let mut x = c.clone();
    x.subrace = Some(g(OTHER_SUB));
    assert!(d.validate(&x).contains(&CcError::BadSubrace));
    let mut x = c.clone();
    x.subclass = None;
    assert!(d.validate(&x).contains(&CcError::BadSubclass));
    let mut x = c.clone();
    x.body_type = 1;
    assert!(d.validate(&x).contains(&CcError::NoBodyPreset));
    let mut x = c.clone();
    x.deity = Some(g(GOD));
    assert!(
        d.validate(&x).contains(&CcError::BadDeity(g(GOD))),
        "the class has no deity"
    );
    // The priest needs one, and not one the race excludes.
    let mut p = CharacterChoice {
        class: g(PRIEST),
        ..warrior()
    };
    d.complete(&mut p);
    assert_eq!(p.deity, Some(g(GOD)));
    assert!(d.validate(&p).is_empty(), "{:?}", d.validate(&p));
    p.deity = None;
    assert!(d.validate(&p).contains(&CcError::DeityRequired));
    p.deity = Some(g(BANNED_GOD));
    assert!(d.validate(&p).contains(&CcError::BadDeity(g(BANNED_GOD))));
}

#[test]
fn spells_are_picked_once() {
    let d = data();
    let mut p = CharacterChoice {
        class: g(PRIEST),
        ..warrior()
    };
    d.complete(&mut p);
    let ch = d.build(&p).expect("valid");
    assert_eq!(ch.spells, ["Spark", "Light"], "the list's first two");
    assert_eq!(ch.spellcasting_ability, Some(Ability::Wisdom));
    assert_eq!(
        ch.max_hp,
        8 + 2,
        "no class preset: 15,14,13,12,10,8 with +1 Con"
    );
    let spells = p
        .picks
        .iter_mut()
        .find(|x| x.items.contains(&"Spark".to_owned()))
        .expect("spell pick");
    spells.items = strings(&["Spark", "Spark"]);
    assert!(
        d.validate(&p)
            .iter()
            .any(|e| matches!(e, CcError::Duplicate { .. }))
    );
}

#[test]
fn spec_round_trip() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    let old_spec = c.to_spec();
    assert_eq!(CharacterChoice::from_spec(&old_spec), Ok(c.clone()));
    c.appearance.head = Some(g(801));
    c.appearance.hair = Some(g(802));
    c.appearance.beard = Some(g(803));
    c.appearance.horns = Some(g(804));
    c.appearance.tail = Some(g(805));
    c.appearance.piercing = Some(g(806));
    c.appearance.private_parts = Some(g(807));
    c.appearance.skin_color = Some(g(808));
    c.appearance.hair_color = Some(g(809));
    c.appearance.eye_color = Some(g(810));
    c.appearance.set_visual("SyntheticExtra", Some(g(811)));
    c.appearance.set_material("SyntheticMaterial", Some(g(812)));
    let spec = c.to_spec();
    assert_eq!(CharacterChoice::from_spec(&spec), Ok(c));
    assert!(CharacterChoice::from_spec("race=nope").is_err());
}

/// The created character on a sim entity: sheet, tags, combat set-up.
#[test]
fn apply_to_the_sim() {
    let d = data();
    let mut c = warrior();
    d.complete(&mut c);
    let ch = d.build(&c).expect("valid");
    let mut sim = crate::Sim::new(crate::GameData::default());
    let who = format!("Hero_{}", g(0xabc));
    let id = sim
        .observe_entity(&who, crate::EntityKind::Character)
        .expect("id");
    sim.apply_created_character(id, &ch);
    let e = sim.entity(id).expect("entity");
    let sheet = e.sheet.as_ref().expect("sheet");
    assert_eq!(sheet.abilities, ch.abilities);
    assert_eq!((sheet.hp, sheet.max_hp, sheet.level), (13, 13, 1));
    assert_eq!(sheet.proficiency_bonus, 2);
    let events: Vec<String> = sim
        .take_events()
        .iter()
        .filter(|e| e.name == "TagSet")
        .map(|e| e.to_string())
        .collect();
    // Engine tags are visible from the merge on (the story asks after it).
    assert!(sim.is_tagged(id, g(TAG_CLASS)));
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(
        sim.origin_def(id).and_then(|o| o.subclass),
        Some(g(SUBCLASS))
    );
}
