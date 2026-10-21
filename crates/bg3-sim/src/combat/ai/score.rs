//! The AI's score of one action: expected effects per affected character, weighted by the
//! archetype's modifiers and summed.
//!
//! The model follows `esv::AiScore` / `esv::AiScoreInternal` (4.1.1.7631656): an action's
//! score is a record per affected entity with damage, heal, damage and heal over time,
//! control, boost, a raw part (position and movement terms) and an instakill chance. Spell
//! simulation adds expected values to it (scaled by the chance that they happen), position
//! and movement add raw terms on the actor, and [`calculate`] turns the records into one
//! number. Written from our reading of the binary; see `docs/research/ai.md`.

use crate::guid::Guid;

use super::archetype::{self as k, Modifiers};

/// Expected effects of an action on one entity (`AiScoreInternal`'s per-entity record).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Effects {
    pub damage: f32,
    pub heal: f32,
    pub dot: f32,
    pub hot: f32,
    pub control: f32,
    pub boost: f32,
    /// Position and movement terms (`AddRaw`); scored with the kill/relation factor only.
    pub raw: f32,
    pub instakill: f32,
}

impl Effects {
    fn scaled(self, f: f32) -> Self {
        Self {
            damage: self.damage * f,
            heal: self.heal * f,
            dot: self.dot * f,
            hot: self.hot * f,
            control: self.control * f,
            boost: self.boost * f,
            raw: self.raw * f,
            instakill: self.instakill * f,
        }
    }
}

/// An action's effects on every entity it touches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AiScore {
    pub entries: Vec<(Guid, Effects)>,
}

impl AiScore {
    fn entry(&mut self, who: Guid) -> &mut Effects {
        let i = match self.entries.iter().position(|(g, _)| *g == who) {
            Some(i) => i,
            None => {
                self.entries.push((who, Effects::default()));
                self.entries.len() - 1
            }
        };
        &mut self.entries[i].1
    }

    pub fn get(&self, who: Guid) -> Effects {
        self.entries
            .iter()
            .find(|(g, _)| *g == who)
            .map(|(_, e)| *e)
            .unwrap_or_default()
    }

    /// Damage now and, for `turns` > 1, half of it per further turn as damage over time
    /// (negative `turns`: over time only).
    // bg3: esv::AiScoreInternal::AddDamage (0x1050855c0, 4.1.1.7631656)
    pub fn add_damage(&mut self, who: Guid, amount: f32, turns: i32) {
        let e = self.entry(who);
        if turns == 0 {
            e.damage += amount;
            return;
        }
        let over_time = if turns < 1 {
            turns as f32 * 0.5 - 0.5
        } else {
            e.damage += amount;
            if turns == 1 {
                return;
            }
            (turns - 1) as f32 * 0.5 + 0.5
        };
        e.dot += over_time * amount;
    }

    pub fn add_heal(&mut self, who: Guid, amount: f32) {
        self.entry(who).heal += amount;
    }

    // bg3: esv::AiScoreInternal::AddRaw (0x105085b34, 4.1.1.7631656)
    pub fn add_raw(&mut self, who: Guid, amount: f32) {
        self.entry(who).raw += amount;
    }

    /// Scale everything by a probability.
    // bg3: esv::AiScoreInternal::ApplyChance (0x105085768, 4.1.1.7631656)
    pub fn apply_chance(&mut self, chance: f32) {
        for (_, e) in &mut self.entries {
            *e = e.scaled(chance);
        }
    }

    /// Add another score scaled by `factor` (instakill chances combine as independent).
    // bg3: esv::AiScoreInternal::Add(esv::AiScore, float) (0x105083080, 4.1.1.7631656)
    pub fn add(&mut self, other: &AiScore, factor: f32) {
        for (who, o) in &other.entries {
            let e = self.entry(*who);
            let o = o.scaled(factor);
            e.damage += o.damage;
            e.heal += o.heal;
            e.dot += o.dot;
            e.hot += o.hot;
            e.control += o.control;
            e.boost += o.boost;
            e.raw += o.raw;
            e.instakill = o.instakill + e.instakill - o.instakill * e.instakill;
        }
    }
}

