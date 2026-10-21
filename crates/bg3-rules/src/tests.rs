//! Unit tests with self-authored fixtures (no game data).

use crate::condition::{
    Condition, ConditionContext, ConditionEngine, RollConditions, condition_to_lua,
};
use crate::context::{
    Ability, Advantage, AttackType, DamageType, Resistance, RulesContext, Weapon,
};
use crate::dice::{DiceRng, DiceType, Mt64, RollDefinition, RollStream, ScriptedDice, roll_d20};
use crate::exec::{CastOutcome, Executor, Spell};
use crate::expr::{self, BuiltinVar, Evaluator, Expr, Mode, NoVariables, VarContext, VarKind};
use crate::fields::{self, BoostEntry, DescriptionParam};
use crate::functor::{self, Entry, FunctorList, Params, TargetSelector};
use crate::rules::{self, Critical};
use crate::testworld::{Combatant, TestWorld};
use crate::textkey;
use crate::thoth::ConditionResult;

// ---- dice ---------------------------------------------------------------------------------

#[test]
fn mt19937_64_reference_value() {
    // The C++ standard's check value: the 10000th output of a default-seeded mt19937_64.
    let mut mt = Mt64::new(5489);
    let mut v = 0;
    for _ in 0..10000 {
        v = mt.next_u64();
    }
    assert_eq!(v, 9981545732273789042);
}

#[test]
fn roll_stream_is_deterministic_and_in_range() {
    let mut a = RollStream::new(42);
    let mut b = RollStream::new(42);
    for _ in 0..1000 {
        let x = a.roll_die(20);
        assert_eq!(x, b.roll_die(20));
        assert!((1..=20).contains(&x));
    }
    assert_eq!(a.counter(), 1000);
}

#[test]
fn roll_definition_parse_and_average() {
    let r = RollDefinition::parse("2d6+3").unwrap();
    assert_eq!(r, RollDefinition::dice(2, DiceType::D6, 3));
    assert_eq!(r.average(), 10);
    assert_eq!(RollDefinition::parse("d8").unwrap().count, 1);
    assert_eq!(RollDefinition::parse("1d4-1").unwrap().bonus, -1);
    assert_eq!(RollDefinition::parse("7").unwrap(), RollDefinition::flat(7));
    assert!(RollDefinition::parse("1d7").is_none());
    assert!(RollDefinition::parse("-1d4").is_none());
    assert!(RollDefinition::parse("1d8+X").is_none());
    assert_eq!(RollDefinition::parse("1d8").unwrap().average(), 4);
}

#[test]
fn d20_advantage() {
    let adv = Advantage {
        advantage: true,
        disadvantage: false,
    };
    let dis = Advantage {
        advantage: false,
        disadvantage: true,
    };
    let both = Advantage {
        advantage: true,
        disadvantage: true,
    };
    assert_eq!(roll_d20(&mut ScriptedDice::new(&[5, 17]), adv).natural, 17);
    assert_eq!(roll_d20(&mut ScriptedDice::new(&[5, 17]), dis).natural, 5);
    let d = roll_d20(&mut ScriptedDice::new(&[5, 17]), both);
    assert_eq!((d.natural, d.discarded), (5, None));
}

// ---- stats expressions --------------------------------------------------------------------

fn eval_avg(s: &str) -> i32 {
    let e = expr::parse(s).unwrap();
    let mut env = NoVariables;
    let mut rng = ScriptedDice::new(&[1]);
    Evaluator::new(&mut env, &mut rng)
        .eval(&e, Mode::Average)
        .unwrap()
}

#[test]
fn expression_operators_are_right_associative() {
    // Split at the first operator: a-b-c = a-(b-c), a/b*c = a/(b*c).
    assert_eq!(eval_avg("10-4-3"), 9);
    assert_eq!(eval_avg("12/2*3"), 2);
    assert_eq!(eval_avg("2*3+4"), 10);
    assert_eq!(eval_avg("1+2*3"), 7);
    assert_eq!(eval_avg("7/2"), 3);
    assert_eq!(eval_avg("-7/2"), -3);
    assert_eq!(eval_avg("5/0"), 0);
    assert_eq!(eval_avg("(2d6)/2"), 3);
    assert_eq!(eval_avg("max(1,1d4-3)"), 1);
    assert_eq!(eval_avg("foreach(3, 1d6)"), 9);
}

