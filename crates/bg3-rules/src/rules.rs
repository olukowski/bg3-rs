//! D&D 5e roll mechanics as BG3 implements them: attack rolls, saving throws, spell save DC,
//! damage and resistances. Semantics and their sources are listed in
//! `docs/formats/stats-expressions.md` ("Rolls").

use std::fmt;

use crate::context::{
    Ability, Advantage, AttackType, DamageType, EntityId, Resistance, RulesContext, Skill,
};
use crate::dice::{D20, DiceRng, RollDefinition, roll_d20};

/// `BaseSpellDifficultyCheck` in `Data.txt` (8 in the reference build); callers that have the
/// stats loaded should pass the real key.
pub const DEFAULT_BASE_SPELL_DC: i32 = 8;

/// Damage values are clamped to this (sqrt of i32::MAX), as in `ApplyResistance`.
pub const DAMAGE_CLAMP: i32 = 46340;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Critical {
    None,
    Success,
    Fail,
}

/// A labelled term of a roll total (for logs and tooltips).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub label: String,
    pub value: i32,
}

fn term(label: impl Into<String>, value: i32) -> Term {
    Term {
        label: label.into(),
        value,
    }
}

fn fmt_terms(f: &mut fmt::Formatter<'_>, d20: &D20, terms: &[Term]) -> fmt::Result {
    write!(f, "d20={}", d20.natural)?;
    if let Some(x) = d20.discarded {
        write!(f, " (other {x})")?;
    }
    for t in terms {
        if t.value >= 0 {
            write!(f, " +{} {}", t.value, t.label)?;
        } else {
            write!(f, " -{} {}", -t.value, t.label)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttackRoll {
    pub attacker: EntityId,
    pub target: EntityId,
    pub attack_type: AttackType,
    pub advantage: Advantage,
    pub d20: D20,
    pub terms: Vec<Term>,
    pub total: i32,
    pub armor_class: i32,
    pub critical: Critical,
    pub hit: bool,
}

impl fmt::Display for AttackRoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} roll: ", self.attack_type)?;
        fmt_terms(f, &self.d20, &self.terms)?;
        write!(f, " = {} vs AC {}", self.total, self.armor_class)?;
        match self.critical {
            Critical::Success => write!(f, " CRITICAL HIT")?,
            Critical::Fail => write!(f, " critical miss")?,
            Critical::None => {}
        }
        write!(f, " -> {}", if self.hit { "hit" } else { "miss" })
    }
}

/// The ability an attack uses, and the weapon (if any).
pub fn attack_ability(
    ctx: &dyn RulesContext,
    attacker: EntityId,
    attack: AttackType,
) -> (Ability, Option<crate::context::Weapon>) {
    if attack.is_spell() {
        let a = ctx
            .spellcasting_ability(attacker)
            .unwrap_or(Ability::Intelligence);
        return (a, None);
    }
    let weapon = if attack.is_unarmed() {
        None
    } else {
        ctx.attack_weapon(attacker, attack)
    };
    let str_mod = ctx.ability_modifier(attacker, Ability::Strength);
    let dex_mod = ctx.ability_modifier(attacker, Ability::Dexterity);
    let ability = match &weapon {
        Some(w) if w.ranged => Ability::Dexterity,
        Some(w) if w.finesse && dex_mod > str_mod => Ability::Dexterity,
        Some(_) => Ability::Strength,
        None if attack.is_unarmed() || !attack.is_ranged() => ctx.unarmed_attack_ability(attacker),
        None => Ability::Strength,
    };
    (ability, weapon)
}

