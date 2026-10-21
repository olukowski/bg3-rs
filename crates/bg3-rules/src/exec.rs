//! Running functors and simple spell casts against a [`RulesContext`], with a readable log.
//!
//! The cast pipeline modelled here is the single-target core of a spell: target conditions,
//! the default `SpellRoll`, then `SpellSuccess` or `SpellFail`, then `SpellProperties`, all in
//! the target pass (functors with a `TARGET`/`AOE` context). Ground passes, projectiles, AoE
//! target gathering and the extra text-key groups (`Cast2[…]`, `CastOffhand[…]`) are not
//! modelled yet and are reported in the log when skipped.

use crate::condition::{Condition, ConditionContext, ConditionEngine, RollConditions};
use crate::context::{EntityId, RulesContext};
use crate::dice::{DiceRng, RollDefinition};
use crate::expr::{
    BuiltinVar, EvalError, Evaluator, ExprEnv, Mode, Number, VarContext, VarKind, Variable,
};
use crate::functor::{self, DamageTypeArg, Entry, Functor, FunctorList, Params, TargetSelector};
use crate::natives::RollEvent;
use crate::rules;
use crate::textkey::DEFAULT_KEY;

/// The fields of a `SpellData` entry that the cast pipeline uses.
#[derive(Debug, Clone, Default)]
pub struct Spell {
    pub name: String,
    pub target_conditions: Option<Condition>,
    pub spell_roll: Option<RollConditions>,
    pub spell_success: FunctorList,
    pub spell_fail: FunctorList,
    pub spell_properties: FunctorList,
}

#[derive(Debug, thiserror::Error)]
#[error("{field}: {message}")]
pub struct SpellError {
    pub field: &'static str,
    pub message: String,
}

impl Spell {
    /// Build from field values (`get("SpellRoll")` etc.), e.g. a `bg3_stats::StatsObject`.
    pub fn from_fields<'a>(
        name: &str,
        get: impl Fn(&str) -> Option<&'a str>,
    ) -> Result<Self, SpellError> {
        let err = |field: &'static str| {
            move |e: &dyn std::fmt::Display| SpellError {
                field,
                message: e.to_string(),
            }
        };
        let nonempty = |f: &str| get(f).filter(|v| !v.trim().is_empty());
        let functors = |f: &'static str| -> Result<FunctorList, SpellError> {
            nonempty(f).map_or(Ok(FunctorList::default()), |v| {
                FunctorList::parse(v).map_err(|e| err(f)(&e))
            })
        };
        Ok(Self {
            name: name.to_owned(),
            target_conditions: nonempty("TargetConditions")
                .map(Condition::parse)
                .transpose()
                .map_err(|e| err("TargetConditions")(&e))?,
            spell_roll: nonempty("SpellRoll")
                .map(RollConditions::parse)
                .transpose()
                .map_err(|e| err("SpellRoll")(&e))?,
            spell_success: functors("SpellSuccess")?,
            spell_fail: functors("SpellFail")?,
            spell_properties: functors("SpellProperties")?,
        })
    }
}

/// Resolves stats-expression variables for one entity (the functor's source).
pub struct EntityEnv<'a> {
    pub ctx: &'a dyn RulesContext,
    pub source: EntityId,
    pub target: Option<EntityId>,
    /// What `Placeholder0` stands for (`DealDamage` weapon keyword).
    pub weapon: Option<String>,
    pub base_spell_dc: i32,
}

