//! `bg3x rules …`: evaluate stats expressions against a sample context, survey the install's
//! expression fields, cast a spell between two sample combatants.

use anyhow::{Context, Result, bail};
use bg3_install::Install;
use bg3_pak::Package;
use bg3_rules::condition::{ConditionContext, ConditionEngine, RollConditions};
use bg3_rules::context::{Ability, DamageType, EntityId, RulesContext, Weapon};
use bg3_rules::dice::{RollDefinition, RollStream};
use bg3_rules::exec::{EntityEnv, Executor, Spell};
use bg3_rules::expr::{self, Evaluator, Mode};
use bg3_rules::functor::{Entry, FunctorList};
use bg3_rules::natives::RollEvent;
use bg3_rules::testworld::{Combatant, TestWorld};
use bg3_rules::{Condition, survey};
use bg3_stats::install::{BASE_LOAD_ORDER, HONOUR_MODULES, load};
use clap::{Subcommand, ValueEnum};

#[derive(Clone, Copy, ValueEnum)]
pub enum Kind {
    /// A condition (`TargetConditions`, `IF(...)` text): `not Self() and Enemy()`.
    Condition,
    /// A functor list (`SpellSuccess`, …): `DealDamage(1d6,Fire);ApplyStatus(X,100,2)`.
    Functor,
    /// Roll conditions (`SpellRoll`): `Attack(AttackType.MeleeWeaponAttack)`.
    Roll,
    /// A stats expression: `2d6+StrengthModifier`.
    Expr,
}

#[derive(Subcommand)]
pub enum RulesCmd {
    /// Parse and evaluate EXPRESSION with a sample Source ("Fighter") and Target ("Goblin").
    /// Condition helpers are loaded from the install when it is found.
    #[command(visible_alias = "e")]
    Eval {
        expression: String,
        #[arg(short, long, value_enum, default_value = "condition")]
        kind: Kind,
        /// Seed of the roll stream.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Don't load the helper scripts from the install.
        #[arg(long)]
        no_helpers: bool,
    },
    /// Parse every expression-valued stats field in the install and print counts per field.
    Survey {
        /// Also load the Honour-mode modules.
        #[arg(long)]
        honour: bool,
        /// Print every failure and dropped item.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Cast a spell (by stats name) from the sample Fighter at the sample Goblin, and back,
    /// for a few rounds.
    Cast {
        spell: String,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 3)]
        rounds: u32,
    },
}

/// Two synthetic combatants.
fn sample_world() -> (TestWorld, EntityId, EntityId) {
    let mut w = TestWorld::new();
    let mut fighter = Combatant::new("Fighter")
        .ability(Ability::Strength, 16)
        .ability(Ability::Dexterity, 12)
        .ability(Ability::Constitution, 14)
        .ability(Ability::Wisdom, 13)
        .hp(13)
        .ac(16)
        .team(1)
        .weapon(Weapon {
            name: "Longsword".into(),
            damage: RollDefinition::parse("1d8").unwrap(),
            damage_type: DamageType::Slashing,
            ranged: false,
            finesse: false,
            proficient: true,
        });
    fighter.player = true;
    fighter.saving_throws.insert(Ability::Strength);
    fighter.saving_throws.insert(Ability::Constitution);
    fighter.spellcasting_ability = Some(Ability::Wisdom);
    let mut goblin = Combatant::new("Goblin")
        .ability(Ability::Strength, 8)
        .ability(Ability::Dexterity, 14)
        .hp(9)
        .ac(15)
        .team(2)
        .weapon(Weapon {
            name: "Scimitar".into(),
            damage: RollDefinition::parse("1d6").unwrap(),
            damage_type: DamageType::Slashing,
            ranged: false,
            finesse: true,
            proficient: true,
        });
    goblin.tags.insert("GOBLIN".into());
    goblin.tags.insert("HUMANOID".into());
    let f = w.spawn(fighter);
    let g = w.spawn(goblin);
    (w, f, g)
}

