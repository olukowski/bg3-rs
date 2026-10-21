//! Expected outcome of a spell on a target, for the AI's score: whether the target is
//! valid, the chance the spell roll succeeds, and the expected effects of the functors.
//!
//! The engine simulates spells per spell type (`esv::spell_state::target::…`,
//! `…::projectile::…`; `CalculateSpellAiSimulate`, `CalculateSpellAiScoreCharacter`) and scores
//! the functors with `esv::AiHelpers::CalculateAiScoreSpellStatsFunctors` (0x105248600) /
//! `CalculateAiScoreStatsFunctors` (0x105223ba0): success functors weighted by the believed
//! chance (see [`score::believed_chance`]), fail functors by its complement, damage from
//! `CalculateAiScoreDamageCharacter` (0x10522eb80) with the resistance "stupidity" blend.
//! Here: single-target spells (`Target`, `Projectile`) through the rules engine, with dice
//! averages (`Mode::Average`) and exact d20 odds; statuses are not scored yet.

use std::collections::HashMap;

use bg3_rules::condition::{Condition, ConditionContext, ConditionEngine};
use bg3_rules::context::{DamageType, EntityId, RulesContext};
use bg3_rules::dice::ScriptedDice;
use bg3_rules::exec::{EntityEnv, Spell};
use bg3_rules::expr::{Evaluator, Mode};
use bg3_rules::functor::{self, DamageTypeArg, FunctorList, Params, TargetSelector};
use bg3_rules::natives::RollEvent;
use bg3_rules::rules;
use bg3_rules::textkey::DEFAULT_KEY;

use super::archetype::{self as k, Modifiers};
use super::score::{AiScore, believed_chance};
use crate::guid::Guid;
use crate::sim::Sim;

/// Per-turn planning state: the condition engine and cached averages.
pub struct Planner {
    pub(super) engine: ConditionEngine,
    average_damage: HashMap<Guid, f32>,
    /// Attack-of-opportunity simulations (attacker, mover) of the current plan.
    pub(super) opportunity: HashMap<(Guid, Guid), Option<AiScore>>,
}

/// What simulating a spell on a target gave.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellSim {
    /// Chance that the spell roll succeeds (1 without a roll).
    pub chance: f32,
    /// The chance as the archetype believes it.
    pub believed: f32,
    pub score: AiScore,
}

/// The spell types simulated (single target).
pub fn simulated_type(spell_type: &str) -> bool {
    matches!(spell_type, "Target" | "Projectile")
}

impl Sim {
    pub(super) fn ai_planner(&mut self) -> Planner {
        Planner {
            engine: self.condition_engine(),
            average_damage: HashMap::new(),
            opportunity: HashMap::new(),
        }
    }

    /// The resolved archetype modifiers of a character (`Archetype` of its object or
    /// template, else `base`); our neutral table when the install's archetypes are not
    /// loaded.
    pub fn ai_modifiers(&self, who: Guid) -> Modifiers {
        let a = &self.data.combat.ai;
        if a.is_empty() {
            return Modifiers::neutral();
        }
        let name = self
            .object_combat(who)
            .archetype
            .unwrap_or_else(|| super::archetype::BASE.to_owned());
        a.resolve(&name)
    }

    fn ai_eval(
        &mut self,
        pl: &mut Planner,
        cond: &Condition,
        data: &ConditionContext,
        dice: &[u32],
    ) -> Option<(bool, Vec<RollEvent>)> {
        let mut rng = ScriptedDice::new(dice);
        pl.engine
            .evaluate(cond, self, &mut rng, data)
            .ok()
            .map(|e| (e.result.result, e.rolls))
    }

    /// Whether `target` passes the spell's `TargetConditions` (script errors count as false,
    /// as in the cast).
    pub(super) fn ai_target_valid(
        &mut self,
        pl: &mut Planner,
        spell: &Spell,
        src: EntityId,
        tgt: EntityId,
    ) -> bool {
        let Some(tc) = &spell.target_conditions else {
            return true;
        };
        let data = ConditionContext {
            source: Some(src),
            target: Some(tgt),
            spell: Some(spell.name.clone()),
            ..Default::default()
        };
        self.ai_eval(pl, tc, &data, &[10]).is_some_and(|(ok, _)| ok)
    }

