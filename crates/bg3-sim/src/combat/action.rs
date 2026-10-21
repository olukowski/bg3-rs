//! What a combatant does on its turn: move, cast a spell (weapon attacks are the spells
//! `Target_MainHandAttack`, `Projectile_MainHandAttack`, …), and what follows from it:
//! damage, `AttackedBy`, downed party members, deaths.

use bg3_rules::condition::ConditionEngine;
use bg3_rules::context::{EntityId, Hand, RulesContext};
use bg3_rules::dice::DiceRng;
use bg3_rules::exec::{CastOutcome, ExecEvent, Executor};
use bg3_rules::natives::RollEvent;

use super::{CombatEvent, distance};
use crate::events::SimValue;
use crate::guid::Guid;
use crate::sim::Sim;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CombatError {
    #[error("not in combat")]
    NotInCombat,
    #[error("not this character's turn")]
    NotYourTurn,
    #[error("unknown character or no combat stats")]
    NoCombatant,
    #[error("the character does not know {0}")]
    UnknownSpell(String),
    #[error("not enough {0}")]
    NotEnoughResources(String),
    #[error("target out of range ({distance:.1} m > {range:.1} m)")]
    OutOfRange { distance: f32, range: f32 },
    #[error("spell {0}: {1}")]
    BadSpell(String, String),
    #[error("the character can't act (downed or dead)")]
    CannotAct,
}

/// The result of [`Sim::combat_cast`].
#[derive(Debug, Clone, PartialEq)]
pub struct CastReport {
    pub outcome: CastOutcome,
    /// The rules engine's text log of the cast.
    pub log: Vec<String>,
    pub events: Vec<ExecEvent>,
}

impl Sim {
    /// The range of a spell for a caster (metres): a number, or the weapon range keywords
    /// (`MeleeMainWeaponRange`: the melee weapon's `WeaponRange`, 1.5 m unarmed;
    /// `RangedMainWeaponRange`: the ranged weapon's). `None` if unknown.
    pub fn spell_range(&self, who: Guid, spell: &str) -> Option<f32> {
        let s = self.data.combat.spells.get(spell)?;
        let r = s.target_radius.as_str();
        if let Ok(v) = r.parse::<f32>() {
            return Some(v);
        }
        let c = self.combatant(who)?;
        let d = &self.data.combat;
        let weapon_range = |n: Option<&String>| n.and_then(|n| d.weapons.get(n)).map(|w| w.range);
        match r {
            "MeleeMainWeaponRange" => {
                Some(weapon_range(c.equipment.melee_main.as_ref()).unwrap_or(1.5))
            }
            "RangedMainWeaponRange" => weapon_range(c.equipment.ranged_main.as_ref()),
            "MeleeOffhandWeaponRange" => weapon_range(c.equipment.melee_off.as_ref()),
            _ => None,
        }
    }

    fn check_can_act(&self, who: Guid) -> Result<(), CombatError> {
        let c = self.combatant(who).ok_or(CombatError::NoCombatant)?;
        if c.dead || c.downed {
            return Err(CombatError::CannotAct);
        }
        if let Some(cid) = c.combat {
            let combat = self.combat_by_id(cid).ok_or(CombatError::NotInCombat)?;
            if !combat.finished && !combat.is_active(who) {
                return Err(CombatError::NotYourTurn);
            }
        }
        Ok(())
    }

    /// Move `who` by `distance` metres (the caller's path length), optionally to `to`. In
    /// combat, it must be `who`'s turn and the distance is paid from the `Movement` resource.
    // *Approximate*: no path finding or difficult terrain here (the caller measures the path,
    // e.g. with bg3-nav); `Movement` is spent 1:1 per metre.
    pub fn combat_move(
        &mut self,
        who: Guid,
        distance: f32,
        to: Option<[f32; 3]>,
    ) -> Result<(), CombatError> {
        if !self.ensure_combatant(who) {
            return Err(CombatError::NoCombatant);
        }
        self.check_can_act(who)?;
        let in_combat = self.in_combat(who);
        let left = {
            let c = self.combatant_mut(who).ok_or(CombatError::NoCombatant)?;
            if in_combat {
                let cost = vec![("Movement".to_owned(), distance.max(0.0))];
                if !c.resources.pay(&cost) {
                    return Err(CombatError::NotEnoughResources("Movement".into()));
                }
            }
            c.resources.get("Movement")
        };
        if let (Some(p), Some(e)) = (to, self.entities.get_mut(who)) {
            e.position = p;
        }
        self.combat_event(CombatEvent::Moved {
            entity: who,
            distance,
            movement_left: left,
        });
        Ok(())
    }