#[test]
fn expression_outer_parens_are_stripped_without_matching() {
    // The engine strips a leading `(` and trailing `)` even when they don't pair up, so
    // `(1)+(2)` becomes `1)+(2`, which fails.
    assert!(expr::parse("(1)+(2)").is_err());
    assert!(expr::parse("((3))").is_ok());
}

#[test]
fn expression_variables() {
    let e = expr::parse("1d8+StrengthModifier").unwrap();
    let Expr::Add(l, r) = e else { panic!("{e:?}") };
    assert_eq!(*l, Expr::Roll(RollDefinition::dice(1, DiceType::D8, 0)));
    let Expr::Var(v) = *r else { panic!() };
    assert_eq!(v.kind, VarKind::Ability(Ability::Strength));
    assert!(v.modifiers.modifier);

    let Expr::Var(v) = expr::parse("Target.ProficiencyBonus").unwrap() else {
        panic!()
    };
    assert_eq!(v.context, VarContext::Target);
    assert_eq!(v.kind, VarKind::Builtin(BuiltinVar::ProficiencyBonus));

    let Expr::Var(v) = expr::parse("LevelMapValue(SneakAttack)").unwrap() else {
        panic!()
    };
    assert_eq!(v.kind, VarKind::LevelMapValue("SneakAttack".into()));

    let Expr::Var(v) = expr::parse("SpellCastingAbilityModifier").unwrap() else {
        panic!()
    };
    assert_eq!(v.kind, VarKind::Builtin(BuiltinVar::SpellCastingAbility));

    assert!(matches!(expr::parse("-1d4").unwrap(), Expr::Neg(_)));
    assert!(matches!(
        expr::parse("0-ResourceRoll(Omen,1)").unwrap(),
        Expr::Sub(..)
    ));
    assert!(expr::parse("1d8+").is_err());
}

#[test]
fn max_rolls_only_the_winner() {
    // Averages: 1 vs 1d4-3 (=-1): the constant wins, so nothing is rolled.
    let e = expr::parse("max(1,1d4-3)").unwrap();
    let mut env = NoVariables;
    let mut rng = ScriptedDice::new(&[4]);
    let mut ev = Evaluator::new(&mut env, &mut rng);
    assert_eq!(ev.eval(&e, Mode::Roll { critical: false }).unwrap(), 1);
    assert!(ev.rolls.is_empty());
    // 0 vs 1d4-2 (average 0): tie goes to the non-dice argument.
    let e = expr::parse("max(0,1d4-2)").unwrap();
    let mut ev = Evaluator::new(&mut env, &mut rng);
    assert_eq!(ev.eval(&e, Mode::Roll { critical: false }).unwrap(), 0);
    // 1 vs 2d6 (average 7): the dice win and are rolled, so the result can be anything 2..12.
    let e = expr::parse("max(1,2d6)").unwrap();
    let mut rng = ScriptedDice::new(&[1, 1]);
    let mut ev = Evaluator::new(&mut env, &mut rng);
    assert_eq!(ev.eval(&e, Mode::Roll { critical: false }).unwrap(), 2);
}

#[test]
fn critical_doubles_dice_not_bonus() {
    let e = expr::parse("1d6+3").unwrap();
    let mut env = NoVariables;
    let mut rng = ScriptedDice::new(&[6, 6]);
    let v = Evaluator::new(&mut env, &mut rng)
        .eval(&e, Mode::Roll { critical: true })
        .unwrap();
    assert_eq!(v, 15);
}

// ---- text keys and functors ---------------------------------------------------------------

#[test]
fn text_key_groups() {
    let g = textkey::split("A(1);B(2);Cast2[C(3);D(4)];E(5);Cast2[F(6)]").unwrap();
    assert_eq!(
        g,
        vec![
            ("Default".to_string(), "A(1);B(2);E(5)".to_string()),
            ("Cast2".to_string(), "C(3);D(4);F(6)".to_string()),
        ]
    );
    assert!(textkey::split("A[B[C]]").is_err());
    assert!(textkey::split("A[B").is_err());
}

