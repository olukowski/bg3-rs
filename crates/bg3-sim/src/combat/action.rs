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
                let cost = vec![(super::ResourceSelector::from("Movement"), distance.max(0.0))];
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
        self.combat_cast_with_slot(who, spell, target, None)
    }

    /// Cast using the lowest sufficient spell slot, or an explicitly selected level.
    pub fn combat_cast_with_slot(
        &mut self,
        who: Guid,
        spell: &str,
        target: Guid,
        slot_level: Option<u32>,
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
        let chosen_slot = costs
            .iter()
            .filter(|(r, _)| r.name == "SpellSlotsGroup")
            .map(|(r, _)| r.level)
            .max()
            .and_then(|minimum| {
                slot_level.or_else(|| {
                    let c = self.combatant(who)?;
                    c.resources
                        .entries
                        .iter()
                        .filter(|r| r.level >= minimum)
                        .map(|r| r.level)
                        .filter(|&level| {
                            let mut resources = c.resources.clone();
                            resources.pay_with_slot(&costs, Some(level))
                        })
                        .min()
                })
            });
        // Upcast spell variants carry the changed expressions and costs in the install.
        // Use those expressions rather than inventing a generic scaling rule.
        let exec_def = if let Some(level) = chosen_slot {
            let power = def
                .field("PowerLevel")
                .or_else(|| def.field("Level"))
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            if level > power {
                let root = def.field("RootSpellID").unwrap_or(spell);
                self.data
                    .combat
                    .spells
                    .get(&format!("{root}_{level}"))
                    .unwrap_or(&def)
            } else {
                &def
            }
        } else {
            &def
        };
        let exec_spell = exec_def
            .to_exec()
            .map_err(|e| CombatError::BadSpell(spell.to_owned(), e.to_string()))?;
        let c = self.combatant_mut(who).ok_or(CombatError::NoCombatant)?;
        if !c.resources.pay_with_slot(&costs, slot_level) {
            return Err(CombatError::NotEnoughResources(
                costs
                    .iter()
                    .map(|(r, _)| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let school = def.field("SpellSchool").unwrap_or("").trim().to_owned();
        let school = if school == "None" {
            String::new()
        } else {
            school
        };
        let id = self.next_story_action();
        let w = SimValue::Guid(self.render(who));
        let t = SimValue::Guid(self.render(target));
        let sp = SimValue::Str(spell.to_owned());
        let ty = SimValue::Str(def.spell_type.to_ascii_lowercase());
        let sc = SimValue::Str(school);
        let spell_args = vec![
            w.clone(),
            sp.clone(),
            ty.clone(),
            sc.clone(),
            SimValue::Int(id),
        ];
        self.push_event("UsingSpell", spell_args.clone());
        self.push_event(
            "UsingSpellOnTarget",
            vec![w.clone(), t.clone(), sp, ty, sc, SimValue::Int(id)],
        );
        let is_attack = def.has_flag("IsAttack");
        if is_attack {
            // StartAttack(target, attacker owner, attacker, story action).
            self.push_event(
                "StartAttack",
                vec![t.clone(), w.clone(), w.clone(), SimValue::Int(id)],
            );
        }
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
        let mut attacked: Vec<Vec<SimValue>> = Vec::new();
        for e in &events {
            self.apply_exec_event(e, id, &mut hit_zero, &mut attacked);
        }
        // A roll against the target that dealt no damage (a miss, a saved cantrip) still
        // raises AttackedBy, with no damage type and 0 (*verified by trace* for missed
        // attacks).
        let rolled = events.iter().any(|e| matches!(e, ExecEvent::Roll(_)));
        if attacked.is_empty() && rolled && target != who {
            attacked.push(vec![
                t,
                w.clone(),
                w,
                SimValue::Str(String::new()),
                SimValue::Int(0),
                SimValue::Str("Attack".to_owned()),
                SimValue::Int(id),
            ]);
        }
        // Story events after the cast resolves, in the order of the reference trace
        // (combat-20261021-183321): CastSpell; per kill Dying + KilledBy; AttackedBy (when
        // nothing died); CastedSpell; then per kill StatusAttempt(DYING) and LeftCombat,
        // AttackedBy, StatusApplied(DYING), statuses removed, Died.
        self.push_event("CastSpell", spell_args.clone());
        let (kills, downs): (Vec<_>, Vec<_>) = hit_zero
            .into_iter()
            .filter(|(v, _)| self.combatant(*v).is_some_and(|c| !c.dead && !c.downed))
            .partition(|(v, _)| !self.is_player_controlled(*v));
        for (v, k) in &kills {
            self.begin_death(*v, Some((*k, id)));
        }
        if kills.is_empty() {
            for a in attacked.drain(..) {
                self.push_event("AttackedBy", a);
            }
        }
        self.push_event("CastedSpell", spell_args);
        let mut combats: Vec<Guid> = Vec::new();
        for (v, _) in &kills {
            if let Some(cid) = self.death_leave_combat(*v) {
                combats.push(cid);
            }
        }
        for a in attacked {
            self.push_event("AttackedBy", a);
        }
        for (v, _) in &kills {
            self.finish_death(*v);
        }
        for (v, k) in downs {
            self.on_zero_hp(v, k);
        }
        let harmful = def.has_flag("IsHarmful") || is_attack;
        if !self.in_combat(who) && harmful && self.is_hostile(who, target) {
            self.enter_combat(who, target);
        }
        if let Some(cid) = self.combat_of(who).map(|c| c.id) {
            combats.push(cid);
        }
        for cid in combats {
            self.check_combat_end(cid);
        }
        Ok(CastReport {
            outcome,
            log,
            events,
        })
    }

    /// The next story action id (the engine numbers player and AI actions; the ids in the
    /// trace are larger because other actions came first).
    pub fn next_story_action(&mut self) -> i32 {
        self.combat.story_actions += 1;
        self.combat.story_actions
    }

    fn apply_exec_event(
        &mut self,
        e: &ExecEvent,
        action: i32,
        hit_zero: &mut Vec<(Guid, Guid)>,
        attacked: &mut Vec<Vec<SimValue>>,
    ) {
        let g = |s: &Sim, x: EntityId| s.entity_guid(x).unwrap_or(Guid::NULL);
        match e {
            ExecEvent::Roll(r) => {
                let text = match r {
                    RollEvent::Attack(a) => format!(
                        "{} attacks {}: {a}",
                        self.display_name(g(self, a.attacker))
                            .unwrap_or_else(|| self.render(g(self, a.attacker))),
                        self.display_name(g(self, a.target))
                            .unwrap_or_else(|| self.render(g(self, a.target)))
                    ),
                    RollEvent::SavingThrow(s) => {
                        format!(
                            "{}: {s}",
                            self.display_name(g(self, s.entity))
                                .unwrap_or_else(|| self.render(g(self, s.entity)))
                        )
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
                // AttackedBy(defender, attacker owner, attacker, damage type, amount, cause,
                // story action): *verified by trace* for weapon attacks and Sacred Flame.
                attacked.push(vec![
                    SimValue::Guid(ts),
                    SimValue::Guid(ss.clone()),
                    SimValue::Guid(ss),
                    SimValue::Str(damage_type.name().to_owned()),
                    SimValue::Int(*amount),
                    SimValue::Str("Attack".to_owned()),
                    SimValue::Int(action),
                ]);
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

    /// Kill a character outside a cast (a failed death save, a story `Die`): the whole
    /// death sequence of [`Sim::begin_death`], [`Sim::death_leave_combat`] and
    /// [`Sim::finish_death`].
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
        self.begin_death(victim, None);
        let cid = self.death_leave_combat(victim);
        self.finish_death(victim);
        if let Some(cid) = cid {
            self.check_combat_end(cid);
        }
    }

    /// First part of a death: `Dying(x)`, then `KilledBy(x, killer owner, killer, story
    /// action)` when there is a killer.
    // *Verified by trace* (combat-20261021-183321, the imps' deaths).
    pub fn begin_death(&mut self, victim: Guid, killer: Option<(Guid, i32)>) {
        if let Some(c) = self.combatant_mut(victim) {
            c.dead = true;
            c.downed = false;
        }
        let v = self.render(victim);
        self.push_event("Dying", vec![SimValue::Guid(v.clone())]);
        if let Some((k, id)) = killer {
            let ks = self.render(k);
            self.push_event(
                "KilledBy",
                vec![
                    SimValue::Guid(v),
                    SimValue::Guid(ks.clone()),
                    SimValue::Guid(ks),
                    SimValue::Int(id),
                ],
            );
        }
        self.combat_event(CombatEvent::Died { entity: victim });
    }

    /// Second part: `StatusAttempt(x, "DYING", null, 0)`, then `LeftCombat(x, combat)` if
    /// the character was in a fight (returned).
    pub fn death_leave_combat(&mut self, victim: Guid) -> Option<Guid> {
        let v = self.render(victim);
        self.push_event(
            "StatusAttempt",
            vec![
                SimValue::Guid(v.clone()),
                SimValue::Str("DYING".to_owned()),
                SimValue::Guid(Guid::NULL.to_string()),
                SimValue::Int(0),
            ],
        );
        let cid = self.combat_of(victim).map(|c| c.id)?;
        if let Some(i) = self.combat.combats.iter().position(|c| c.id == cid) {
            // A dead member of the acting group is done; its story TurnEnded is not raised.
            let c = &mut self.combat.combats[i];
            if c.current_group()
                .is_some_and(|g| g.members.contains(&victim))
                && !c.ended.contains(&victim)
            {
                c.ended.push(victim);
            }
        }
        if let Some(c) = self.combatant_mut(victim) {
            c.combat = None;
        }
        self.push_event(
            "LeftCombat",
            vec![SimValue::Guid(v), SimValue::Guid(cid.to_string())],
        );
        Some(cid)
    }

    /// Last part: `StatusApplied(x, "DYING", null, 0)`, the character's other statuses
    /// removed, `Died(x)`.
    // *Approximate*: the trace removes each status with the story action id it was applied
    // with (the sim does not keep it and reports 0), and also raises tag, sight and loot
    // events the sim does not model.
    pub fn finish_death(&mut self, victim: Guid) {
        let v = self.render(victim);
        let others: Vec<String> = self
            .entity(victim)
            .map(|e| e.statuses.iter().map(|s| s.id.clone()).collect())
            .unwrap_or_default();
        if let Some(e) = self.entities.get_mut(victim) {
            e.statuses.clear();
            e.statuses.push(crate::status::ActiveStatus {
                id: "DYING".to_owned(),
                source: Guid::NULL,
                remaining: -1.0,
            });
        }
        self.push_event(
            "StatusApplied",
            vec![
                SimValue::Guid(v.clone()),
                SimValue::Str("DYING".to_owned()),
                SimValue::Guid(Guid::NULL.to_string()),
                SimValue::Int(0),
            ],
        );
        for s in others.into_iter().filter(|s| s != "DYING") {
            self.push_event(
                "StatusRemoved",
                vec![
                    SimValue::Guid(v.clone()),
                    SimValue::Str(s),
                    SimValue::Guid(Guid::NULL.to_string()),
                    SimValue::Int(0),
                ],
            );
        }
        self.push_event("Died", vec![SimValue::Guid(v)]);
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
