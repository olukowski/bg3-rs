//! The game-state interface the rules engine evaluates against.
//!
//! [`RulesContext`] is deliberately small and query-oriented so the simulation (`bg3-sim`) can
//! implement it on top of its own entity storage. [`crate::testworld::TestWorld`] is a simple
//! in-memory implementation for tests and the CLI demo.

use std::fmt;

/// An entity handle (character, item, …), opaque to the rules engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u32);

macro_rules! named_enum {
    ($(#[$m:meta])* $name:ident { $($v:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($v),* }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),*];
            pub fn name(self) -> &'static str {
                match self { $($name::$v => stringify!($v)),* }
            }
            pub fn parse(s: &str) -> Option<Self> {
                match s { $(stringify!($v) => Some($name::$v),)* _ => None }
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.name()) }
        }
    };
}

named_enum!(
    /// The six abilities (`Ability` value list in `ValueLists.txt`, without `None`).
    Ability { Strength, Dexterity, Constitution, Intelligence, Wisdom, Charisma }
);

named_enum!(
    /// Damage types (`Damage Type` value list, without `None`).
    DamageType {
        Slashing, Piercing, Bludgeoning, Acid, Thunder, Necrotic, Fire, Lightning, Cold,
        Psychic, Poison, Radiant, Force,
    }
);

named_enum!(
    /// `AttackType` as used by `Attack(AttackType.X)` in roll conditions.
    AttackType {
        MeleeWeaponAttack, RangedWeaponAttack, MeleeOffHandWeaponAttack,
        RangedOffHandWeaponAttack, MeleeSpellAttack, RangedSpellAttack, MeleeUnarmedAttack,
        RangedUnarmedAttack,
    }
);

named_enum!(
    /// Creature size (`Size` in conditions; ordered smallest first).
    Size { Tiny, Small, Medium, Large, Huge, Gargantuan }
);

impl AttackType {
    pub fn is_spell(self) -> bool {
        matches!(
            self,
            AttackType::MeleeSpellAttack | AttackType::RangedSpellAttack
        )
    }

    pub fn is_ranged(self) -> bool {
        matches!(
            self,
            AttackType::RangedWeaponAttack
                | AttackType::RangedOffHandWeaponAttack
                | AttackType::RangedSpellAttack
                | AttackType::RangedUnarmedAttack
        )
    }

    pub fn is_offhand(self) -> bool {
        matches!(
            self,
            AttackType::MeleeOffHandWeaponAttack | AttackType::RangedOffHandWeaponAttack
        )
    }

    pub fn is_unarmed(self) -> bool {
        matches!(
            self,
            AttackType::MeleeUnarmedAttack | AttackType::RangedUnarmedAttack
        )
    }
}

/// How an entity takes a damage type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Resistance {
    #[default]
    Normal,
    Vulnerable,
    Resistant,
    Immune,
}

/// Which equipped weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hand {
    Main,
    Off,
}

/// The parts of a weapon the rules need.
#[derive(Debug, Clone, PartialEq)]
pub struct Weapon {
    pub name: String,
    /// Damage dice, e.g. `1d8` (stats `Damage` field).
    pub damage: crate::dice::RollDefinition,
    pub damage_type: DamageType,
    pub ranged: bool,
    /// `Finesse` weapon property: may use Dexterity for melee attacks.
    pub finesse: bool,
    /// Whether the wielder is proficient with it.
    pub proficient: bool,
}

/// Advantage state of a d20 roll. Advantage and disadvantage together cancel, regardless of
/// how many sources each has.
// bg3: eoc::Roll advantage/disadvantage flags, consumed by
// eoc::RollManager::InternalRollDiceWithBoosts (0x101c62184, 4.1.1.7631656)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Advantage {
    pub advantage: bool,
    pub disadvantage: bool,
}

/// Read and write access to game state for rules evaluation.
///
/// Only the core queries are required; everything else has a neutral default so a minimal
/// implementation stays small. Property names used by Thoth scripts (`entity.HP`,
/// `entity.Strength`, …) are served by [`RulesContext::property`], which defaults to the typed
/// queries.
pub trait RulesContext {
    // ---- identity and state -------------------------------------------------------------
    fn exists(&self, e: EntityId) -> bool;
    fn name(&self, e: EntityId) -> String;
    fn is_character(&self, e: EntityId) -> bool {
        self.exists(e)
    }
    fn is_item(&self, e: EntityId) -> bool {
        let _ = e;
        false
    }
    fn is_player(&self, e: EntityId) -> bool {
        let _ = e;
        false
    }
    fn is_dead(&self, e: EntityId) -> bool {
        self.hp(e) <= 0
    }
    fn is_downed(&self, e: EntityId) -> bool {
        let _ = e;
        false
    }
    fn in_combat(&self, e: EntityId) -> bool {
        let _ = e;
        false
    }
    fn size(&self, e: EntityId) -> Size {
        let _ = e;
        Size::Medium
    }