fn only_functor(s: &str) -> functor::Functor {
    let l = FunctorList::parse(s).unwrap();
    match l.default_group() {
        [Entry::Functor(f)] => (**f).clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn functor_contexts_conditions_and_targets() {
    let f = only_functor("GROUND:DealDamage(MainMeleeWeapon, MainMeleeWeaponDamageType)");
    assert!(f.contexts.has(functor::CTX_GROUND));
    assert!(!f.contexts.has(functor::CTX_TARGET));
    let Params::DealDamage(d) = &f.params else {
        panic!()
    };
    assert_eq!(d.weapon.as_deref(), Some("MainMeleeWeapon"));
    assert_eq!(d.amount, Expr::Placeholder(0));

    let f = only_functor("TARGET:IF(not Tagged('X')):ApplyStatus(SELF,TEST_BURN,100,2)");
    assert!(f.contexts.has(functor::CTX_TARGET));
    assert_eq!(f.condition.as_ref().unwrap().source, "not Tagged('X')");
    assert_eq!(f.target, TargetSelector::Source);
    let Params::ApplyStatus(a) = &f.params else {
        panic!()
    };
    assert_eq!(a.status, "TEST_BURN");
    assert_eq!(a.chance, Some(100.0));

    // No context prefix: TARGET|AOE. Missing colon after IF and a doubled colon are accepted.
    let f = only_functor("IF(Character())ApplyStatus(PRONE,100,1)");
    assert!(f.contexts.has(functor::CTX_TARGET) && f.contexts.has(functor::CTX_AOE));
    assert!(f.condition.is_some());
    assert!(matches!(
        only_functor("IF(A())::Kill()").params,
        Params::Raw
    ));
    // Context names are case-insensitive.
    assert!(
        only_functor("Ground:Kill()")
            .contexts
            .has(functor::CTX_GROUND)
    );
}

#[test]
fn functor_apply_status_condition_argument() {
    let f = only_functor("ApplyStatus(TEST_FEAR,100,2, , , ,not SavingThrow(Ability.Wisdom, 12))");
    let Params::ApplyStatus(a) = &f.params else {
        panic!()
    };
    assert_eq!(
        a.condition.as_ref().unwrap().source,
        "not SavingThrow(Ability.Wisdom, 12)"
    );
    assert_eq!(a.duration, Some(Expr::Roll(RollDefinition::flat(2))));
}

#[test]
fn functor_unknown_names_are_dropped() {
    let l = FunctorList::parse("Kill();Explode(3);DealDamage(1d4,Banana)").unwrap();
    let kinds: Vec<bool> = l
        .default_group()
        .iter()
        .map(|e| matches!(e, Entry::Functor(_)))
        .collect();
    assert_eq!(kinds, vec![true, false, false]);
}

#[test]
fn functor_call_splitting() {
    let c = functor::parse_call("IF(A(1, 2), B):X(3)").unwrap();
    assert_eq!(c.name, "IF");
    assert_eq!(c.args, vec!["A(1, 2)", "B"]);
    assert_eq!(c.remainder, "X(3)");
    let c = functor::parse_call("F(a, , b)").unwrap();
    assert_eq!(c.args, vec!["a", "", "b"]);
    let c = functor::parse_call("F(a, )").unwrap();
    assert_eq!(c.args, vec!["a"]);
    assert!(functor::parse_call("F(a").is_none());
}

// ---- other field languages ----------------------------------------------------------------

#[test]
fn boosts() {
    let b = fields::parse_boosts(
        "AC(1);IF(HasStatus('TEST_X')):Resistance(Fire, Resistant);BlockRegainHP(A;B)",
    )
    .unwrap();
    assert_eq!(b.len(), 3);
    let BoostEntry::Boost(r) = &b[1] else {
        panic!()
    };
    assert_eq!(r.name, "Resistance");
    assert!(r.condition.is_some());
    let BoostEntry::Boost(r) = &b[2] else {
        panic!()
    };
    assert_eq!(r.args, vec!["A;B"]);
    // Text after a call without `;` is read as another boost.
    assert_eq!(fields::parse_boosts("A(1):B(2)").unwrap().len(), 2);
}

#[test]
fn requirements_costs_and_params() {
    let r = fields::parse_requirements("!Combat;Immobile").unwrap();
    assert!(r[0].negate && r[0].name == "Combat" && !r[1].negate);
    let c = fields::parse_costs("ActionPoint:1;SpellSlotsGroup:1:1:3").unwrap();
    assert_eq!(c[1].extra, vec!["1", "3"]);
    assert!(fields::parse_costs("Movement:Distance*0.5").is_ok());
    let p = fields::parse_description_params("DealDamage(1d6,Fire);3;Distance(1.5);50%").unwrap();
    let params = &p[0].1;
    assert!(matches!(&params[0], DescriptionParam::Call { name, .. } if name == "DealDamage"));
    assert!(matches!(&params[1], DescriptionParam::Expr(_)));
    assert!(matches!(&params[3], DescriptionParam::Text(t) if t == "50%"));
}

// ---- conditions ---------------------------------------------------------------------------

#[test]
fn condition_rewrite() {
    assert_eq!(
        condition_to_lua("not Self() and (A() or B())and C()"),
        "context = ... \nreturn ~Self()&(A()|B())&C()"
    );
    // Comparisons bind looser than the rewritten `&`.
    let c = Condition::parse("X == 1 and Y()").unwrap();
    assert!(c.lua.ends_with("X == 1&Y()"));
}

#[test]
fn condition_result_operators() {
    let t = ConditionResult {
        chance: 0.5,
        ..ConditionResult::new(true)
    };
    let f = ConditionResult {
        chance: 0.25,
        ..ConditionResult::new(false)
    };
    let and = t.and(&f);
    assert!(!and.result);
    assert_eq!(and.chance, 0.125);
    let or = t.or(&f);
    assert!(or.result);
    assert_eq!(or.chance, 1.0 - 0.5 * 0.75);
    let not = f.not();
    assert!(not.result);
    assert_eq!(not.chance, 0.75);
}

/// A self-authored helper, standing in for the shipped `CommonConditions.khn`.
const HELPERS: &str = r#"
-- Our own test helpers (not the shipped ones).
function Self(a, b)
    a = a or context.Source
    b = b or context.Target
    local same = a.IsValid and a == b
    return ConditionResult(same, {ConditionError("NotSelf")})
end
function Strong(who)
    return ConditionResult((who or context.Target).Strength >= 15)
end
"#;

fn world() -> (TestWorld, crate::EntityId, crate::EntityId) {
    let mut w = TestWorld::new();
    let sword = Weapon {
        name: "Sword".into(),
        damage: RollDefinition::dice(1, DiceType::D8, 0),
        damage_type: DamageType::Slashing,
        ranged: false,
        finesse: false,
        proficient: true,
    };
    let a = w.spawn(
        Combatant::new("Ann")
            .ability(Ability::Strength, 16)
            .hp(20)
            .ac(14)
            .team(1)
            .weapon(sword),
    );
    let mut b = Combatant::new("Bob").hp(9).ac(12).team(2);
    b.tags.insert("TEST_UNDEAD".into());
    b.resistances
        .insert(DamageType::Slashing, Resistance::Resistant);
    let b = w.spawn(b);
    (w, a, b)
}

fn engine(w: &mut TestWorld) -> ConditionEngine {
    let mut e = ConditionEngine::new();
    e.load_helpers("helpers", HELPERS, w, &mut ScriptedDice::new(&[1]))
        .unwrap();
    e
}

#[test]
fn evaluates_conditions_against_context() {
    let (mut w, a, b) = world();
    let mut e = engine(&mut w);
    let mut rng = ScriptedDice::new(&[10]);
    let data = ConditionContext {
        source: Some(a),
        target: Some(b),
        ..Default::default()
    };
    let mut check = |s: &str| {
        e.evaluate_str(s, &mut w, &mut rng, &data)
            .unwrap()
            .result
            .result
    };
    assert!(check("not Dead() and not Self()"));
    assert!(check("Self(context.Source, context.Source)"));
    assert!(check("Enemy() and Tagged('TEST_UNDEAD')"));
    assert!(!check("Ally()"));
    assert!(check("Strong(context.Source) and not Strong()"));
    assert!(check("Character() or Item()"));
    assert!(check(""));
    // Errors carried by a false result.
    let r = e
        .evaluate_str("Self()", &mut w, &mut rng, &data)
        .unwrap()
        .result;
    assert_eq!(&*r.active_errors[0].name, "NotSelf");
    // A missing helper is a script error.
    assert!(
        e.evaluate_str("NoSuchHelper()", &mut w, &mut rng, &data)
            .is_err()
    );
}

#[test]
fn conditions_evaluate_both_sides() {
    // `and` becomes `&`: the saving throw is rolled even though the left side is false.
    let (mut w, a, b) = world();
    let mut e = engine(&mut w);
    let mut rng = ScriptedDice::new(&[3, 3]);
    let data = ConditionContext {
        source: Some(a),
        target: Some(b),
        ..Default::default()
    };
    let ev = e
        .evaluate_str(
            "Ally() and SavingThrow(Ability.Dexterity, 10)",
            &mut w,
            &mut rng,
            &data,
        )
        .unwrap();
    assert!(!ev.result.result);
    assert_eq!(ev.rolls.len(), 1);
}

#[test]
fn roll_conditions_text_keys() {
    let r = RollConditions::parse("Attack(AttackType.MeleeWeaponAttack);CastOffhand[Attack(AttackType.MeleeOffHandWeaponAttack)]").unwrap();
    assert_eq!(r.groups.len(), 2);
    assert!(r.get("CastOffhand").is_some());
}

// ---- rolls --------------------------------------------------------------------------------

#[test]
fn attack_roll_rules() {
    let (w, a, b) = world();
    // d20 = 9: 9 + 3 (Str) + 2 (prof) = 14 >= AC 12 → hit.
    let r = rules::roll_attack(
        &w,
        &mut ScriptedDice::new(&[9, 1]),
        a,
        b,
        AttackType::MeleeWeaponAttack,
    );
    assert_eq!((r.total, r.hit, r.critical), (14, true, Critical::None));
    // Natural 1 misses regardless; natural 20 hits regardless.
    let r = rules::roll_attack(
        &w,
        &mut ScriptedDice::new(&[1, 1]),
        a,
        b,
        AttackType::MeleeWeaponAttack,
    );
    assert_eq!((r.hit, r.critical), (false, Critical::Fail));
    let mut tough = w.clone();
    tough.get_mut(b).armor_class = 30;
    let r = rules::roll_attack(
        &tough,
        &mut ScriptedDice::new(&[20, 1]),
        a,
        b,
        AttackType::MeleeWeaponAttack,
    );
    assert_eq!((r.hit, r.critical), (true, Critical::Success));
}

#[test]
fn saving_throws_and_spell_dc() {
    let (mut w, a, b) = world();
    w.get_mut(b).saving_throws.insert(Ability::Wisdom);
    // d20 20 + 0 + 2 vs DC 23: natural 20 does not auto-succeed.
    let s = rules::roll_saving_throw(
        &w,
        &mut ScriptedDice::new(&[20, 1]),
        b,
        Ability::Wisdom,
        23,
        Advantage::default(),
    );
    assert_eq!((s.total, s.success), (22, false));
    let s = rules::roll_saving_throw(
        &w,
        &mut ScriptedDice::new(&[1, 1]),
        b,
        Ability::Wisdom,
        3,
        Advantage::default(),
    );
    assert!(s.success);
    w.get_mut(a).spellcasting_ability = Some(Ability::Strength);
    assert_eq!(rules::spell_dc(&w, a, None, 8), 8 + 2 + 3);
    assert_eq!(rules::spell_dc(&w, b, None, 8), 8 + 2 - 5);
}

#[test]
fn resistance_rounding() {
    assert_eq!(rules::apply_resistance(7, Resistance::Resistant), 3);
    assert_eq!(rules::apply_resistance(7, Resistance::Vulnerable), 14);
    assert_eq!(rules::apply_resistance(7, Resistance::Immune), 0);
    assert_eq!(rules::apply_resistance(-3, Resistance::Normal), 0);
    assert_eq!(
        rules::apply_resistance(100_000, Resistance::Normal),
        rules::DAMAGE_CLAMP
    );
}

// ---- spell casts --------------------------------------------------------------------------

fn melee_spell() -> Spell {
    let fields = [
        ("TargetConditions", "not Dead() and not Self()"),
        ("SpellRoll", "Attack(AttackType.MeleeWeaponAttack)"),
        (
            "SpellSuccess",
            "DealDamage(max(2,MainMeleeWeapon),MainMeleeWeaponDamageType);IF(Tagged('TEST_UNDEAD')):ApplyStatus(TEST_SHAKEN,100,2)",
        ),
        ("SpellProperties", "GROUND:Kill()"),
    ];
    Spell::from_fields("Test_Strike", |f| {
        fields.iter().find(|(k, _)| *k == f).map(|(_, v)| *v)
    })
    .unwrap()
}

#[test]
fn cast_melee_attack_hit_and_crit() {
    let (mut w, a, b) = world();
    let mut e = engine(&mut w);
    let spell = melee_spell();
    // Attack d20 pair (12, 1): hit; damage 1d8 → 7, +3 Str = 10, resisted → 5.
    let mut rng = ScriptedDice::new(&[12, 1, 7]);
    let mut ex = Executor::new(&mut e, &mut w, &mut rng);
    assert_eq!(
        ex.cast(&spell, a, b),
        CastOutcome::Success { critical: false }
    );
    let log = ex.log.join("\n");
    assert!(log.contains("deals 5 Slashing damage to Bob"), "{log}");
    assert!(
        log.contains("Bob gains status TEST_SHAKEN (2 turns)"),
        "{log}"
    );
    assert!(log.contains("GROUND pass not modelled"), "{log}");
    assert_eq!(w.hp(b), 4);
    assert!(w.has_status(b, "TEST_SHAKEN"));

    // Critical: d20 20 doubles the weapon dice: (8 + 8) + 3 = 19, resisted → 9 → Bob down.
    let mut rng = ScriptedDice::new(&[20, 1, 8, 8]);
    let mut ex = Executor::new(&mut e, &mut w, &mut rng);
    assert_eq!(
        ex.cast(&spell, a, b),
        CastOutcome::Success { critical: true }
    );
    assert_eq!(w.hp(b), 0);

    // Dead targets fail the target conditions.
    let mut rng = ScriptedDice::new(&[20]);
    let mut ex = Executor::new(&mut e, &mut w, &mut rng);
    assert_eq!(ex.cast(&spell, a, b), CastOutcome::InvalidTarget);
}

#[test]
fn cast_save_spell() {
    let (mut w, a, b) = world();
    w.get_mut(a).spellcasting_ability = Some(Ability::Strength);
    let mut e = engine(&mut w);
    let fields = [
        ("SpellRoll", "not SavingThrow(Ability.Dexterity, 13)"),
        ("SpellSuccess", "DealDamage(2d6,Fire)"),
        ("SpellFail", "DealDamage((2d6)/2,Fire)"),
    ];
    let spell = Spell::from_fields("Test_Burn", |f| {
        fields.iter().find(|(k, _)| *k == f).map(|(_, v)| *v)
    })
    .unwrap();
    // Save fails (5 vs 13): full 2d6 = 3 + 4.
    let mut rng = ScriptedDice::new(&[5, 1, 3, 4]);
    let mut ex = Executor::new(&mut e, &mut w, &mut rng);
    assert_eq!(
        ex.cast(&spell, a, b),
        CastOutcome::Success { critical: false }
    );
    assert_eq!(w.hp(b), 2);
    // Save succeeds: half of 2d6 (3 + 4 = 7 → 3).
    w.get_mut(b).hp = 9;
    let mut rng = ScriptedDice::new(&[18, 1, 3, 4]);
    let mut ex = Executor::new(&mut e, &mut w, &mut rng);
    assert_eq!(ex.cast(&spell, a, b), CastOutcome::Failure);
    assert_eq!(w.hp(b), 6);
}