/// How the actor stands towards an affected entity (`eoc::relation::LEGACY_GetRelation`:
/// 0 hostile, 50 neutral, 100 allied; the actor itself is allied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Enemy,
    Neutral,
    Ally,
}

/// What the scoring needs to know about an affected entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetInfo {
    pub relation: Relation,
    pub is_self: bool,
    /// Hit points (temporary ones included).
    pub hp: i32,
    pub max_hp: i32,
    pub summon: bool,
    pub downed: bool,
    pub knocked_down: bool,
    pub incapacitated: bool,
    pub preferred: bool,
    pub unpreferred: bool,
    pub in_sight: bool,
    /// The target's current enemy is the actor.
    pub my_enemy: bool,
    /// The actor's current enemy is the target.
    pub my_hostile: bool,
    /// How many characters already fight the target.
    pub hostile_count: u32,
    /// Not in the actor's fight.
    pub out_of_combat: bool,
    /// For allies: distance to their closest enemy (buff falloff).
    pub closest_enemy: Option<f32>,
}

impl TargetInfo {
    pub fn new(relation: Relation, hp: i32, max_hp: i32) -> Self {
        Self {
            relation,
            is_self: false,
            hp,
            max_hp,
            summon: false,
            downed: false,
            knocked_down: false,
            incapacitated: false,
            preferred: false,
            unpreferred: false,
            in_sight: true,
            my_enemy: false,
            my_hostile: false,
            hostile_count: 0,
            out_of_combat: false,
            closest_enemy: None,
        }
    }
}

/// The actor's side of the scoring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorInfo {
    /// `esv::AiHelpers::GetAverageDamageForCharacter`: damage scores are in units of this.
    pub average_damage: f32,
    /// Self-damage is in units of the actor's maximum hit points instead.
    pub max_hp: i32,
}

/// One entity's part of a score, after the modifiers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TargetBreakdown {
    pub entity: Guid,
    pub kill: bool,
    /// Product of the target multipliers (preferred, downed, summon, …), sign and `SCORE_MOD`.
    pub target_multiplier: f32,
    pub damage: f32,
    pub heal: f32,
    pub dot: f32,
    pub hot: f32,
    pub control: f32,
    pub boost: f32,
    pub raw: f32,
    pub total: f32,
}

/// The summed score of an action.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreBreakdown {
    pub targets: Vec<TargetBreakdown>,
    pub total: f32,
    /// Killing oneself, or no affected entity: the engine rejects the action.
    pub invalid: bool,
}

/// Weight of a value by its sign: `<KIND>_<WHO>_POS` for >= 0, else `_NEG`.
fn signed(m: &Modifiers, kind: &str, who: &str, v: f32) -> f32 {
    let sign = if v >= 0.0 { "POS" } else { "NEG" };
    m.get(&format!("MULTIPLIER_{kind}_{who}_{sign}"))
}