/// Roll an attack: d20 (with advantage/disadvantage) + ability modifier + proficiency +
/// boosts, against the target's AC.
///
/// A natural 1 is a critical miss and a natural `20 - reduction` or higher a critical hit;
/// both decide the outcome regardless of the total. Otherwise the attack hits when
/// `total >= AC`.
// bg3: eoc::stats::GetAttackRollResults (0x101fc88e4) and the critical marking in
// eoc::RollManager (4.1.1.7631656). The "success/failure never" critical boosts
// (CriticalHit(AttackTarget, …, Never)) are not modelled yet.
pub fn roll_attack(
    ctx: &dyn RulesContext,
    rng: &mut dyn DiceRng,
    attacker: EntityId,
    target: EntityId,
    attack: AttackType,
) -> AttackRoll {
    let advantage = ctx.attack_advantage(attacker, target, attack);
    let d20 = roll_d20(rng, advantage);
    let (ability, weapon) = attack_ability(ctx, attacker, attack);
    let mut terms = vec![term(
        ability.name(),
        ctx.ability_modifier(attacker, ability),
    )];
    let proficient = attack.is_spell() || weapon.as_ref().is_none_or(|w| w.proficient);
    if proficient {
        terms.push(term("proficiency", ctx.proficiency_bonus(attacker)));
    }
    let bonus = ctx.attack_bonus(attacker, attack);
    if bonus != 0 {
        terms.push(term("bonus", bonus));
    }
    let total = d20.natural as i32 + terms.iter().map(|t| t.value).sum::<i32>();
    let armor_class = ctx.armor_class(target);
    let crit_at = 20 - ctx.critical_threshold_reduction(attacker);
    let critical = if d20.natural == 1 {
        Critical::Fail
    } else if d20.natural as i32 >= crit_at {
        Critical::Success
    } else {
        Critical::None
    };
    let hit = match critical {
        Critical::Success => true,
        Critical::Fail => false,
        Critical::None => total >= armor_class,
    };
    AttackRoll {
        attacker,
        target,
        attack_type: attack,
        advantage,
        d20,
        terms,
        total,
        armor_class,
        critical,
        hit,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavingThrowRoll {
    pub entity: EntityId,
    pub ability: Ability,
    pub advantage: Advantage,
    pub d20: D20,
    pub terms: Vec<Term>,
    pub total: i32,
    pub dc: i32,
    pub success: bool,
}

impl fmt::Display for SavingThrowRoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} saving throw: ", self.ability)?;
        fmt_terms(f, &self.d20, &self.terms)?;
        write!(
            f,
            " = {} vs DC {} -> {}",
            self.total,
            self.dc,
            if self.success { "success" } else { "failure" }
        )
    }
}

/// Roll a saving throw: d20 + ability modifier + proficiency (if proficient in that save)
/// against `dc`; succeeds when `total >= dc`. Natural 1s and 20s have no special effect.
// bg3: ls::thoth::shared::condition::SavingThrow (0x102036a9c) →
// eoc::RollManager::RollDiceWithBoostsAgainstDC (4.1.1.7631656)
pub fn roll_saving_throw(
    ctx: &dyn RulesContext,
    rng: &mut dyn DiceRng,
    entity: EntityId,
    ability: Ability,
    dc: i32,
    extra: Advantage,
) -> SavingThrowRoll {
    let base = ctx.saving_throw_advantage(entity, ability);
    let advantage = Advantage {
        advantage: base.advantage || extra.advantage,
        disadvantage: base.disadvantage || extra.disadvantage,
    };
    let d20 = roll_d20(rng, advantage);
    let mut terms = vec![term(ability.name(), ctx.ability_modifier(entity, ability))];
    if ctx.saving_throw_proficient(entity, ability) {
        terms.push(term("proficiency", ctx.proficiency_bonus(entity)));
    }
    let total = d20.natural as i32 + terms.iter().map(|t| t.value).sum::<i32>();
    SavingThrowRoll {
        entity,
        ability,
        advantage,
        d20,
        terms,
        total,
        dc,
        success: total >= dc,
    }
}

/// What an ability check tests (a dialog roll's `RollType` / `Skill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// `SkillCheck`: proficiency (and expertise) in the skill count.
    Skill(Skill),
    /// `RawAbility`: the ability modifier alone.
    RawAbility,
    /// `SavingThrow`: saving-throw proficiency in the ability counts.
    SavingThrow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbilityCheckRoll {
    pub entity: EntityId,
    pub ability: Ability,
    pub kind: CheckKind,
    pub advantage: Advantage,
    pub d20: D20,
    pub terms: Vec<Term>,
    pub total: i32,
    pub dc: i32,
    pub critical: Critical,
    pub success: bool,
}

impl fmt::Display for AbilityCheckRoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CheckKind::Skill(s) => write!(f, "{s} ({}) check: ", self.ability)?,
            CheckKind::RawAbility => write!(f, "{} check: ", self.ability)?,
            CheckKind::SavingThrow => write!(f, "{} saving throw: ", self.ability)?,
        }
        fmt_terms(f, &self.d20, &self.terms)?;
        write!(f, " = {} vs DC {}", self.total, self.dc)?;
        match self.critical {
            Critical::Success => write!(f, " natural 20")?,
            Critical::Fail => write!(f, " natural 1")?,
            Critical::None => {}
        }
        write!(
            f,
            " -> {}",
            if self.success { "success" } else { "failure" }
        )
    }
}

