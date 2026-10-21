//! Tests against the real game install (`cargo test -p bg3-rules -- --ignored`).

use bg3_pak::Package;
use bg3_rules::condition::{Condition, ConditionContext, ConditionEngine, condition_to_lua};
use bg3_rules::context::{Ability, DamageType, Weapon};
use bg3_rules::dice::{RollDefinition, RollStream};
use bg3_rules::exec::{Executor, Spell};
use bg3_rules::survey::{Language, survey};
use bg3_rules::testworld::{Combatant, TestWorld};
use bg3_rules::thoth::{self, Machine, NoHost, Value};
use bg3_stats::install::{BASE_LOAD_ORDER, HONOUR_MODULES, load};

fn paks() -> Option<Vec<Package>> {
    let Ok(install) = bg3_install::Install::locate() else {
        eprintln!("BG3 install not found; skipping");
        return None;
    };
    Some(
        install
            .paks()
            .unwrap()
            .iter()
            .map(|p| Package::open(p).unwrap())
            .collect(),
    )
}

/// Every `.khn` script in every pak: (pak file name, path, source).
fn khn_files(paks: &[Package]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for pak in paks {
        for e in pak.entries() {
            if e.name.to_ascii_lowercase().ends_with(".khn") {
                let bytes = pak.read(e).unwrap();
                let pak_name = pak
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((
                    pak_name,
                    e.name.clone(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
            }
        }
    }
    out
}

#[test]
#[ignore = "needs BG3 install"]
fn parse_all_khn_scripts() {
    let Some(paks) = paks() else { return };
    let files = khn_files(&paks);
    let mut failures = Vec::new();
    let mut lines = 0;
    for (pak, path, src) in &files {
        lines += src.lines().count();
        if let Err(e) = thoth::parse_chunk(src) {
            failures.push(format!("{pak}:{path}: {e}"));
        }
    }
    eprintln!(
        "khn: {} files, {lines} lines, {} failures",
        files.len(),
        failures.len()
    );
    for f in &failures {
        eprintln!("  {f}");
    }
    assert!(files.len() > 50);
    assert!(failures.is_empty());
}

#[test]
#[ignore = "needs BG3 install"]
fn parse_all_stats_expressions() {
    let Some(paks) = paks() else { return };
    let mut order = BASE_LOAD_ORDER.to_vec();
    order.extend_from_slice(HONOUR_MODULES);
    let stats = load(&paks, &order).unwrap();
    let s = survey(&stats);
    eprintln!(
        "{:<18} {:<34} {:>7} {:>7} {:>8}",
        "language", "field", "values", "ok", "dropped"
    );
    for ((lang, field), t) in &s.by_field {
        eprintln!(
            "{:<18} {:<34} {:>7} {:>7} {:>8}",
            format!("{lang:?}"),
            field,
            t.values,
            t.ok,
            t.dropped_items
        );
        for (entry, value, err) in t.failures.iter().take(5) {
            eprintln!("    FAIL {entry}: {value:?}: {err}");
        }
    }
    for lang in [
        Language::Functors,
        Language::Condition,
        Language::RollConditions,
        Language::Boosts,
        Language::Requirements,
        Language::FunctorContexts,
        Language::Costs,
        Language::DescriptionParams,
        Language::Expression,
    ] {
        let (v, ok) = s.total(lang);
        eprintln!("TOTAL {lang:?}: {ok}/{v}");
    }
    eprintln!("functor items the engine drops: {}", s.dropped.len());
    for (entry, text, reason) in &s.dropped {
        eprintln!("    dropped in {entry}: {text:?}: {reason}");
    }
    eprintln!("functors: {:?}", s.functor_names);
    eprintln!("condition calls: {} distinct", s.condition_calls.len());
    eprintln!("raw expression variables: {:?}", s.raw_variables);
    eprintln!("literal description params: {:?}", s.literal_params);
    assert_eq!(s.failures(), 0);
}

/// Our reimplementation of `generateConditionString.khn` gives the same text as running the
/// shipped script in our interpreter, for every condition in the install.
#[test]
#[ignore = "needs BG3 install"]
fn condition_rewrite_matches_shipped_script() {
    let Some(paks) = paks() else { return };
    let files = khn_files(&paks);
    let (_, _, script) = files
        .iter()
        .find(|(_, p, _)| p.ends_with("thoth/generateConditionString.khn"))
        .expect("generateConditionString.khn");
    let chunk = thoth::parse_chunk(script).unwrap();
    let stats = load(&paks, BASE_LOAD_ORDER).unwrap();
    let mut conditions = std::collections::BTreeSet::new();
    for obj in stats.entries() {
        for (k, v) in &obj.own {
            if k.contains("Condition") || k == "SpellRoll" {
                conditions.insert(v.clone());
            }
        }
    }
    let mut m = Machine::new();
    let mut checked = 0;
    for c in &conditions {
        let out = m
            .exec(&mut NoHost, chunk.clone(), vec![Value::str(c)])
            .unwrap();
        let Some(Value::Str(lua)) = out.first() else {
            panic!("script returned {out:?}");
        };
        assert_eq!(&**lua, condition_to_lua(c), "for {c:?}");
        checked += 1;
    }
    eprintln!("checked {checked} distinct condition strings");
    assert!(checked > 1000);
}

/// A melee attack using `Target_MainHandAttack`'s real fields, between two synthetic
/// combatants, with the shipped condition helpers and a fixed seed.
#[test]
#[ignore = "needs BG3 install"]
fn mini_encounter_main_hand_attack() {
    let Some(paks) = paks() else { return };
    let stats = load(&paks, BASE_LOAD_ORDER).unwrap();
    let obj = stats
        .get("Target_MainHandAttack")
        .expect("Target_MainHandAttack");
    let spell = Spell::from_fields(&obj.name, |f| obj.get(f)).unwrap();

    let mut world = TestWorld::new();
    let mut fighter = Combatant::new("Fighter");
    fighter.player = true;
    let fighter = world.spawn(
        fighter
            .ability(Ability::Strength, 16)
            .hp(12)
            .ac(16)
            .team(1)
            .weapon(Weapon {
                name: "Longsword".into(),
                damage: RollDefinition::parse("1d8").unwrap(),
                damage_type: DamageType::Slashing,
                ranged: false,
                finesse: false,
                proficient: true,
            }),
    );
    let goblin = world.spawn(
        Combatant::new("Goblin")
            .ability(Ability::Dexterity, 14)
            .hp(15)
            .ac(15)
            .team(2)
            .weapon(Weapon {
                name: "Scimitar".into(),
                damage: RollDefinition::parse("1d6").unwrap(),
                damage_type: DamageType::Slashing,
                ranged: false,
                finesse: true,
                proficient: true,
            }),
    );
    let mut engine = ConditionEngine::new();
    let mut rng = RollStream::new(1234);
    for (_, path, src) in khn_files(&paks) {
        if path.starts_with("Mods/Shared/Scripts/thoth/helpers/") {
            engine
                .load_helpers(&path, &src, &mut world, &mut rng)
                .unwrap();
        }
    }
    // The shipped helpers define Self() in Thoth; the natives cover Dead() and Player().
    let tc = Condition::parse(obj.get("TargetConditions").unwrap()).unwrap();
    let data = ConditionContext {
        source: Some(fighter),
        target: Some(goblin),
        ..Default::default()
    };
    assert!(
        engine
            .evaluate(&tc, &mut world, &mut rng, &data)
            .unwrap()
            .result
            .result
    );

    let mut ex = Executor::new(&mut engine, &mut world, &mut rng);
    let mut round = 0;
    while ex.ctx.hp(goblin) > 0 && ex.ctx.hp(fighter) > 0 && round < 10 {
        round += 1;
        ex.log.push(format!("-- round {round}"));
        ex.cast(&spell, fighter, goblin);
        if ex.ctx.hp(goblin) > 0 {
            ex.cast(&spell, goblin, fighter);
        }
    }
    let log = ex.log.join("\n");
    eprintln!("{log}");
    assert!(log.contains("casts Target_MainHandAttack"));
    assert!(log.contains("MeleeWeaponAttack roll"));
}

/// How far condition evaluation gets on the install's `TargetConditions`, with the shipped
/// helpers and two synthetic combatants: counts evaluations that complete, and the missing
/// natives/properties that stop the others. Informational; asserts only that some complete.
#[test]
#[ignore = "needs BG3 install"]
fn evaluate_install_target_conditions() {
    let Some(paks) = paks() else { return };
    let stats = load(&paks, BASE_LOAD_ORDER).unwrap();
    let mut world = TestWorld::new();
    let a = world.spawn(Combatant::new("A").team(1));
    let b = world.spawn(Combatant::new("B").team(2));
    let mut engine = ConditionEngine::new();
    let mut rng = RollStream::new(1);
    for (_, path, src) in khn_files(&paks) {
        if path.starts_with("Mods/Shared/Scripts/thoth/helpers/") {
            engine
                .load_helpers(&path, &src, &mut world, &mut rng)
                .unwrap();
        }
    }
    let data = ConditionContext {
        source: Some(a),
        target: Some(b),
        ..Default::default()
    };
    let mut distinct = std::collections::BTreeSet::new();
    for obj in stats.entries() {
        if let Some(v) = obj.get("TargetConditions") {
            distinct.insert(v.to_owned());
        }
    }
    let (mut ok, mut failed) = (0, 0);
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    for c in &distinct {
        let Ok(cond) = Condition::parse(c) else {
            continue;
        };
        match engine.evaluate(&cond, &mut world, &mut rng, &data) {
            Ok(_) => ok += 1,
            Err(e) => {
                failed += 1;
                let msg = e.to_string();
                let key = msg
                    .split('\'')
                    .nth(1)
                    .map_or(msg.clone(), |n| format!("'{n}'"));
                *reasons.entry(key).or_default() += 1;
            }
        }
    }
    let mut top: Vec<_> = reasons.into_iter().collect();
    top.sort_by_key(|x| std::cmp::Reverse(x.1));
    eprintln!(
        "distinct TargetConditions: {} evaluated, {} stopped",
        ok, failed
    );
    for (r, n) in top.iter().take(25) {
        eprintln!("  {n:>4}  {r}");
    }
    assert!(ok > 0);
}
