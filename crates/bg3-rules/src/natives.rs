//! Engine natives for conditions: the subset of `ls::thoth::shared`, `…::condition` and
//! `…::server::condition` functions we implement over a [`RulesContext`].
//!
//! Natives that take an entity default to `context.Target` when the argument is omitted (the
//! helper scripts note "Default to Target, like cpp functions"; confirmed for `Dead`, which
//! reads the target from the top of the context stack). `Ally`/`Enemy`/`HasStatus` take a
//! second entity defaulting to `context.Source`. Unimplemented natives are a script error.

use std::rc::Rc;

use crate::condition::ConditionContext;
use crate::context::{Ability, Advantage, AttackType, EntityId, Property, RulesContext, Size};
use crate::dice::{DiceRng, DiceType};
use crate::rules::{self, AttackRoll, SavingThrowRoll};
use crate::thoth::interp::{Host, LResult, Machine, rt_err};
use crate::thoth::value::{ConditionError, ConditionResult, Key, Table, Value};

/// A roll made while evaluating a condition.
#[derive(Debug, Clone, PartialEq)]
pub enum RollEvent {
    Attack(AttackRoll),
    SavingThrow(SavingThrowRoll),
    /// `RollDieAgainstDC(die, dc)`.
    Die {
        die: DiceType,
        value: u32,
        dc: i32,
    },
}

/// Natives implemented here.
pub const NATIVES: &[&str] = &[
    "ConditionResult",
    "ConditionError",
    "Dead",
    "Character",
    "Item",
    "Player",
    "Party",
    "Combat",
    "IsDowned",
    "Ally",
    "Enemy",
    "Tagged",
    "HasStatus",
    "HasAnyStatus",
    "HasPassive",
    "Attack",
    "SavingThrow",
    "CalculateSpellDC",
    "RollDieAgainstDC",
];

/// Enum types exported to conditions (indexing them yields enum values; labels are not
/// validated).
pub const ENUM_TYPES: &[&str] = &[
    "Ability",
    "AttackType",
    "DamageType",
    "DamageFlags",
    "DiceType",
    "Size",
    "Skill",
    "SpellFlags",
    "SpellType",
    "SpellSchool",
    "StatusEvent",
    "StatsFunctorType",
    "HitWith",
    "WeaponProperties",
    "ObscuredState",
    "ResistanceType",
    "ActionType",
    "ProficiencyGroup",
    "StatusGroup",
    "SurfaceType",
    "ArmorType",
    "ItemSlot",
    "WeaponType",
    "SpellCategory",
    "RollCritical",
    "ConditionRollType",
    "AttackFlags",
    "CauseType",
    "SpellCooldownType",
    "StatusRemoveCause",
    "TagCategory",
    "InstrumentType",
    "EquipmentSlot",
    "SurfaceTransformActionType",
    "ExtraData",
];

pub fn register(m: &mut Machine) {
    for n in NATIVES {
        m.set_global(n, Value::Native((*n).into()));
    }
    for t in ENUM_TYPES {
        m.set_global(t, Value::EnumType((*t).into()));
    }
}

/// The [`Host`] that serves natives from a [`RulesContext`].
pub struct RulesHost<'a> {
    pub ctx: &'a mut dyn RulesContext,
    pub rng: &'a mut dyn DiceRng,
    pub data: ConditionContext,
    pub base_spell_dc: i32,
    pub rolls: Vec<crate::natives::RollEvent>,
}

fn cr(b: bool) -> LResult<Vec<Value>> {
    Ok(vec![Value::cond(ConditionResult::new(b))])
}