    /// The exact chance that the spell roll succeeds: the roll condition evaluated for every
    /// d20 result (every pair with advantage or disadvantage).
    pub(super) fn ai_success_chance(
        &mut self,
        pl: &mut Planner,
        spell: &Spell,
        src: EntityId,
        tgt: EntityId,
    ) -> f32 {
        let Some(roll) = spell.spell_roll.as_ref().and_then(|r| r.get(DEFAULT_KEY)) else {
            return 1.0;
        };
        let data = ConditionContext {
            source: Some(src),
            target: Some(tgt),
            spell: Some(spell.name.clone()),
            ..Default::default()
        };
        let Some((ok, rolls)) = self.ai_eval(pl, roll, &data, &[10, 10]) else {
            return 0.0;
        };
        let mut d20 = false;
        let mut paired = false;
        for r in &rolls {
            let adv = match r {
                RollEvent::Attack(a) => Some(a.advantage),
                RollEvent::SavingThrow(s) => Some(s.advantage),
                RollEvent::Die { .. } => None,
            };
            if let Some(a) = adv {
                d20 = true;
                paired |= a.advantage != a.disadvantage;
            }
        }
        if !d20 {
            return if ok { 1.0 } else { 0.0 };
        }
        let mut hits = 0u32;
        let mut total = 0u32;
        for a in 1..=20u32 {
            let bs: Vec<u32> = if paired { (1..=20).collect() } else { vec![a] };
            for b in bs {
                total += 1;
                if self
                    .ai_eval(pl, roll, &data, &[a, b])
                    .is_some_and(|(ok, _)| ok)
                {
                    hits += 1;
                }
            }
        }
        hits as f32 / total as f32
    }

    /// Expected effects of a functor list, each weighted by `weight`.
    // bg3: esv::AiHelpers::CalculateAiScoreStatsFunctors (0x105223ba0) and
    // CalculateAiScoreDamageCharacter (0x10522eb80, 4.1.1.7631656). Functor conditions
    // (`IF(…)`) are assumed true; statuses and the other functors are not scored yet.
    fn ai_functor_effects(
        &self,
        m: &Modifiers,
        list: &FunctorList,
        src: EntityId,
        tgt: EntityId,
        weight: f32,
        out: &mut AiScore,
    ) {
        if weight == 0.0 {
            return;
        }
        for entry in list.group(DEFAULT_KEY) {
            let functor::Entry::Functor(f) = entry else {
                continue;
            };
            let c = f.contexts;
            if c.has(functor::CTX_AI_IGNORE)
                || !(c.has(functor::CTX_TARGET)
                    || c.has(functor::CTX_AOE)
                    || c.has(functor::CTX_AI_ONLY))
            {
                continue;
            }
            let (s, t) = match f.target {
                TargetSelector::Target => (src, tgt),
                TargetSelector::Source => (src, src),
                TargetSelector::Swap => (tgt, src),
                _ => continue,
            };
            let Some(victim) = self.entity_guid(t) else {
                continue;
            };
            match &f.params {
                Params::DealDamage(d) => {
                    let ty = match &d.damage_type {
                        Some(DamageTypeArg::Type(t)) => Some(*t),
                        Some(DamageTypeArg::Weapon(kw)) => {
                            rules::weapon_damage_roll(self, s, kw).map(|(_, t)| t)
                        }
                        None => None,
                    };
                    let Some(raw) = self.ai_average(&d.amount, s, Some(t), d.weapon.clone()) else {
                        continue;
                    };
                    let amount = ty.map_or(raw as f32, |ty| self.ai_resisted(m, raw, t, ty));
                    out.add_damage(victim, amount * weight, 0);
                }
                Params::RegainHitPoints { amount } => {
                    if let Some(v) = self.ai_average(amount, s, Some(t), None) {
                        out.add_heal(victim, v as f32 * weight);
                    }
                }
                _ => {}
            }
        }
    }