impl ExprEnv for EntityEnv<'_> {
    fn variable(&mut self, v: &Variable) -> Result<Number, EvalError> {
        let e = match v.context {
            VarContext::Default => self.source,
            VarContext::Target => self
                .target
                .ok_or_else(|| EvalError::Unsupported(format!("{}: no target", v.text)))?,
            _ => return Err(EvalError::Unsupported(v.text.clone())),
        };
        let c = self.ctx;
        let ability_value = |a| {
            if v.modifiers.modifier {
                c.ability_modifier(e, a)
            } else {
                c.ability_score(e, a)
            }
        };
        Ok(match &v.kind {
            VarKind::Ability(a) => ability_value(*a),
            VarKind::Builtin(BuiltinVar::ProficiencyBonus) => c.proficiency_bonus(e),
            VarKind::Builtin(BuiltinVar::Level) => c.level(e),
            VarKind::Builtin(BuiltinVar::CurrentHP) => c.hp(e),
            VarKind::Builtin(BuiltinVar::MaxHP) => c.max_hp(e),
            VarKind::Builtin(BuiltinVar::SpellCastingAbility) => match c.spellcasting_ability(e) {
                Some(a) => ability_value(a),
                None => {
                    return Err(EvalError::Unsupported(format!(
                        "{}: no spellcasting ability",
                        v.text
                    )));
                }
            },
            VarKind::Builtin(BuiltinVar::UnarmedMeleeAbility) => {
                ability_value(c.unarmed_attack_ability(e))
            }
            VarKind::Builtin(BuiltinVar::SpellDC) => {
                rules::spell_dc(c, e, None, self.base_spell_dc)
            }
            _ => return Err(EvalError::UnknownVariable(v.text.clone())),
        })
    }

    fn level_map(&mut self, v: &Variable, name: &str) -> Result<RollDefinition, EvalError> {
        let e = match v.context {
            VarContext::Target => self.target.unwrap_or(self.source),
            _ => self.source,
        };
        self.ctx
            .level_map_value(e, name)
            .ok_or_else(|| EvalError::Unsupported(format!("LevelMapValue({name})")))
    }

    fn placeholder(&mut self, n: u32) -> Result<RollDefinition, EvalError> {
        match (n, &self.weapon) {
            (0, Some(kw)) => rules::weapon_damage_roll(self.ctx, self.source, kw)
                .map(|(r, _)| r)
                .ok_or_else(|| EvalError::Unsupported(format!("{kw}: no weapon"))),
            _ => Err(EvalError::Unsupported(format!("Placeholder{n}"))),
        }
    }
}

/// What happened during a cast, structured (the same as the text log, for front ends and
/// combat logs).
#[derive(Debug, Clone, PartialEq)]
pub enum ExecEvent {
    /// A roll made by a condition (attack roll, saving throw, raw die).
    Roll(RollEvent),
    Damage {
        source: EntityId,
        target: EntityId,
        damage_type: crate::context::DamageType,
        /// Before resistances.
        raw: i32,
        /// Dealt.
        amount: i32,
        critical: bool,
        /// Each dice roll as `NdM=total`.
        rolls: Vec<String>,
        hp_after: i32,
    },
    StatusApplied {
        source: EntityId,
        target: EntityId,
        status: String,
        /// Turns; -1 = until removed.
        duration: i32,
    },
    StatusRemoved {
        target: EntityId,
        status: String,
    },
    Healed {
        target: EntityId,
        amount: i32,
        hp_after: i32,
    },
}

/// What a cast did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastOutcome {
    InvalidTarget,
    Success { critical: bool },
    Failure,
}

/// Runs functors and casts; collects a human-readable log.
pub struct Executor<'a> {
    pub engine: &'a mut ConditionEngine,
    pub ctx: &'a mut dyn RulesContext,
    pub rng: &'a mut dyn DiceRng,
    pub log: Vec<String>,
    pub events: Vec<ExecEvent>,
}

impl<'a> Executor<'a> {
    pub fn new(
        engine: &'a mut ConditionEngine,
        ctx: &'a mut dyn RulesContext,
        rng: &'a mut dyn DiceRng,
    ) -> Self {
        Self {
            engine,
            ctx,
            rng,
            log: Vec::new(),
            events: Vec::new(),
        }
    }

    fn name(&self, e: EntityId) -> String {
        self.ctx.name(e)
    }

    fn say(&mut self, line: String) {
        self.log.push(line);
    }

    fn log_rolls(&mut self, rolls: &[RollEvent]) {
        for r in rolls {
            self.events.push(ExecEvent::Roll(r.clone()));
            let line = match r {
                RollEvent::Attack(a) => format!(
                    "  {} attacks {}: {a}",
                    self.name(a.attacker),
                    self.name(a.target)
                ),
                RollEvent::SavingThrow(s) => format!("  {}: {s}", self.name(s.entity)),
                RollEvent::Die { die, value, dc } => {
                    format!("  d{} = {value} vs {dc}", die.faces())
                }
            };
            self.say(line);
        }
    }

    /// Evaluate a condition; script errors count as false (as in the engine) and are logged.
    pub fn check(&mut self, cond: &Condition, data: &ConditionContext) -> (bool, Vec<RollEvent>) {
        match self.engine.evaluate(cond, self.ctx, self.rng, data) {
            Ok(ev) => (ev.result.result, ev.rolls),
            Err(e) => {
                self.say(format!(
                    "  condition `{}` failed to evaluate: {e}",
                    cond.source
                ));
                (false, Vec::new())
            }
        }
    }