    /// A condition engine with the install's helper scripts loaded.
    fn condition_engine(&mut self) -> ConditionEngine {
        let mut engine = ConditionEngine::new();
        engine.base_spell_dc = self.data.combat.base_spell_dc;
        let data = self.data.clone();
        let mut dice = std::mem::take(&mut self.dice);
        for (path, src) in &data.combat.helper_scripts {
            if let Err(e) = engine.load_helpers(path, src, self, &mut dice) {
                self.combat_event(CombatEvent::Log(format!("helper script {path}: {e}")));
            }
        }
        self.dice = dice;
        engine
    }

    /// Cast `spell` from `who` at `target`: checks the turn, that the spell is known, the
    /// range and the `UseCosts`, pays them, runs the cast through the rules engine and
    /// applies what follows (story events, downed / dead targets, end of combat).
    ///
    /// Out of combat, a harmful cast at a hostile target starts a fight after it resolves
    /// (*approximate*: the engine's surprise and pre-combat rules are not modelled).
    ///
    /// Story events (*order and argument values unverified*): `UsingSpell(caster, spell,
    /// type, element, 0)`, `UsingSpellOnTarget(caster, target, spell, type, element, 0)`,
    /// then per damage `AttackedBy(target, attacker owner, attacker, damage type, amount,
    /// "Attack", 0)`, then `CastedSpell(caster, spell, type, element, 0)`.
    pub fn combat_cast(
        &mut self,
        who: Guid,
        spell: &str,
        target: Guid,
    ) -> Result<CastReport, CombatError> {
        if !self.ensure_combatant(who) {
            return Err(CombatError::NoCombatant);
        }
        self.ensure_combatant(target);
        self.check_can_act(who)?;
        let def = self
            .data
            .combat
            .spells
            .get(spell)
            .cloned()
            .ok_or_else(|| CombatError::UnknownSpell(spell.to_owned()))?;
        if !self
            .combatant(who)
            .is_some_and(|c| c.spells.iter().any(|s| s == spell))
        {
            return Err(CombatError::UnknownSpell(spell.to_owned()));
        }
        // *Approximate*: centre-to-centre distance; the engine measures between bounds.
        if let (Some(range), Some(a), Some(b)) = (
            self.spell_range(who, spell),
            self.entity(who),
            self.entity(target),
        ) {
            let d = distance(a.position, b.position);
            if d > range + 1e-3 {
                return Err(CombatError::OutOfRange { distance: d, range });
            }
        }
        let costs = def.use_costs.clone();
        {
            let c = self.combatant_mut(who).ok_or(CombatError::NoCombatant)?;
            if let Some((n, _)) = costs.iter().find(|(n, a)| c.resources.get(n) + 1e-4 < *a) {
                return Err(CombatError::NotEnoughResources(n.clone()));
            }
            c.resources.pay(&costs);
        }
        let exec_spell = def
            .to_exec()
            .map_err(|e| CombatError::BadSpell(spell.to_owned(), e.to_string()))?;
        let element = def.field("DamageType").unwrap_or("None").trim().to_owned();
        let element = if element.is_empty() {
            "None".to_owned()
        } else {
            element
        };
        let w = SimValue::Guid(self.render(who));
        let t = SimValue::Guid(self.render(target));
        let sp = SimValue::Str(spell.to_owned());
        let ty = SimValue::Str(def.spell_type.clone());
        let el = SimValue::Str(element);
        self.push_event(
            "UsingSpell",
            vec![
                w.clone(),
                sp.clone(),
                ty.clone(),
                el.clone(),
                SimValue::Int(0),
            ],
        );
        self.push_event(
            "UsingSpellOnTarget",
            vec![
                w.clone(),
                t.clone(),
                sp.clone(),
                ty.clone(),
                el.clone(),
                SimValue::Int(0),
            ],
        );
        self.combat_event(CombatEvent::SpellCast {
            caster: who,
            spell: spell.to_owned(),
            target,
        });
        let (Some(src), Some(tgt)) = (self.entity_id(who), self.entity_id(target)) else {
            return Err(CombatError::NoCombatant);
        };
        let mut engine = self.condition_engine();
        let mut dice = std::mem::take(&mut self.dice);
        let (outcome, log, events) = {
            let mut ex = Executor::new(&mut engine, self, &mut dice);
            let outcome = ex.cast(&exec_spell, src, tgt);
            (
                outcome,
                std::mem::take(&mut ex.log),
                std::mem::take(&mut ex.events),
            )
        };
        self.dice = dice;
        for l in &log {
            self.combat_event(CombatEvent::Log(l.clone()));
        }
        let mut hit_zero: Vec<(Guid, Guid)> = Vec::new();
        for e in &events {
            self.apply_exec_event(e, &mut hit_zero);
        }
        self.push_event("CastedSpell", vec![w, sp, ty, el, SimValue::Int(0)]);
        for (victim, killer) in hit_zero {
            self.on_zero_hp(victim, killer);
        }
        let harmful = def.has_flag("IsHarmful") || def.has_flag("IsAttack");
        if !self.in_combat(who) && harmful && self.is_hostile(who, target) {
            self.enter_combat(who, target);
        }
        if let Some(cid) = self.combat_of(who).map(|c| c.id) {
            self.check_combat_end(cid);
        }
        Ok(CastReport {
            outcome,
            log,
            events,
        })
    }