/// Roll an ability check against `dc` (dialog rolls, `RollType` `SkillCheck`, `RawAbility` or
/// `SavingThrow`): d20 (with advantage/disadvantage) + ability modifier + proficiency bonus
/// when proficient (twice with expertise) + flat skill bonuses; succeeds when `total >= dc`.
///
/// A natural 20 always succeeds and a natural 1 always fails, reported as a critical
/// success / failure. That is how the game presents dialog checks; the branch itself was not
/// located in the binary (*inferred*), unlike the modifier sources, which follow
/// `eoc::active_roll::DetermineStaticModifiers` → `CheckProficiencyBoost` /
/// `CheckExpertiseBoost` / `CheckSkillBoost` (4.1.1.7631656). Boosts beyond these (roll
/// bonuses, advantage boosts, Guidance, …) are not modelled.
pub fn roll_ability_check(
    ctx: &dyn RulesContext,
    rng: &mut dyn DiceRng,
    entity: EntityId,
    ability: Ability,
    kind: CheckKind,
    dc: i32,
    advantage: Advantage,
) -> AbilityCheckRoll {
    let d20 = roll_d20(rng, advantage);
    let mut terms = vec![term(ability.name(), ctx.ability_modifier(entity, ability))];
    let pb = ctx.proficiency_bonus(entity);
    match kind {
        CheckKind::Skill(s) => {
            if ctx.skill_expertise(entity, s) {
                terms.push(term("expertise", 2 * pb));
            } else if ctx.skill_proficient(entity, s) {
                terms.push(term("proficiency", pb));
            }
            let b = ctx.skill_bonus(entity, s);
            if b != 0 {
                terms.push(term(s.name(), b));
            }
        }
        CheckKind::SavingThrow => {
            if ctx.saving_throw_proficient(entity, ability) {
                terms.push(term("proficiency", pb));
            }
        }
        CheckKind::RawAbility => {}
    }
    let total = d20.natural as i32 + terms.iter().map(|t| t.value).sum::<i32>();
    let critical = match d20.natural {
        20 => Critical::Success,
        1 => Critical::Fail,
        _ => Critical::None,
    };
    let success = match critical {
        Critical::Success => true,
        Critical::Fail => false,
        Critical::None => total >= dc,
    };
    AbilityCheckRoll {
        entity,
        ability,
        kind,
        advantage,
        d20,
        terms,
        total,
        dc,
        critical,
        success,
    }
}

/// Spell save DC: `base + proficiency + DC boosts + ability modifier`. Without an ability the
/// caster's spellcasting ability is used; with none at all the modifier is -5, as for an
/// entity without stats.
// bg3: ls::thoth::shared::CalculateSpellDC (0x10202c118, 4.1.1.7631656)
pub fn spell_dc(
    ctx: &dyn RulesContext,
    caster: EntityId,
    ability: Option<Ability>,
    base: i32,
) -> i32 {
    let modifier = match ability.or_else(|| ctx.spellcasting_ability(caster)) {
        Some(a) => ctx.ability_modifier(caster, a),
        None => -5,
    };
    base + ctx.proficiency_bonus(caster) + ctx.spell_dc_bonus(caster) + modifier
}

/// Apply resistance to one damage amount: immune → 0, resistant → halved (rounded down),
/// vulnerable → doubled; clamped to `0..=46340`.
// bg3: eoc::stats::ApplyResistance (0x101fe0b5c, 4.1.1.7631656): halving is
// `d - round(d / 2)`. The vulnerability branch was not located; doubling is the D&D rule.
pub fn apply_resistance(amount: i32, r: Resistance) -> i32 {
    let d = amount.clamp(0, DAMAGE_CLAMP);
    let d = match r {
        Resistance::Normal => d,
        Resistance::Resistant => d - (d as f64 / 2.0).round() as i32,
        Resistance::Vulnerable => d.saturating_mul(2),
        Resistance::Immune => 0,
    };
    d.clamp(0, DAMAGE_CLAMP)
}

/// A weapon's damage roll for `Placeholder0` in `DealDamage`: weapon dice plus the attack
/// ability modifier. Off-hand attacks only add a negative modifier (the 5e two-weapon rule;
/// not confirmed in the binary, `eoc::stats::GetWeaponDamage` 0x101fbf724 is only partly read).
pub fn weapon_damage_roll(
    ctx: &dyn RulesContext,
    attacker: EntityId,
    keyword: &str,
) -> Option<(RollDefinition, DamageType)> {
    let offhand = keyword.starts_with("Offhand");
    let attack = match (offhand, keyword.contains("Ranged")) {
        (false, false) => AttackType::MeleeWeaponAttack,
        (false, true) => AttackType::RangedWeaponAttack,
        (true, false) => AttackType::MeleeOffHandWeaponAttack,
        (true, true) => AttackType::RangedOffHandWeaponAttack,
    };
    let (ability, weapon) = attack_ability(ctx, attacker, attack);
    let modifier = ctx.ability_modifier(attacker, ability);
    let modifier = if offhand { modifier.min(0) } else { modifier };
    match weapon {
        Some(w) => Some((
            RollDefinition {
                bonus: w.damage.bonus + modifier,
                ..w.damage
            },
            w.damage_type,
        )),
        // Unarmed: 1 + Strength modifier, bludgeoning (5e). Not confirmed in the binary.
        None if !offhand => Some((RollDefinition::flat(1 + modifier), DamageType::Bludgeoning)),
        None => None,
    }
}