/// Score of one entity's effects.
// bg3: esv::AiScoreInternal::CalculateScore (0x10508365c, 4.1.1.7631656), the per-entity
// loop. Written from the decompiled control flow; the status-based zeroing (the actor
// charmed or similar), the first-action buff bonus and the "best target" bookkeeping are
// not modelled.
pub fn score_target(
    m: &Modifiers,
    actor: &ActorInfo,
    who: Guid,
    t: &TargetInfo,
    e: &Effects,
) -> (TargetBreakdown, bool) {
    let mut invalid = false;
    let mut target_mult = 1.0f32;
    if !t.is_self {
        if t.preferred {
            target_mult = m.get(k::MULTIPLIER_TARGET_PREFERRED);
        }
        if t.unpreferred {
            target_mult *= m.get(k::MULTIPLIER_TARGET_UNPREFERRED);
        }
    }
    let (mut damage, heal, dot, hot) = (e.damage, e.heal, e.dot, e.hot);
    let (mut control, mut boost, raw) = (e.control, e.boost, e.raw);
    let hp = t.hp;
    let max_hp = t.max_hp.max(1);
    if t.relation == Relation::Ally && !t.is_self {
        // Buffs on allies far from the fight are worth less (BUFF_DIST_MIN..MAX: 1 → 0.1).
        let (lo, hi) = (m.get(k::BUFF_DIST_MIN), m.get(k::BUFF_DIST_MAX));
        if let Some(d) = t.closest_enemy
            && hi > lo
        {
            let d = d.clamp(lo, hi);
            boost *= ((d - lo) / (hi - lo)) * -0.9 + 1.0;
        }
    }
    let mut kill_mult = 1.0f32;
    let mut kill = false;
    if damage > 0.0 || e.instakill.abs() > 1.192_092_9e-7 {
        if t.relation == Relation::Enemy && damage > 0.0 {
            control *= (actor.average_damage / damage).min(1.0);
        }
        let dealt = damage as i32;
        if dealt < hp && e.instakill == 0.0 {
            if t.relation == Relation::Ally {
                damage /= (hp as f32 / max_hp as f32).max(0.1);
            }
        } else {
            kill = true;
            if t.is_self {
                invalid = true;
            }
            damage += m.get(k::MULTIPLIER_KILL_TARGET_HEALTH_BIAS) * hp as f32;
            if e.instakill != 0.0 {
                damage += m.get(k::INSTAKILL_BASE_SCORE)
                    + m.get(k::MULTIPLIER_INSTAKILL_TARGET_HEALTH_BIAS) * hp as f32;
            }
            kill_mult = match (t.relation, t.summon) {
                (Relation::Enemy, false) => m.get(k::MULTIPLIER_KILL_ENEMY),
                (Relation::Enemy, true) => m.get(k::MULTIPLIER_KILL_ENEMY_SUMMON),
                (Relation::Ally, false) => m.get(k::MULTIPLIER_KILL_ALLY),
                (Relation::Ally, true) => m.get(k::MULTIPLIER_KILL_ALLY_SUMMON),
                (Relation::Neutral, _) => 1.0,
            };
            kill_mult *= if hp <= dealt { 1.0 } else { e.instakill };
            boost = 0.0;
            control = 0.0;
        }
    }
    // Heals count negative (a heal on an enemy is bad); overheal and low targets adjust.
    let low_divisor = |ratio: f32| {
        let cap = if t.is_self {
            m.get(k::MAX_HEAL_SELF_MULTIPLIER)
        } else {
            m.get(k::MAX_HEAL_MULTIPLIER)
        };
        (ratio + ratio).max(1.0 - cap)
    };
    let mut heal_v = -heal;
    if heal > 0.0 {
        let missing = (max_hp - hp) as f32;
        if (max_hp as f32) < heal + hp as f32 {
            heal_v = (missing / heal) * -missing;
        }
        let ratio = hp as f32 / max_hp as f32;
        if ratio < 0.5 {
            heal_v /= low_divisor(ratio);
        }
    }
    let mut hot_v = -hot;
    if hot > 0.0 {
        let after = (hp + heal as i32 - e.damage as i32).max(0).min(max_hp);
        let mut v = (max_hp - after) as f32;
        if hot + after as f32 <= max_hp as f32 {
            v = hot;
        }
        hot_v = -v;
        let ratio = after as f32 / max_hp as f32;
        if ratio < 0.5 {
            hot_v /= low_divisor(ratio);
        }
    }
    let (mut damage, mut heal_v, mut dot, mut hot_v) = (damage, heal_v, dot, hot_v);
    let (mut control, mut boost, mut raw) = (control, boost, raw);
    if t.relation == Relation::Neutral {
        for v in [
            &mut damage,
            &mut heal_v,
            &mut dot,
            &mut hot_v,
            &mut control,
            &mut boost,
            &mut raw,
        ] {
            *v = v.max(0.0);
        }
    }
    if t.is_self {
        kill_mult = -kill_mult;
    } else {
        match t.relation {
            Relation::Ally => kill_mult *= m.get(k::MULTIPLIER_SCORE_ON_ALLY),
            Relation::Neutral => kill_mult *= m.get(k::MULTIPLIER_SCORE_ON_NEUTRAL),
            Relation::Enemy => {
                if t.my_enemy {
                    target_mult *= m.get(k::MULTIPLIER_TARGET_MY_ENEMY);
                }
                if t.my_hostile {
                    target_mult *= m.get(k::MULTIPLIER_TARGET_MY_HOSTILE);
                }
            }
        }
        if t.out_of_combat {
            target_mult *= m.get(k::MULTIPLIER_SCORE_OUT_OF_COMBAT);
        }
        if t.summon {
            target_mult *= m.get(k::MULTIPLIER_TARGET_SUMMON);
        }
        if t.downed {
            target_mult *= m.get(match t.relation {
                Relation::Enemy => k::MULTIPLIER_TARGET_ENEMY_DOWNED,
                Relation::Ally => k::MULTIPLIER_TARGET_ALLY_DOWNED,
                Relation::Neutral => k::MULTIPLIER_TARGET_NEUTRAL_DOWNED,
            });
        }
        match t.hostile_count {
            0 => {}
            1 => target_mult *= m.get(k::MULTIPLIER_TARGET_HOSTILE_COUNT_ONE),
            _ => target_mult *= m.get(k::MULTIPLIER_TARGET_HOSTILE_COUNT_TWO_OR_MORE),
        }
        // Applied when the sight check fails (as the binary does).
        if !t.in_sight {
            target_mult *= m.get(k::MULTIPLIER_TARGET_IN_SIGHT);
        }
        if e.control == 0.0 {
            if t.knocked_down {
                target_mult *= m.get(k::MULTIPLIER_TARGET_KNOCKED_DOWN);
            } else if t.incapacitated {
                target_mult *= m.get(k::MULTIPLIER_TARGET_INCAPACITATED);
            }
        }
    }
    kill_mult *= m.get(k::SCORE_MOD);
    target_mult *= kill_mult;
    let (damage, heal_v, dot, hot_v) = (
        damage * target_mult,
        heal_v * target_mult,
        dot * target_mult,
        hot_v * target_mult,
    );
    let (control, boost) = (control * target_mult, boost * target_mult);
    let who_s = if t.is_self {
        "SELF"
    } else {
        match t.relation {
            Relation::Enemy => "ENEMY",
            Relation::Ally => "ALLY",
            Relation::Neutral => "NEUTRAL",
        }
    };
    let normalized = |v: f32| {
        // bg3: esv::AiHelpers::GetNormalizedDamageScore (0x10524e9f8): value / the actor's
        // average damage (0 without one).
        if actor.average_damage == 0.0 {
            0.0
        } else {
            v / actor.average_damage
        }
    };
    let damage_s = if t.is_self {
        if actor.max_hp != 0 {
            damage * signed(m, "DAMAGE", who_s, damage) / actor.max_hp as f32
        } else {
            0.0
        }
    } else {
        normalized(damage * signed(m, "DAMAGE", who_s, damage))
    };
    let heal_s = normalized(heal_v * signed(m, "HEAL", who_s, heal_v));
    let dot_s = normalized(dot * signed(m, "DOT", who_s, dot));
    let hot_s = normalized(hot_v * signed(m, "HOT", who_s, hot_v));
    let control_s = control * signed(m, "CONTROL", who_s, control);
    let boost_s = boost * signed(m, "BOOST", who_s, boost);
    let raw_s = raw * kill_mult;
    let total = raw_s + boost_s + control_s + damage_s + heal_s + dot_s + hot_s;
    (
        TargetBreakdown {
            entity: who,
            kill,
            target_multiplier: target_mult,
            damage: damage_s,
            heal: heal_s,
            dot: dot_s,
            hot: hot_s,
            control: control_s,
            boost: boost_s,
            raw: raw_s,
            total,
        },
        invalid,
    )
}