    fn apply_exec_event(&mut self, e: &ExecEvent, hit_zero: &mut Vec<(Guid, Guid)>) {
        let g = |s: &Sim, x: EntityId| s.entity_guid(x).unwrap_or(Guid::NULL);
        match e {
            ExecEvent::Roll(r) => {
                let text = match r {
                    RollEvent::Attack(a) => format!(
                        "{} attacks {}: {a}",
                        self.render(g(self, a.attacker)),
                        self.render(g(self, a.target))
                    ),
                    RollEvent::SavingThrow(s) => {
                        format!("{}: {s}", self.render(g(self, s.entity)))
                    }
                    RollEvent::Die { die, value, dc } => {
                        format!("d{} = {value} vs {dc}", die.faces())
                    }
                };
                self.combat_event(CombatEvent::Roll {
                    roll: r.clone(),
                    text,
                });
            }
            ExecEvent::Damage {
                source,
                target,
                damage_type,
                raw,
                amount,
                critical,
                hp_after,
                ..
            } => {
                let (s, t) = (g(self, *source), g(self, *target));
                let max_hp = self.max_hp(*target);
                self.combat_event(CombatEvent::Damage {
                    source: s,
                    target: t,
                    amount: *amount,
                    raw: *raw,
                    damage_type: *damage_type,
                    critical: *critical,
                    hp: *hp_after,
                    max_hp,
                });
                let (ss, ts) = (self.render(s), self.render(t));
                self.push_event(
                    "AttackedBy",
                    vec![
                        SimValue::Guid(ts),
                        SimValue::Guid(ss.clone()),
                        SimValue::Guid(ss),
                        SimValue::Str(damage_type.name().to_owned()),
                        SimValue::Int(*amount),
                        SimValue::Str("Attack".to_owned()),
                        SimValue::Int(0),
                    ],
                );
                if *hp_after <= 0 && !hit_zero.iter().any(|(v, _)| *v == t) {
                    hit_zero.push((t, s));
                }
            }
            ExecEvent::StatusApplied { target, status, .. } => {
                let t = g(self, *target);
                self.combat_event(CombatEvent::StatusApplied {
                    target: t,
                    status: status.clone(),
                });
            }
            ExecEvent::Healed {
                target,
                amount,
                hp_after,
            } => {
                let t = g(self, *target);
                self.combat_event(CombatEvent::Healed {
                    target: t,
                    amount: *amount,
                    hp: *hp_after,
                });
            }
            ExecEvent::StatusRemoved { .. } => {}
        }
    }