    // ---- abilities and proficiency ------------------------------------------------------
    fn ability_score(&self, e: EntityId, a: Ability) -> i32;
    /// `floor((score - 10) / 2)`.
    // bg3: floor((score - 10) * 0.5) in ls::thoth::shared::CalculateSpellDC (0x10202c118)
    fn ability_modifier(&self, e: EntityId, a: Ability) -> i32 {
        (self.ability_score(e, a) - 10).div_euclid(2)
    }
    fn level(&self, e: EntityId) -> i32;
    /// Proficiency bonus. The engine stores it precomputed per entity (from the
    /// `ProficiencyBonusScaling` level map), so it is a query here, not a formula.
    fn proficiency_bonus(&self, e: EntityId) -> i32;
    fn saving_throw_proficient(&self, e: EntityId, a: Ability) -> bool {
        let _ = (e, a);
        false
    }
    fn spellcasting_ability(&self, e: EntityId) -> Option<Ability> {
        let _ = e;
        None
    }
    /// Flat bonus to the spell save DC from boosts (`SpellSaveDC(n)`).
    fn spell_dc_bonus(&self, e: EntityId) -> i32 {
        let _ = e;
        0
    }

    // ---- combat stats -------------------------------------------------------------------
    fn armor_class(&self, e: EntityId) -> i32;
    fn hp(&self, e: EntityId) -> i32;
    fn max_hp(&self, e: EntityId) -> i32;
    fn weapon(&self, e: EntityId, hand: Hand) -> Option<Weapon> {
        let _ = (e, hand);
        None
    }
    fn resistance(&self, e: EntityId, t: DamageType) -> Resistance {
        let _ = (e, t);
        Resistance::Normal
    }
    /// Flat attack-roll bonus from boosts (`RollBonus(Attack, …)`, enchantments, …).
    fn attack_bonus(&self, attacker: EntityId, attack: AttackType) -> i32 {
        let _ = (attacker, attack);
        0
    }
    /// Advantage on an attack roll by `attacker` against `target`.
    fn attack_advantage(
        &self,
        attacker: EntityId,
        target: EntityId,
        attack: AttackType,
    ) -> Advantage {
        let _ = (attacker, target, attack);
        Advantage::default()
    }
    /// Advantage on a saving throw.
    fn saving_throw_advantage(&self, e: EntityId, a: Ability) -> Advantage {
        let _ = (e, a);
        Advantage::default()
    }
    /// How much `ReduceCriticalAttackThreshold` boosts lower the natural roll needed for a
    /// critical hit (0 = only a natural 20).
    fn critical_threshold_reduction(&self, attacker: EntityId) -> i32 {
        let _ = attacker;
        0
    }
    /// `LevelMapValue(name)` for an entity, if the context knows the level map.
    fn level_map_value(&self, e: EntityId, name: &str) -> Option<crate::expr::Number> {
        let _ = (e, name);
        None
    }

    // ---- relations, tags, statuses, passives --------------------------------------------
    fn is_ally(&self, a: EntityId, b: EntityId) -> bool {
        a == b
    }
    fn is_enemy(&self, a: EntityId, b: EntityId) -> bool {
        let _ = (a, b);
        false
    }
    fn has_tag(&self, e: EntityId, tag: &str) -> bool {
        let _ = (e, tag);
        false
    }
    fn has_status(&self, e: EntityId, status: &str) -> bool {
        let _ = (e, status);
        false
    }
    fn has_passive(&self, e: EntityId, passive: &str) -> bool {
        let _ = (e, passive);
        false
    }

    /// Generic property lookup for scripts (`entity.Name`). Return `None` for unknown names.
    fn property(&self, e: EntityId, name: &str) -> Option<Property> {
        default_property(self, e, name)
    }

    // ---- mutation -----------------------------------------------------------------------
    fn set_hp(&mut self, e: EntityId, hp: i32);
    fn apply_status(
        &mut self,
        target: EntityId,
        status: &str,
        duration: i32,
        source: Option<EntityId>,
    ) {
        let _ = (target, status, duration, source);
    }
    fn remove_status(&mut self, target: EntityId, status: &str) {
        let _ = (target, status);
    }
}

/// A scalar property value served to scripts.
#[derive(Debug, Clone, PartialEq)]
pub enum Property {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Entity(Option<EntityId>),
    /// An enum value: (enum type, label).
    Enum(&'static str, &'static str),
}

/// The default [`RulesContext::property`]: the names the shipped condition helpers read.
pub fn default_property<C: RulesContext + ?Sized>(
    c: &C,
    e: EntityId,
    name: &str,
) -> Option<Property> {
    if let Some(a) = Ability::parse(name) {
        return Some(Property::Int(c.ability_score(e, a) as i64));
    }
    Some(match name {
        "IsValid" => Property::Bool(c.exists(e)),
        "HP" => Property::Int(c.hp(e) as i64),
        "MaxHP" => Property::Int(c.max_hp(e) as i64),
        "Level" => Property::Int(c.level(e) as i64),
        "ProficiencyBonus" => Property::Int(c.proficiency_bonus(e) as i64),
        "AC" | "ArmorClass" => Property::Int(c.armor_class(e) as i64),
        "Size" => Property::Enum("Size", c.size(e).name()),
        "Name" => Property::Str(c.name(e)),
        _ => return None,
    })
}