    /// Damage after resistances as the archetype reasons about them: fully resisted →
    /// `raw * MULTIPLIER_IMMUNITY_STUPIDITY`; else the resisted amount moved towards the raw
    /// one by `MULTIPLIER_RESISTANCE_STUPIDITY`.
    // bg3: esv::AiHelpers::CalculateAiScoreDamageCharacter (0x10522eb80, 4.1.1.7631656)
    fn ai_resisted(&self, m: &Modifiers, raw: i32, tgt: EntityId, ty: DamageType) -> f32 {
        let res = rules::apply_resistance(raw, self.resistance(tgt, ty));
        if res < 1 {
            if raw > 0 {
                raw as f32 * m.get(k::MULTIPLIER_IMMUNITY_STUPIDITY)
            } else {
                0.0
            }
        } else if res != raw {
            res as f32 + (raw - res) as f32 * m.get(k::MULTIPLIER_RESISTANCE_STUPIDITY)
        } else {
            res as f32
        }
    }

    fn ai_average(
        &self,
        e: &bg3_rules::expr::Expr,
        src: EntityId,
        tgt: Option<EntityId>,
        weapon: Option<String>,
    ) -> Option<i32> {
        let mut env = EntityEnv {
            ctx: self,
            source: src,
            target: tgt,
            weapon,
            base_spell_dc: self.data.combat.base_spell_dc,
        };
        let mut rng = ScriptedDice::new(&[1]);
        Evaluator::new(&mut env, &mut rng)
            .eval(e, Mode::Average)
            .ok()
    }

    /// Simulate `spell` from `src` on `tgt` with the actor's modifiers `m`. `None` when the
    /// spell can't be simulated or the target is invalid.
    pub(super) fn ai_simulate(
        &mut self,
        pl: &mut Planner,
        m: &Modifiers,
        spell: &str,
        src: Guid,
        tgt: Guid,
    ) -> Option<SpellSim> {
        let def = self.data.combat.spells.get(spell)?;
        if !simulated_type(&def.spell_type) {
            return None;
        }
        let exec = def.to_exec().ok()?;
        let (s, t) = (self.entity_id(src)?, self.entity_id(tgt)?);
        if !self.ai_target_valid(pl, &exec, s, t) {
            return None;
        }
        let chance = self.ai_success_chance(pl, &exec, s, t);
        let believed = believed_chance(m, chance);
        let mut score = AiScore::default();
        self.ai_functor_effects(m, &exec.spell_success, s, t, believed, &mut score);
        self.ai_functor_effects(m, &exec.spell_fail, s, t, 1.0 - believed, &mut score);
        self.ai_functor_effects(m, &exec.spell_properties, s, t, 1.0, &mut score);
        Some(SpellSim {
            chance,
            believed,
            score,
        })
    }

    /// The damage the AI's scores are measured in: the best average damage of the
    /// character's spells (`DealDamage` of `SpellSuccess`, dice averaged, no target).
    // bg3: esv::AiHelpers::GetAverageDamageForCharacterNoCache (0x10523a384,
    // 4.1.1.7631656): the maximum over the spell book (one spell type skipped, read as
    // `Throw`) of the summed damage of `esv::LEGACY_GetSpellDamage`, divided by the attack
    // cost amount (1).
    pub(super) fn ai_average_damage(&mut self, pl: &mut Planner, who: Guid) -> f32 {
        if let Some(v) = pl.average_damage.get(&who) {
            return *v;
        }
        let mut best = 0.0f32;
        let spells = self
            .combatant(who)
            .map(|c| c.spells.clone())
            .unwrap_or_default();
        if let Some(src) = self.entity_id(who) {
            for s in spells {
                let Some(def) = self.data.combat.spells.get(&s) else {
                    continue;
                };
                if def.spell_type == "Throw" {
                    continue;
                }
                let Ok(exec) = def.to_exec() else {
                    continue;
                };
                let mut sum = 0.0f32;
                for entry in exec.spell_success.group(DEFAULT_KEY) {
                    if let functor::Entry::Functor(f) = entry
                        && let Params::DealDamage(d) = &f.params
                        && let Some(v) = self.ai_average(&d.amount, src, None, d.weapon.clone())
                    {
                        sum += v.max(0) as f32;
                    }
                }
                best = best.max(sum);
            }
        }
        pl.average_damage.insert(who, best);
        best
    }
}