    /// Cast `spell` from `source` at `target`.
    pub fn cast(&mut self, spell: &Spell, source: EntityId, target: EntityId) -> CastOutcome {
        let line = format!(
            "{} casts {} on {}",
            self.name(source),
            spell.name,
            self.name(target)
        );
        self.say(line);
        let data = ConditionContext {
            source: Some(source),
            target: Some(target),
            spell: Some(spell.name.clone()),
            ..Default::default()
        };
        if let Some(tc) = &spell.target_conditions {
            let (ok, _) = self.check(tc, &data);
            if !ok {
                self.say(format!("  target conditions `{}` not met", tc.source));
                return CastOutcome::InvalidTarget;
            }
        }
        let mut critical = false;
        let success = match spell.spell_roll.as_ref().and_then(|r| r.get(DEFAULT_KEY)) {
            Some(roll) => {
                let (ok, rolls) = self.check(roll, &data);
                critical = rolls.iter().any(
                    |r| matches!(r, RollEvent::Attack(a) if a.critical == rules::Critical::Success),
                );
                self.log_rolls(&rolls);
                ok
            }
            None => true,
        };
        let list = if success {
            &spell.spell_success
        } else {
            &spell.spell_fail
        };
        self.run_list(list, source, target, critical);
        self.run_list(&spell.spell_properties, source, target, critical);
        let mut skipped: Vec<&str> = [
            &spell.spell_success,
            &spell.spell_fail,
            &spell.spell_properties,
        ]
        .iter()
        .flat_map(|l| l.groups.iter().map(|(k, _)| k.as_str()))
        .filter(|k| *k != DEFAULT_KEY)
        .collect();
        skipped.sort_unstable();
        skipped.dedup();
        if !skipped.is_empty() {
            self.say(format!(
                "  (text-key groups not modelled: {})",
                skipped.join(", ")
            ));
        }
        if success {
            CastOutcome::Success { critical }
        } else {
            CastOutcome::Failure
        }
    }

    /// Run the default group of a functor list in the target pass.
    pub fn run_list(
        &mut self,
        list: &FunctorList,
        source: EntityId,
        target: EntityId,
        critical: bool,
    ) {
        for entry in list.group(DEFAULT_KEY) {
            match entry {
                Entry::Functor(f) => self.run_functor(f, source, target, critical),
                Entry::Dropped { text, reason } => {
                    self.say(format!("  (dropped `{text}`: {reason})"))
                }
            }
        }
    }

    /// Run one functor in the target pass.
    pub fn run_functor(&mut self, f: &Functor, source: EntityId, target: EntityId, critical: bool) {
        if !f.contexts.has(functor::CTX_TARGET) && !f.contexts.has(functor::CTX_AOE) {
            self.say(format!(
                "  (skipped `{}`: {} pass not modelled)",
                f.text, f.contexts
            ));
            return;
        }
        if f.contexts.has(functor::CTX_AI_ONLY) {
            return;
        }
        let data = ConditionContext {
            source: Some(source),
            target: Some(target),
            ..Default::default()
        };
        if let Some(c) = &f.condition {
            let (ok, rolls) = self.check(c, &data);
            self.log_rolls(&rolls);
            if !ok {
                self.say(format!("  IF({}) is false: skip {}", c.source, f.name));
                return;
            }
        }
        let (src, tgt) = match f.target {
            TargetSelector::Target => (source, target),
            TargetSelector::Source => (source, source),
            TargetSelector::Swap => (target, source),
            other => {
                self.say(format!(
                    "  (skipped `{}`: target {other:?} not modelled)",
                    f.text
                ));
                return;
            }
        };
        match &f.params {
            Params::DealDamage(d) => self.deal_damage(d, src, tgt, critical),
            Params::ApplyStatus(a) => {
                if let Some(c) = &a.condition {
                    let (ok, rolls) = self.check(c, &data);
                    self.log_rolls(&rolls);
                    if !ok {
                        self.say(format!("  {} resists {}", self.name(tgt), a.status));
                        return;
                    }
                }
                let duration = match &a.duration {
                    Some(e) => {
                        match self.eval(e, src, Some(tgt), None, Mode::Roll { critical: false }) {
                            Ok(v) => v,
                            Err(err) => {
                                self.say(format!("  ApplyStatus duration: {err}"));
                                return;
                            }
                        }
                    }
                    None => 1,
                };
                self.ctx.apply_status(tgt, &a.status, duration, Some(src));
                self.events.push(ExecEvent::StatusApplied {
                    source: src,
                    target: tgt,
                    status: a.status.clone(),
                    duration,
                });
                let line = format!(
                    "  {} gains status {} ({duration} turns)",
                    self.name(tgt),
                    a.status
                );
                self.say(line);
            }
            Params::RemoveStatus { status } => {
                self.ctx.remove_status(tgt, status);
                self.events.push(ExecEvent::StatusRemoved {
                    target: tgt,
                    status: status.clone(),
                });
                let line = format!("  {} loses status {status}", self.name(tgt));
                self.say(line);
            }
            Params::RegainHitPoints { amount } => {
                match self.eval(amount, src, Some(tgt), None, Mode::Roll { critical: false }) {
                    Ok(n) => {
                        let hp = (self.ctx.hp(tgt) + n.max(0)).min(self.ctx.max_hp(tgt));
                        self.ctx.set_hp(tgt, hp);
                        self.events.push(ExecEvent::Healed {
                            target: tgt,
                            amount: n.max(0),
                            hp_after: hp,
                        });
                        let line = format!(
                            "  {} regains {n} HP ({hp}/{})",
                            self.name(tgt),
                            self.ctx.max_hp(tgt)
                        );
                        self.say(line);
                    }
                    Err(e) => self.say(format!("  RegainHitPoints: {e}")),
                }
            }
            Params::Raw => self.say(format!("  (functor {} not implemented)", f.name)),
        }
    }

