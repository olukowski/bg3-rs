//! Character creation on the install's data: the point-buy rules against the class
//! presets, a level-1 human fighter, every race × class with the defaults, and the trace's
//! character in a live game. Needs the game install; skips cleanly without it.

use std::sync::Arc;

use bg3_rules::context::Ability;
use bg3_sim::Guid;
use bg3_sim::cc::{CcData, CharacterChoice, POINT_BUY_POINTS, point_buy_spent};
use bg3_sim::start::{load_game, new_game, trace_choice};

fn vfs() -> Option<Arc<bg3_vfs::Vfs>> {
    match bg3_install::Install::locate() {
        Ok(i) => Some(Arc::new(bg3_vfs::Vfs::from_install(&i).expect("vfs"))),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn data(vfs: &bg3_vfs::Vfs) -> (CcData, bg3_sim::TagDefs) {
    let stats =
        bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER).expect("stats");
    (
        CcData::load(vfs, &stats).expect("cc data"),
        bg3_sim::load::load_tags(vfs).expect("tags"),
    )
}

fn named<'a>(cc: &'a CcData, name: &str) -> &'a bg3_sim::cc::CcClass {
    cc.playable_classes()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("class {name}"))
}

/// Every `AbilityDistributionPresets` entry is a legal point buy that spends the whole
/// budget: the evidence for the point-buy table (`bg3_sim::cc::point_buy_cost`).
#[test]
#[ignore = "needs BG3 install"]
fn presets_spend_the_point_buy_budget() {
    let Some(vfs) = vfs() else {
        return;
    };
    let (cc, _) = data(&vfs);
    assert!(
        cc.ability_presets.len() >= 12,
        "{}",
        cc.ability_presets.len()
    );
    for (class, scores) in &cc.ability_presets {
        assert_eq!(
            point_buy_spent(scores),
            Some(POINT_BUY_POINTS),
            "{class}: {scores:?}"
        );
    }
}

#[test]
#[ignore = "needs BG3 install"]
fn human_fighter_level_one() {
    let Some(vfs) = vfs() else {
        return;
    };
    let (cc, _) = data(&vfs);
    let human = cc
        .playable_races()
        .into_iter()
        .find(|r| r.name == "Human")
        .expect("human")
        .id;
    let fighter = named(&cc, "Fighter");
    let mut choice = CharacterChoice {
        race: human,
        class: fighter.id,
        abilities: [15, 14, 13, 12, 10, 8],
        ..trace_choice()
    };
    choice.subrace = None;
    choice.deity = None;
    cc.complete(&mut choice);
    let c = cc.build(&choice).unwrap_or_else(|e| panic!("{e:?}"));
    // The fighter's default bonus is Strength +2, Constitution +1 (`DefaultValues`).
    assert_eq!(c.abilities, [17, 14, 14, 12, 10, 8]);
    assert_eq!(fighter.base_hp, 10);
    assert_eq!(c.max_hp, 10 + 2);
    assert_eq!(c.saving_throws, [Ability::Strength, Ability::Constitution]);
    for p in [
        "LightArmor",
        "MediumArmor",
        "HeavyArmor",
        "Shields",
        "SimpleWeapons",
        "MartialWeapons",
    ] {
        assert!(c.proficiencies.iter().any(|x| x == p), "{p}");
    }
    // Starting equipment `EQP_CC_Fighter`: scale mail (14, Dex cap 2) and the default
    // fighting style, Defence (+1 AC in armour).
    assert!(c.equipment.iter().any(|e| e == "ARM_ScaleMail_Body"));
    assert!(c.passives.iter().any(|p| p == "FightingStyle_Defense"));
    assert_eq!(c.armor_class, 14 + 2 + 1);
    assert_eq!(
        c.skills.len(),
        2 + 2 + 1,
        "background, class, Human Versatility"
    );
}

/// Every playable race and class makes a valid character with the defaults.
#[test]
#[ignore = "needs BG3 install"]
fn every_race_and_class_completes() {
    let Some(vfs) = vfs() else {
        return;
    };
    let (cc, _) = data(&vfs);
    let mut n = 0;
    for race in cc.playable_races() {
        for class in cc.playable_classes() {
            let mut choice = CharacterChoice {
                race: race.id,
                class: class.id,
                background: cc.playable_backgrounds()[0].id,
                ..CharacterChoice::default()
            };
            cc.complete(&mut choice);
            let errs = cc.validate(&choice);
            assert!(errs.is_empty(), "{} {}: {errs:?}", race.name, class.name);
            let c = cc.build(&choice).expect("valid");
            assert!(
                c.max_hp >= 1 && c.armor_class >= 10,
                "{} {}",
                race.name,
                class.name
            );
            n += 1;
        }
    }
    assert!(n >= 11 * 12, "{n}");
}

/// The tags the engine gave the trace's avatar after `CharacterCreationFinished`
/// (`full-nautiloid.log`, `TagSet` without a story `SetTag`), in its order.
const TRACE_CC_TAGS: &[&str] = &[
    "GUILD_ARTISAN",
    "CLERIC",
    "MYSTRA",
    "ALIGN_GOOD",
    "GENERIC",
    "REALLY_GENERIC",
];
/// The tags the trace's avatar had before `CharacterCreationFinished` (race and template).
const TRACE_SPAWN_TAGS: &[&str] = &[
    "DEEPGNOME",
    "UNDERDARK",
    "GNOME",
    "SHORT",
    "HUMANOID",
    "CIVIL_RACE",
    "VFX_ASHPILE_MEDIUM",
    "MALE",
];