fn enum_label(v: &Value, ty: &str) -> Option<Rc<str>> {
    match v {
        Value::Enum(e) if &*e.ty == ty => Some(e.name.clone()),
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn truthy_cond(v: &Value) -> bool {
    match v {
        Value::Cond(c) => c.borrow().result,
        v => v.truthy(),
    }
}

fn errors_from(v: &Value) -> Vec<ConditionError> {
    let Value::Table(t) = v else {
        return Vec::new();
    };
    let t = t.borrow();
    (1..=t.len())
        .filter_map(|i| match t.get(&Key::Int(i)) {
            Value::Str(s) => Some(ConditionError {
                name: s,
                data: Vec::new(),
            }),
            Value::Table(e) => match e.borrow().get_str("ErrorName") {
                Value::Str(s) => Some(ConditionError {
                    name: s,
                    data: Vec::new(),
                }),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

impl<'a> RulesHost<'a> {
    pub fn new(
        ctx: &'a mut dyn RulesContext,
        rng: &'a mut dyn DiceRng,
        data: ConditionContext,
        base_spell_dc: i32,
    ) -> Self {
        Self {
            ctx,
            rng,
            data,
            base_spell_dc,
            rolls: Vec::new(),
        }
    }

    /// The `context` value passed to the chunk.
    pub fn context_value(&self) -> Value {
        let t = Table::new_ref();
        {
            let mut t = t.borrow_mut();
            t.set_str("Source", Value::Entity(self.data.source));
            t.set_str("Target", Value::Entity(self.data.target));
            t.set_str("Observer", Value::Entity(self.data.observer));
            if let Some(a) = self
                .data
                .source
                .and_then(|s| self.ctx.spellcasting_ability(s))
            {
                t.set_str(
                    "PreferredCastingAbility",
                    Value::Enum(Rc::new(crate::thoth::value::EnumValue {
                        ty: "Ability".into(),
                        name: a.name().into(),
                    })),
                );
            }
            if let Some(s) = &self.data.spell {
                t.set_str("SpellId", Value::str(s));
            }
            if let Some(s) = &self.data.status {
                t.set_str("StatusId", Value::str(s));
            }
        }
        Value::Table(t)
    }

    fn entity(
        &self,
        args: &[Value],
        i: usize,
        default: Option<EntityId>,
    ) -> LResult<Option<EntityId>> {
        match args.get(i) {
            None | Some(Value::Nil) => Ok(default),
            Some(Value::Entity(e)) => Ok(*e),
            Some(v) => rt_err(format!(
                "argument {} must be an entity, got {}",
                i + 1,
                v.type_name()
            )),
        }
    }

    fn target(&self, args: &[Value], i: usize) -> LResult<Option<EntityId>> {
        self.entity(args, i, self.data.target)
    }

    fn source(&self, args: &[Value], i: usize) -> LResult<Option<EntityId>> {
        self.entity(args, i, self.data.source)
    }

    fn string(args: &[Value], i: usize) -> LResult<Rc<str>> {
        match args.get(i) {
            Some(Value::Str(s)) => Ok(s.clone()),
            Some(Value::Enum(e)) => Ok(e.name.clone()),
            v => rt_err(format!("argument {} must be a string, got {:?}", i + 1, v)),
        }
    }

    /// Predicate on an optional entity; a null handle is false.
    fn pred(
        &self,
        e: Option<EntityId>,
        f: impl Fn(&dyn RulesContext, EntityId) -> bool,
    ) -> LResult<Vec<Value>> {
        cr(e.is_some_and(|e| self.ctx.exists(e) && f(&*self.ctx, e)))
    }
}

impl Host for RulesHost<'_> {
    fn call_native(&mut self, _m: &mut Machine, name: &str, args: &[Value]) -> LResult<Vec<Value>> {
        match name {
            "ConditionResult" => {
                let result = truthy_cond(args.first().unwrap_or(&Value::Nil));
                let mut c = ConditionResult::new(result);
                c.active_errors = errors_from(args.get(1).unwrap_or(&Value::Nil));
                c.inactive_errors = errors_from(args.get(2).unwrap_or(&Value::Nil));
                if let Some(ch) = args.get(3).and_then(|v| v.as_number()) {
                    c.chance = ch as f32;
                }
                Ok(vec![Value::cond(c)])
            }
            "ConditionError" => {
                let t = Table::new_ref();
                t.borrow_mut()
                    .set_str("ErrorName", Value::Str(Self::string(args, 0)?));
                Ok(vec![Value::Table(t)])
            }
            // bg3: ls::thoth::shared::Dead (0x102026e54)
            "Dead" => self.pred(self.target(args, 0)?, |c, e| c.is_dead(e)),
            "Character" => self.pred(self.target(args, 0)?, |c, e| c.is_character(e)),
            "Item" => self.pred(self.target(args, 0)?, |c, e| c.is_item(e)),
            "Player" | "Party" => self.pred(self.target(args, 0)?, |c, e| c.is_player(e)),
            "Combat" => self.pred(self.target(args, 0)?, |c, e| c.in_combat(e)),
            "IsDowned" => self.pred(self.target(args, 0)?, |c, e| c.is_downed(e)),
            "Ally" | "Enemy" => {
                let (t, s) = (self.target(args, 0)?, self.source(args, 1)?);
                let r = match (t, s) {
                    (Some(t), Some(s)) if name == "Ally" => self.ctx.is_ally(s, t),
                    (Some(t), Some(s)) => self.ctx.is_enemy(s, t),
                    _ => false,
                };
                cr(r)
            }
            "Tagged" => {
                let tag = Self::string(args, 0)?;
                self.pred(self.target(args, 1)?, |c, e| c.has_tag(e, &tag))
            }
            "HasStatus" => {
                let s = Self::string(args, 0)?;
                self.pred(self.target(args, 1)?, |c, e| c.has_status(e, &s))
            }
            "HasAnyStatus" => {
                let names: Vec<Rc<str>> = match args.first() {
                    Some(Value::Table(t)) => {
                        let t = t.borrow();
                        (1..=t.len())
                            .filter_map(|i| match t.get(&Key::Int(i)) {
                                Value::Str(s) => Some(s),
                                _ => None,
                            })
                            .collect()
                    }
                    _ => return rt_err("HasAnyStatus: expected a list of statuses"),
                };
                self.pred(self.target(args, 3)?, |c, e| {
                    names.iter().any(|n| c.has_status(e, n))
                })
            }
            "HasPassive" => {
                let p = Self::string(args, 0)?;
                self.pred(self.target(args, 1)?, |c, e| c.has_passive(e, &p))
            }
            // Real-mode Attack rolls the attack (source vs target).
            // bg3: ls::thoth::shared::condition::Attack (0x102036574) → eoc::stats::RollSpellCheck
            "Attack" => {
                let Some(label) = args.first().and_then(|v| enum_label(v, "AttackType")) else {
                    return rt_err("Attack: expected an AttackType");
                };
                let Some(ty) = AttackType::parse(&label) else {
                    return rt_err(format!("Attack: unknown attack type {label}"));
                };
                let (Some(s), Some(t)) = (self.data.source, self.data.target) else {
                    return rt_err("Attack: needs a source and a target");
                };
                let roll = rules::roll_attack(&*self.ctx, self.rng, s, t, ty);
                let hit = roll.hit;
                self.rolls.push(RollEvent::Attack(roll));
                cr(hit)
            }
            // bg3: ls::thoth::shared::condition::SavingThrow (0x102036a9c): rolls for the target.
            "SavingThrow" => {
                let Some(label) = args.first().and_then(|v| enum_label(v, "Ability")) else {
                    return rt_err("SavingThrow: expected an Ability");
                };
                let Some(ability) = Ability::parse(&label) else {
                    return rt_err(format!("SavingThrow: unknown ability {label}"));
                };
                let Some(dc) = args.get(1).and_then(|v| v.as_int()) else {
                    return rt_err("SavingThrow: expected a DC");
                };
                let Some(t) = self.data.target else {
                    return rt_err("SavingThrow: needs a target");
                };
                let adv = Advantage {
                    advantage: args.get(2).is_some_and(truthy_cond),
                    disadvantage: args.get(3).is_some_and(truthy_cond),
                };
                let roll =
                    rules::roll_saving_throw(&*self.ctx, self.rng, t, ability, dc as i32, adv);
                let ok = roll.success;
                self.rolls.push(RollEvent::SavingThrow(roll));
                cr(ok)
            }
            // bg3: ls::thoth::shared::CalculateSpellDC (0x10202c118)
            "CalculateSpellDC" => {
                let ability = args
                    .first()
                    .and_then(|v| enum_label(v, "Ability"))
                    .and_then(|l| Ability::parse(&l));
                let Some(e) = self.source(args, 1)? else {
                    return rt_err("CalculateSpellDC: no entity");
                };
                Ok(vec![Value::Int(
                    rules::spell_dc(&*self.ctx, e, ability, self.base_spell_dc) as i64,
                )])
            }
            // Not located in the binary; d20-style "roll >= DC".
            "RollDieAgainstDC" => {
                let die = args
                    .first()
                    .and_then(|v| enum_label(v, "DiceType"))
                    .and_then(|l| DiceType::from_label(&l))
                    .unwrap_or(DiceType::D20);
                let dc = args.get(1).and_then(|v| v.as_int()).unwrap_or(0) as i32;
                let value = self.rng.roll_die(die.faces());
                self.rolls.push(RollEvent::Die { die, value, dc });
                cr(value as i32 >= dc)
            }
            _ => rt_err(format!("native function '{name}' is not implemented")),
        }
    }

    fn index_entity(&mut self, e: Option<EntityId>, key: &str) -> LResult<Value> {
        let Some(e) = e else {
            return match key {
                "IsValid" => Ok(Value::Bool(false)),
                _ => rt_err(format!("null entity has no property '{key}'")),
            };
        };
        match self.ctx.property(e, key) {
            Some(Property::Int(i)) => Ok(Value::Int(i)),
            Some(Property::Float(f)) => Ok(Value::Float(f)),
            Some(Property::Bool(b)) => Ok(Value::Bool(b)),
            Some(Property::Str(s)) => Ok(Value::str(&s)),
            Some(Property::Entity(e)) => Ok(Value::Entity(e)),
            Some(Property::Enum(ty, name)) => {
                Ok(Value::Enum(Rc::new(crate::thoth::value::EnumValue {
                    ty: ty.into(),
                    name: name.into(),
                })))
            }
            None => rt_err(format!("entity property '{key}' is not implemented")),
        }
    }

    fn enum_ordinal(&self, ty: &str, name: &str) -> Option<i64> {
        match ty {
            "Size" if name == "None" => Some(-1),
            "Size" => Size::parse(name).map(|s| s as i64),
            "Ability" => Ability::parse(name).map(|a| a as i64 + 1),
            _ => None,
        }
    }
}