    fn eval(
        &mut self,
        e: &crate::expr::Expr,
        source: EntityId,
        target: Option<EntityId>,
        weapon: Option<String>,
        mode: Mode,
    ) -> Result<Number, EvalError> {
        let mut env = EntityEnv {
            ctx: &*self.ctx,
            source,
            target,
            weapon,
            base_spell_dc: self.engine.base_spell_dc,
        };
        Evaluator::new(&mut env, self.rng).eval(e, mode)
    }

    fn deal_damage(
        &mut self,
        d: &functor::DealDamage,
        src: EntityId,
        tgt: EntityId,
        critical: bool,
    ) {
        let damage_type = match &d.damage_type {
            Some(DamageTypeArg::Type(t)) => Some(*t),
            Some(DamageTypeArg::Weapon(kw)) => {
                rules::weapon_damage_roll(&*self.ctx, src, kw).map(|(_, t)| t)
            }
            None => None,
        };
        let Some(damage_type) = damage_type else {
            self.say("  DealDamage: no damage type".into());
            return;
        };
        let mut env = EntityEnv {
            ctx: &*self.ctx,
            source: src,
            target: Some(tgt),
            weapon: d.weapon.clone(),
            base_spell_dc: self.engine.base_spell_dc,
        };
        let mut ev = Evaluator::new(&mut env, self.rng);
        let raw = match ev.eval(&d.amount, Mode::Roll { critical }) {
            Ok(v) => v,
            Err(e) => {
                self.say(format!("  DealDamage: {e}"));
                return;
            }
        };
        let rolls: Vec<String> = ev
            .rolls
            .iter()
            .map(|r| format!("{}={}", r.roll, r.total))
            .collect();
        let resistance = self.ctx.resistance(tgt, damage_type);
        let amount = rules::apply_resistance(raw, resistance);
        let hp = (self.ctx.hp(tgt) - amount).max(0);
        self.ctx.set_hp(tgt, hp);
        self.events.push(ExecEvent::Damage {
            source: src,
            target: tgt,
            damage_type,
            raw,
            amount,
            critical,
            rolls: rolls.clone(),
            hp_after: hp,
        });
        let mut line = format!(
            "  {} deals {amount} {damage_type} damage to {}",
            self.name(src),
            self.name(tgt)
        );
        if !rolls.is_empty() {
            line.push_str(&format!(
                " [{}{}]",
                if critical { "critical, " } else { "" },
                rolls.join(", ")
            ));
        }
        if amount != raw {
            line.push_str(&format!(" ({raw} before {resistance:?})"));
        }
        line.push_str(&format!(" -> {hp}/{} HP", self.ctx.max_hp(tgt)));
        if hp == 0 {
            line.push_str(", down");
        }
        self.say(line);
    }
}