fn engine(world: &mut TestWorld, rng: &mut RollStream, helpers: bool) -> Result<ConditionEngine> {
    let mut engine = ConditionEngine::new();
    if !helpers {
        return Ok(engine);
    }
    match Install::locate() {
        Ok(install) => {
            let vfs = bg3_vfs::Vfs::from_install(&install)?;
            let n =
                bg3_rules::install::load_helpers(&mut engine, &vfs, BASE_LOAD_ORDER, world, rng)?;
            world.level_maps = bg3_rules::levelmap::LevelMaps::load(&vfs, BASE_LOAD_ORDER);
            eprintln!(
                "// loaded {n} condition helper scripts and {} level maps from the install",
                world.level_maps.series.len()
            );
        }
        Err(e) => eprintln!("// install not found ({e}); no condition helpers loaded"),
    }
    Ok(engine)
}

fn print_rolls(world: &TestWorld, rolls: &[RollEvent]) {
    for r in rolls {
        match r {
            RollEvent::Attack(a) => println!(
                "  {} -> {}: {a}",
                world.name(a.attacker),
                world.name(a.target)
            ),
            RollEvent::SavingThrow(s) => println!("  {}: {s}", world.name(s.entity)),
            RollEvent::Die { die, value, dc } => println!("  d{} = {value} vs {dc}", die.faces()),
        }
    }
}

pub fn run(cmd: RulesCmd) -> Result<()> {
    match cmd {
        RulesCmd::Eval {
            expression,
            kind,
            seed,
            no_helpers,
        } => eval(&expression, kind, seed, !no_helpers),
        RulesCmd::Survey { honour, verbose } => survey_cmd(honour, verbose),
        RulesCmd::Cast {
            spell,
            seed,
            rounds,
        } => cast(&spell, seed, rounds),
    }
}

fn eval(text: &str, kind: Kind, seed: u64, helpers: bool) -> Result<()> {
    let (mut world, fighter, goblin) = sample_world();
    let mut rng = RollStream::new(seed);
    let data = ConditionContext {
        source: Some(fighter),
        target: Some(goblin),
        ..Default::default()
    };
    match kind {
        Kind::Condition => {
            let mut engine = engine(&mut world, &mut rng, helpers)?;
            let c = Condition::parse(text).context("parse")?;
            println!("lua: {}", c.lua.replace('\n', "\\n"));
            let ev = engine.evaluate(&c, &mut world, &mut rng, &data)?;
            let r = &ev.result;
            println!("result: {} (chance {})", r.result, r.chance);
            for e in &r.active_errors {
                println!("  active error: {}", e.name);
            }
            for e in &r.inactive_errors {
                println!("  inactive error: {}", e.name);
            }
            print_rolls(&world, &ev.rolls);
        }
        Kind::Roll => {
            let mut engine = engine(&mut world, &mut rng, helpers)?;
            let rc = RollConditions::parse(text).context("parse")?;
            for (key, c) in &rc.groups {
                let ev = engine.evaluate(c, &mut world, &mut rng, &data)?;
                println!("[{key}] {} -> {}", c.source, ev.result.result);
                print_rolls(&world, &ev.rolls);
            }
        }
        Kind::Functor => {
            let mut engine = engine(&mut world, &mut rng, helpers)?;
            let list = FunctorList::parse(text).context("parse")?;
            for (key, entries) in &list.groups {
                println!("[{key}]");
                for e in entries {
                    match e {
                        Entry::Functor(f) => {
                            print!("  {} contexts={} target={:?}", f.name, f.contexts, f.target);
                            if let Some(c) = &f.condition {
                                print!(" if=`{}`", c.source);
                            }
                            println!(" params={:?}", f.params);
                        }
                        Entry::Dropped { text, reason } => println!("  dropped `{text}`: {reason}"),
                    }
                }
            }
            println!("-- executing on Fighter -> Goblin (target pass)");
            let mut ex = Executor::new(&mut engine, &mut world, &mut rng);
            ex.run_list(&list, fighter, goblin, false);
            for l in &ex.log {
                println!("{l}");
            }
        }
        Kind::Expr => {
            engine(&mut world, &mut rng, helpers)?;
            // Weapon keywords become Placeholder0, as in DealDamage.
            let mut src = text.to_owned();
            let mut weapon = None;
            if let Some(kw) = bg3_rules::functor::WEAPON_KEYWORDS
                .iter()
                .find(|k| text.contains(*k))
            {
                src = text.replacen(kw, "Placeholder0", 1);
                weapon = Some(kw.to_string());
            }
            let e = expr::parse(&src).context("parse")?;
            println!("ast: {e}");
            let mut env = EntityEnv {
                ctx: &world,
                source: fighter,
                target: Some(goblin),
                weapon,
                base_spell_dc: bg3_rules::rules::DEFAULT_BASE_SPELL_DC,
            };
            let avg = Evaluator::new(&mut env, &mut rng).eval(&e, Mode::Average)?;
            println!("average: {avg}");
            let mut ev = Evaluator::new(&mut env, &mut rng);
            let v = ev.eval(&e, Mode::Roll { critical: false })?;
            let rolls: Vec<String> = ev
                .rolls
                .iter()
                .map(|r| format!("{}={}", r.roll, r.total))
                .collect();
            println!("rolled: {v} [{}]", rolls.join(", "));
        }
    }
    Ok(())
}