/// Sum the per-entity scores of an action.
pub fn calculate(
    m: &Modifiers,
    actor: &ActorInfo,
    score: &AiScore,
    info: impl Fn(Guid) -> Option<TargetInfo>,
) -> ScoreBreakdown {
    let mut out = ScoreBreakdown::default();
    for (who, e) in &score.entries {
        let Some(t) = info(*who) else {
            continue;
        };
        let (b, invalid) = score_target(m, actor, *who, &t, e);
        out.invalid |= invalid;
        out.total += b.total;
        out.targets.push(b);
    }
    if out.targets.is_empty() {
        out.invalid = true;
    }
    out
}

/// Turn an action's summed score into its final value: `multiplier * score - cost`, where
/// the cost is the resource cost score times `MULTIPLIER_ACTION_RESOURCE_COST` (a free
/// action costs `MULTIPLIER_FREE_ACTION`), and the multiplier is
/// `MULTIPLIER_CANNOT_EXECUTE_THIS_TURN` for actions that cannot be done this turn.
// bg3: esv::CombatAIAction::CalculateScore (0x1051e1c2c, 4.1.1.7631656). Cooldown and item
// count factors are not modelled (no cooldowns or item uses in the AI yet).
pub fn final_score(m: &Modifiers, score: f32, resource_cost: f32, this_turn: bool) -> (f32, f32) {
    let mut cost = resource_cost * m.get(k::MULTIPLIER_ACTION_RESOURCE_COST);
    if cost == 0.0 {
        cost = m.get(k::MULTIPLIER_FREE_ACTION);
    }
    let mult = if this_turn {
        1.0
    } else {
        m.get(k::MULTIPLIER_CANNOT_EXECUTE_THIS_TURN)
    };
    (mult * score - cost, cost)
}