#[test]
#[ignore = "needs BG3 install"]
fn trace_choice_reproduces_the_trace_avatar() {
    let Some(vfs) = vfs() else {
        return;
    };
    let (cc, tags) = data(&vfs);
    let mut choice = trace_choice();
    cc.complete(&mut choice);
    let c = cc.build(&choice).unwrap_or_else(|e| panic!("{e:?}"));
    let names: Vec<&str> = c
        .tags
        .iter()
        .map(|t| tags.get(*t).map_or("?", |d| d.name.as_str()))
        .collect();
    assert_eq!(names, TRACE_CC_TAGS);
    assert_eq!(
        c.template,
        Guid::parse(bg3_sim::start::TRACE_AVATAR_TEMPLATE).unwrap()
    );
    // The cleric starting equipment the trace's avatar received (`AddedTo` events).
    for item in [
        "WPN_Mace",
        "ARM_Shield",
        "ARM_ChainShirt_Body",
        "ARM_Boots_Leather",
        "ARM_Camp_Body",
        "ARM_Camp_Shoes",
        "OBJ_Scroll_Revivify",
        "OBJ_Potion_Healing",
        "OBJ_Keychain",
        "OBJ_Bag_AlchemyPouch",
        "OBJ_Backpack_CampSupplies",
        "ARM_Underwear_Gnomes",
    ] {
        assert!(c.equipment.iter().any(|e| e == item), "{item}");
    }
    // A cleric's level-1 hit points: BaseHp 8 + Constitution modifier.
    assert_eq!(c.max_hp, 8 + (c.abilities[2] - 10).div_euclid(2));

    // In a live game the avatar carries all of them.
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let (mut game, opts) = load_game(vfs).expect("load");
            let started = new_game(&mut game, &opts);
            for name in TRACE_SPAWN_TAGS.iter().chain(TRACE_CC_TAGS) {
                let t = game.sim.data.tags.by_name(name).expect("tag");
                assert!(game.sim.is_tagged(started.avatar, t), "{name}");
            }
            let sheet = game
                .sim
                .entity(started.avatar)
                .and_then(|e| e.sheet.clone())
                .expect("sheet");
            assert_eq!(sheet.max_hp, c.max_hp);
            assert_eq!(sheet.abilities, c.abilities);
            assert!(game.sim.ensure_combatant(started.avatar));
            let fighter = game.sim.combatant(started.avatar).expect("combatant");
            assert_eq!(fighter.equipment.melee_main.as_deref(), Some("WPN_Mace"));
            assert_eq!(fighter.equipment.shield.as_deref(), Some("ARM_Shield"));
            assert!(fighter.spells.iter().any(|s| s == "Target_SacredFlame"));
            let sheet = game
                .sim
                .entity(started.avatar)
                .and_then(|e| e.sheet.clone())
                .expect("sheet");
            assert_eq!(sheet.armor_class, c.armor_class, "chain shirt and shield");
        })
        .expect("spawn")
        .join()
        .expect("game thread");
}

#[test]
#[ignore = "needs BG3 install"]
fn default_deep_gnome_appearance_and_hair_spec() {
    let Some(vfs) = vfs() else {
        return;
    };
    let (cc, _) = data(&vfs);
    let mut choice = trace_choice();
    cc.complete(&mut choice);
    let options = cc.appearance_options(&choice, "Hair");
    let hair = options
        .iter()
        .find(|v| !v.visual.is_null())
        .expect("hair visual");
    choice.appearance.hair = Some(hair.id);
    let skins = cc.material_options(&choice, "Skin");
    assert!(!skins.is_empty());
    assert!(skins.len() < cc.appearance.material_options("Skin").len());
    assert!(
        skins
            .iter()
            .any(|m| Some(m.id) == choice.appearance.skin_color)
    );
    let neutral = cc
        .appearance
        .material_options("Skin")
        .into_iter()
        .find(|m| m.name == "HUM_Neutral_-2")
        .expect("human Neutral Tone 1");
    assert!(!skins.iter().any(|m| m.id == neutral.id));
    let mut invalid = choice.clone();
    invalid.appearance.skin_color = Some(neutral.id);
    assert!(cc.validate(&invalid).iter().any(|e| matches!(e,
        bg3_sim::cc::CcError::NotAnOption { slot, .. } if slot == "appearance Skin material")));
    for race in &cc.races {
        for (kind, ids) in &race.palettes {
            if !matches!(kind.as_str(), "Skin" | "Hair" | "Eye") {
                continue;
            }
            for id in ids {
                assert!(
                    cc.appearance
                        .materials
                        .iter()
                        .any(|m| m.kind == *kind && m.id == *id),
                    "{} {kind}: missing CC colour entry {id}",
                    race.name
                );
            }
        }
    }
    eprintln!(
        "deep gnome palettes: {:?}",
        cc.appearance
            .material_kinds()
            .iter()
            .map(|kind| (kind, cc.material_options(&choice, kind).len()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        CharacterChoice::from_spec(&choice.to_spec()),
        Ok(choice.clone())
    );
    assert!(
        cc.validate(&choice).is_empty(),
        "{:?}",
        cc.validate(&choice)
    );
}