    /// A character dropped to 0 hit points: a party member is downed (`DOWNED` status,
    /// `DownedChanged(x, 1)`); anyone else dies.
    // *Approximate*: party members are downed and everyone else dies, as in the game's
    // default rules; the engine decides through the death/downed systems (esv::death), which
    // were not read.
    pub fn on_zero_hp(&mut self, victim: Guid, killer: Guid) {
        if self.combatant(victim).is_some_and(|c| c.dead || c.downed) {
            return;
        }
        if self.is_player_controlled(victim) {
            if let Some(c) = self.combatant_mut(victim) {
                c.downed = true;
                c.death_saves = (0, 0);
            }
            self.apply_status_with(victim, "DOWNED", -1.0, killer, 0);
            let v = self.render(victim);
            self.push_event("DownedChanged", vec![SimValue::Guid(v), SimValue::Int(1)]);
            self.combat_event(CombatEvent::Downed { entity: victim });
            self.end_turn_if_active(victim);
        } else {
            self.kill(victim);
        }
    }

    /// Death: `Dying(x)`, the `DYING` status (story action id 0, null source), `Died(x)`.
    ///
    /// *Verified by trace* for a scripted death in the opening (`HitpointsChanged(x, 0.0)` →
    /// `Dying` → `StatusAttempt`/`StatusApplied` `DYING` → `Died`); combat deaths are not in
    /// the trace yet.
    pub fn kill(&mut self, victim: Guid) {
        if self.combatant(victim).is_some_and(|c| c.dead) {
            return;
        }
        if self
            .entity(victim)
            .and_then(|e| e.sheet.as_ref())
            .is_some_and(|s| s.hp > 0)
        {
            self.set_hitpoints(victim, 0);
        }
        if let Some(c) = self.combatant_mut(victim) {
            c.dead = true;
            c.downed = false;
        }
        let v = self.render(victim);
        self.push_event("Dying", vec![SimValue::Guid(v.clone())]);
        self.apply_status_with(victim, "DYING", -1.0, Guid::NULL, 0);
        self.push_event("Died", vec![SimValue::Guid(v)]);
        self.combat_event(CombatEvent::Died { entity: victim });
        self.end_turn_if_active(victim);
        if let Some(cid) = self.combat_of(victim).map(|c| c.id) {
            self.check_combat_end(cid);
        }
    }

    fn end_turn_if_active(&mut self, who: Guid) {
        if self.combat_of(who).is_some_and(|c| c.is_active(who)) {
            self.combat_end_turn(who).ok();
        }
    }

    /// A downed character's death saving throw: d20 ≥ 10 succeeds, 1 counts as two
    /// failures, 20 gets the character up with 1 hit point; three successes stabilise
    /// (stays downed), three failures kill.
    // *Approximate*: the D&D 5e rule; the engine's DOWNED status machine was not read.
    pub fn death_saving_throw(&mut self, who: Guid) {
        let Some(&super::Combatant {
            downed,
            death_saves,
            ..
        }) = self.combatant(who)
        else {
            return;
        };
        if !downed || death_saves.0 >= 3 {
            return;
        }
        let roll = self.dice.roll_die(20);
        let (mut s, mut f) = death_saves;
        match roll {
            20 => {
                if let Some(c) = self.combatant_mut(who) {
                    c.downed = false;
                    c.death_saves = (0, 0);
                }
                self.remove_status(who, "DOWNED");
                self.set_hitpoints(who, 1);
                let w = self.render(who);
                self.push_event("DownedChanged", vec![SimValue::Guid(w), SimValue::Int(0)]);
            }
            1 => f += 2,
            r if r >= 10 => s += 1,
            _ => f += 1,
        }
        if roll != 20
            && let Some(c) = self.combatant_mut(who)
        {
            c.death_saves = (s.min(3), f.min(3));
        }
        self.combat_event(CombatEvent::DeathSave {
            entity: who,
            roll,
            successes: s.min(3),
            failures: f.min(3),
        });
        if f >= 3 {
            self.kill(who);
        }
    }

    /// The weapon an attack uses (for [`RulesContext::attack_weapon`]).
    pub(crate) fn rules_attack_weapon(
        &self,
        id: Guid,
        attack: bg3_rules::context::AttackType,
    ) -> Option<bg3_rules::context::Weapon> {
        let hand = if attack.is_offhand() {
            Hand::Off
        } else {
            Hand::Main
        };
        self.combat_weapon(id, attack.is_ranged(), hand)
    }
}