/// The success weight of a chance: the archetype's `MODIFIER_HIT_CHANCE_STUPIDITY` `s`
/// makes the AI over-estimate it, `s + (1 - s) * chance` (1 = always assumes success).
// bg3: esv::AiHelpers::CalculateAiScoreStatsFunctors (0x105223ba0) and
// CalculateAiScoreSpellStatsFunctors (0x105248600, 4.1.1.7631656)
pub fn believed_chance(m: &Modifiers, chance: f32) -> f32 {
    let s = m.get(k::MODIFIER_HIT_CHANCE_STUPIDITY);
    (s + (1.0 - s) * chance).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(n: u128) -> Guid {
        Guid(n)
    }

    fn mods() -> Modifiers {
        let mut m = Modifiers::neutral();
        m.set(k::SCORE_MOD, 100.0);
        m.set(k::MULTIPLIER_KILL_ENEMY, 1.25);
        m.set("MULTIPLIER_DAMAGE_ALLY_NEG", 4.0);
        m.set(k::MULTIPLIER_SCORE_ON_ALLY, -1.1);
        m.set(k::MULTIPLIER_ACTION_RESOURCE_COST, 0.01);
        m.set(k::MULTIPLIER_FREE_ACTION, 0.9);
        m.set(k::MULTIPLIER_CANNOT_EXECUTE_THIS_TURN, 0.2);
        m
    }

    const ACTOR: ActorInfo = ActorInfo {
        average_damage: 5.0,
        max_hp: 10,
    };

    fn one(t: TargetInfo, e: Effects) -> TargetBreakdown {
        score_target(&mods(), &ACTOR, g(1), &t, &e).0
    }

    #[test]
    fn damage_is_in_units_of_the_actors_average_damage() {
        let b = one(
            TargetInfo::new(Relation::Enemy, 20, 20),
            Effects {
                damage: 5.0,
                ..Default::default()
            },
        );
        assert!(!b.kill);
        assert!((b.total - 100.0).abs() < 1e-4, "{b:?}");
    }

    #[test]
    fn a_kill_applies_the_kill_multiplier() {
        let e = Effects {
            damage: 5.0,
            ..Default::default()
        };
        let b = one(TargetInfo::new(Relation::Enemy, 5, 20), e);
        assert!(b.kill);
        assert!((b.total - 125.0).abs() < 1e-4);
        // 4.9 expected damage truncates to 4 < 5 HP: no kill.
        let b = one(
            TargetInfo::new(Relation::Enemy, 5, 20),
            Effects {
                damage: 4.9,
                ..Default::default()
            },
        );
        assert!(!b.kill);
    }

    #[test]
    fn hurting_an_ally_is_negative_and_worse_when_hurt() {
        let e = Effects {
            damage: 5.0,
            ..Default::default()
        };
        let full = one(TargetInfo::new(Relation::Ally, 20, 20), e);
        // -1.1 * 100 * 5 * 4 (ALLY_NEG) / 5.
        assert!((full.total + 440.0).abs() < 1e-3, "{full:?}");
        let half = one(TargetInfo::new(Relation::Ally, 10, 20), e);
        assert!((half.total - 2.0 * full.total).abs() < 1e-3);
    }

    #[test]
    fn self_damage_is_in_units_of_own_max_hp_and_suicide_is_invalid() {
        let mut t = TargetInfo::new(Relation::Ally, 10, 10);
        t.is_self = true;
        let e = Effects {
            damage: 2.0,
            ..Default::default()
        };
        let (b, invalid) = score_target(&mods(), &ACTOR, g(1), &t, &e);
        assert!(!invalid);
        assert!((b.total + 20.0).abs() < 1e-4, "-100 * 2 / 10: {b:?}");
        let e = Effects {
            damage: 10.0,
            ..Default::default()
        };
        assert!(score_target(&mods(), &ACTOR, g(1), &t, &e).1);
    }

    #[test]
    fn raw_position_terms_on_self_flip_sign() {
        let mut t = TargetInfo::new(Relation::Ally, 10, 10);
        t.is_self = true;
        let b = one(
            t,
            Effects {
                raw: -0.02,
                ..Default::default()
            },
        );
        assert!((b.total - 2.0).abs() < 1e-4);
    }

    #[test]
    fn healing_counts_negative_on_enemies_and_positive_on_self() {
        let e = Effects {
            heal: 5.0,
            ..Default::default()
        };
        assert!(one(TargetInfo::new(Relation::Enemy, 5, 20), e).total < 0.0);
        let mut t = TargetInfo::new(Relation::Ally, 5, 20);
        t.is_self = true;
        assert!(one(t, e).total > 0.0);
    }

    #[test]
    fn add_damage_spreads_over_turns_and_chance_scales() {
        let mut s = AiScore::default();
        s.add_damage(g(1), 4.0, 3);
        assert_eq!(s.get(g(1)).damage, 4.0);
        assert_eq!(s.get(g(1)).dot, 6.0);
        s.apply_chance(0.5);
        assert_eq!(s.get(g(1)).damage, 2.0);
        let mut t = AiScore::default();
        t.add(&s, 2.0);
        assert_eq!(t.get(g(1)).dot, 6.0);
    }

    #[test]
    fn final_score_subtracts_costs_and_discounts_future_actions() {
        let m = mods();
        let (f, cost) = final_score(&m, 100.0, 1.0, true);
        assert!((cost - 0.01).abs() < 1e-6);
        assert!((f - 99.99).abs() < 1e-4);
        let (f, cost) = final_score(&m, 100.0, 0.0, false);
        assert_eq!(cost, 0.9);
        assert!((f - 19.1).abs() < 1e-4);
    }

    #[test]
    fn stupidity_overestimates_chances() {
        let mut m = mods();
        m.set(k::MODIFIER_HIT_CHANCE_STUPIDITY, 0.85);
        assert!((believed_chance(&m, 0.5) - 0.925).abs() < 1e-6);
        m.set(k::MODIFIER_HIT_CHANCE_STUPIDITY, 1.0);
        assert_eq!(believed_chance(&m, 0.0), 1.0);
    }
}