fn install_stats(honour: bool) -> Result<(Vec<Package>, bg3_stats::Stats)> {
    let paks: Vec<Package> = Install::locate()?
        .paks()?
        .iter()
        .map(Package::open)
        .collect::<Result<_, _>>()?;
    let mut order = BASE_LOAD_ORDER.to_vec();
    if honour {
        order.extend_from_slice(HONOUR_MODULES);
    }
    let stats = load(&paks, &order)?;
    Ok((paks, stats))
}

fn survey_cmd(honour: bool, verbose: bool) -> Result<()> {
    let (_, stats) = install_stats(honour)?;
    let s = survey::survey(&stats);
    println!(
        "{:<18} {:<34} {:>7} {:>7} {:>8}",
        "language", "field", "values", "ok", "dropped"
    );
    for ((lang, field), t) in &s.by_field {
        println!(
            "{:<18} {:<34} {:>7} {:>7} {:>8}",
            format!("{lang:?}"),
            field,
            t.values,
            t.ok,
            t.dropped_items
        );
        if verbose {
            for (entry, value, err) in &t.failures {
                println!("    FAIL {entry}: {value:?}: {err}");
            }
        }
    }
    println!("failures: {}", s.failures());
    println!("items the engine drops: {}", s.dropped.len());
    if verbose {
        for (entry, text, reason) in &s.dropped {
            println!("    {entry}: {text:?}: {reason}");
        }
        println!("literal description params: {}", s.literal_params.len());
        for (entry, text) in &s.literal_params {
            println!("    {entry}: {text:?}");
        }
    }
    println!(
        "distinct functions called by conditions: {}",
        s.condition_calls.len()
    );
    println!("functor types used: {}", s.functor_names.len());
    println!("boost types used: {}", s.boost_names.len());
    Ok(())
}

fn cast(name: &str, seed: u64, rounds: u32) -> Result<()> {
    let (_, stats) = install_stats(false)?;
    let obj = stats
        .get(name)
        .with_context(|| format!("no stats object named {name}"))?;
    if obj.type_name.as_deref() != Some("SpellData") {
        bail!("{name} is not a SpellData entry");
    }
    let spell = Spell::from_fields(&obj.name, |f| obj.get(f))?;
    let (mut world, fighter, goblin) = sample_world();
    let mut rng = RollStream::new(seed);
    let mut engine = engine(&mut world, &mut rng, true)?;
    let mut ex = Executor::new(&mut engine, &mut world, &mut rng);
    for round in 1..=rounds {
        if ex.ctx.hp(fighter) == 0 || ex.ctx.hp(goblin) == 0 {
            break;
        }
        ex.log.push(format!("-- round {round}"));
        ex.cast(&spell, fighter, goblin);
        if ex.ctx.hp(goblin) > 0 {
            ex.cast(&spell, goblin, fighter);
        }
    }
    for l in &ex.log {
        println!("{l}");
    }
    Ok(())
}
