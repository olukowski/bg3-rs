//! A first, **approximate** enemy turn: walk towards the nearest hostile and hit it with the
//! first damaging spell that reaches.
//!
//! The real AI scores every action, target and position (`esv::ai::...`, `esv::AiHelpers`,
//! e.g. `esv::AiHelpers::CalculateMovementScore`, archetypes from `Public/<Module>/AI/`); see
//! `docs/research/combat.md`. Nothing here is from the binary.

use super::{CombatError, distance};
use crate::guid::Guid;
use crate::sim::Sim;

/// What the AI did on one character's turn.
#[derive(Debug, Clone, PartialEq)]
pub enum AiAction {
    Cast {
        spell: String,
        target: Guid,
        moved: f32,
    },
    MovedOnly {
        target: Guid,
        moved: f32,
    },
    Nothing,
}

impl Sim {
    /// The living hostile participants of `who`'s fight, nearest first.
    pub fn hostiles_by_distance(&self, who: Guid) -> Vec<(Guid, f32)> {
        let Some(c) = self.combat_of(who) else {
            return Vec::new();
        };
        let pos = self.entity(who).map_or([0.0; 3], |e| e.position);
        let mut out: Vec<(Guid, f32)> = c
            .participants
            .iter()
            .copied()
            .filter(|&p| p != who && self.is_hostile(who, p))
            .filter(|&p| self.combatant(p).is_some_and(|x| !x.dead && !x.downed))
            .filter_map(|p| Some((p, distance(pos, self.entity(p)?.position))))
            .collect();
        out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.0.cmp(&b.0.0)));
        out
    }

    /// The damaging spells `who` can afford now, in the order it knows them.
    fn ai_attacks(&self, who: Guid) -> Vec<(String, f32)> {
        let Some(c) = self.combatant(who) else {
            return Vec::new();
        };
        c.spells
            .iter()
            .filter_map(|s| {
                let d = self.data.combat.spells.get(s)?;
                let harmful = d.has_flag("IsHarmful") || d.has_flag("IsAttack");
                let damaging = d
                    .field("SpellSuccess")
                    .is_some_and(|f| f.contains("DealDamage"))
                    || d.field("SpellProperties")
                        .is_some_and(|f| f.contains("DealDamage") && !f.contains("GROUND:"));
                let costs_ok = c.resources.can_pay(&d.use_costs)
                    && d.use_costs
                        .iter()
                        .all(|(n, _)| matches!(n.as_str(), "ActionPoint" | "BonusActionPoint"));
                (harmful && damaging && costs_ok)
                    .then(|| Some((s.clone(), self.spell_range(who, s)?)))
                    .flatten()
            })
            .collect()
    }

    /// Play one AI turn for `who` (*approximate*, see the module docs), then end it.
    pub fn ai_turn(&mut self, who: Guid) -> Result<AiAction, CombatError> {
        let result = self.ai_act(who);
        if self.combat_of(who).is_some_and(|c| c.is_active(who)) {
            self.combat_end_turn(who)?;
        }
        result
    }

    fn ai_act(&mut self, who: Guid) -> Result<AiAction, CombatError> {
        let Some(&(target, dist)) = self.hostiles_by_distance(who).first() else {
            return Ok(AiAction::Nothing);
        };
        let mut attacks = self.ai_attacks(who);
        // Melee first when standing next to the target.
        attacks.sort_by_key(|(s, _)| {
            !self
                .data
                .combat
                .spells
                .get(s)
                .is_some_and(|d| d.has_flag("IsMelee"))
        });
        if attacks.is_empty() {
            return Ok(AiAction::Nothing);
        }
        let movement = self
            .combatant(who)
            .map_or(0.0, |c| c.resources.get("Movement"));
        // Without moving: the first attack in range. Else: walk just far enough for the
        // first attack that can then reach.
        let pick = attacks
            .iter()
            .find(|(_, r)| dist <= *r)
            .map(|(s, _)| (s.clone(), 0.0))
            .or_else(|| {
                attacks
                    .iter()
                    .find(|(_, r)| dist - movement <= *r)
                    .map(|(s, r)| (s.clone(), (dist - r + 0.05).min(movement).max(0.0)))
            });
        let Some((spell, step)) = pick else {
            // Nothing reaches: close in as far as possible.
            let step = movement.min((dist - 1.0).max(0.0));
            if step > 0.0 {
                self.ai_step_towards(who, target, step)?;
            }
            return Ok(AiAction::MovedOnly {
                target,
                moved: step,
            });
        };
        if step > 0.0 {
            self.ai_step_towards(who, target, step)?;
        }
        self.combat_cast(who, &spell, target)?;
        Ok(AiAction::Cast {
            spell,
            target,
            moved: step,
        })
    }

    /// Move `step` metres in a straight line towards `target`.
    fn ai_step_towards(&mut self, who: Guid, target: Guid, step: f32) -> Result<(), CombatError> {
        let (Some(a), Some(b)) = (
            self.entity(who).map(|e| e.position),
            self.entity(target).map(|e| e.position),
        ) else {
            return Ok(());
        };
        let d = distance(a, b);
        let k = if d > 0.0 { (step / d).min(1.0) } else { 0.0 };
        let to = [
            a[0] + (b[0] - a[0]) * k,
            a[1] + (b[1] - a[1]) * k,
            a[2] + (b[2] - a[2]) * k,
        ];
        self.combat_move(who, step, Some(to))
    }

    /// Let the AI play every non-player turn until a player-controlled character is up or
    /// the fight ends. Returns the actions taken. `max_turns` guards against endless loops.
    pub fn combat_run_ai(&mut self, combat: Guid, max_turns: usize) -> Vec<(Guid, AiAction)> {
        let mut out = Vec::new();
        for _ in 0..max_turns {
            let Some(c) = self.combat_by_id(combat) else {
                break;
            };
            if c.finished {
                break;
            }
            let Some(g) = c.current_group() else {
                break;
            };
            let next = g
                .members
                .iter()
                .copied()
                .find(|m| !c.ended.contains(m) && !self.is_player_controlled(*m));
            let Some(who) = next else {
                break;
            };
            let action = self.ai_turn(who).unwrap_or(AiAction::Nothing);
            if self.combat_of(who).is_some_and(|c| c.is_active(who)) {
                self.combat_end_turn(who).ok();
            }
            out.push((who, action));
        }
        out
    }
}
