//! The enemy AI: score every candidate action (spell × target × position), pick the best.
//!
//! Structure of the engine's combat AI (`esv::AiHelpers`, `esv::CombatAIAction`,
//! `esv::AiScoreInternal`, 4.1.1.7631656), written fresh from our reading of the binary;
//! `docs/research/ai.md` has the pipeline, the formulas and what is faithful:
//!
//! - [`archetype`]: the modifier tables (`Public/<Module>/AI/Archetypes`, read at runtime);
//! - [`simulate`]: expected outcome of a spell on a target (chance, damage, heals);
//! - [`position`]: end-position terms (allies/enemies nearby, distance to enemies);
//! - [`score`]: the per-target score and the final action score;
//! - [`plan`]: candidate generation, attacks of opportunity, sorting, execution.
//!
//! A turn: plan, execute the best candidate if it scores above zero, plan again with what
//! is left, until nothing scores; a move towards an action that can't be done this turn
//! ends the turn. The choice is deterministic (no random draw in the engine's selection).

pub mod archetype;
pub mod plan;
pub mod position;
pub mod score;
pub mod simulate;

pub use plan::{AiPlan, Candidate, Tile};

use super::{CombatError, distance};
use crate::guid::Guid;
use crate::sim::Sim;

/// What the AI did on one character's turn (its first action).
#[derive(Debug, Clone, PartialEq)]
pub enum AiAction {
    Cast {
        spell: String,
        target: Guid,
        moved: f32,
        /// Walked BG3-space polyline, including both endpoints; empty without movement.
        path: Vec<[f32; 3]>,
    },
    MovedOnly {
        target: Guid,
        moved: f32,
        /// Walked BG3-space polyline, including both endpoints; empty without movement.
        path: Vec<[f32; 3]>,
    },
    Nothing,
}

impl AiAction {
    pub fn path(&self) -> &[[f32; 3]] {
        match self {
            Self::Cast { path, .. } | Self::MovedOnly { path, .. } => path,
            Self::Nothing => &[],
        }
    }
}

/// Actions per turn at most (a guard; characters run out of resources first).
const MAX_ACTIONS_PER_TURN: usize = 6;

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

    /// Play one AI turn for `who`, then end it.
    pub fn ai_turn(&mut self, who: Guid) -> Result<AiAction, CombatError> {
        self.ai_turn_traced(who, &mut |_, _| {})
    }

    /// [`Sim::ai_turn`], showing each decision's plan to `on_plan` before it is carried out.
    pub fn ai_turn_traced(
        &mut self,
        who: Guid,
        on_plan: &mut dyn FnMut(&Sim, &AiPlan),
    ) -> Result<AiAction, CombatError> {
        let result = self.ai_act(who, on_plan);
        if self.combat_of(who).is_some_and(|c| c.is_active(who)) {
            self.combat_end_turn(who)?;
        }
        result
    }

    fn ai_act(
        &mut self,
        who: Guid,
        on_plan: &mut dyn FnMut(&Sim, &AiPlan),
    ) -> Result<AiAction, CombatError> {
        let mut planner = self.ai_planner();
        let mut first: Option<AiAction> = None;
        for _ in 0..MAX_ACTIONS_PER_TURN {
            if !self.combat_of(who).is_some_and(|c| c.is_active(who)) {
                break;
            }
            let Some(plan) = self.ai_plan_with(&mut planner, who) else {
                break;
            };
            on_plan(self, &plan);
            let Some(c) = plan.chosen().cloned() else {
                break;
            };
            // After acting, only actions doable this turn (no walking off afterwards).
            if first.is_some() && !c.this_turn {
                break;
            }
            self.ai_execute(who, &c)?;
            let action = if c.this_turn {
                AiAction::Cast {
                    spell: c.spell.clone(),
                    target: c.target,
                    moved: c.tile.cost,
                    path: c.tile.path.clone(),
                }
            } else {
                AiAction::MovedOnly {
                    target: c.target,
                    moved: c.tile.cost,
                    path: c.tile.path.clone(),
                }
            };
            first.get_or_insert(action);
            if !c.this_turn {
                break;
            }
        }
        Ok(first.unwrap_or(AiAction::Nothing))
    }

    /// Let the AI play every non-player turn until a player-controlled character is up or
    /// the fight ends. Returns the actions taken. `max_turns` guards against endless loops.
    pub fn combat_run_ai(&mut self, combat: Guid, max_turns: usize) -> Vec<(Guid, AiAction)> {
        self.combat_run_ai_traced(combat, max_turns, &mut |_, _| {})
    }

    /// [`Sim::combat_run_ai`] with every decision's plan shown to `on_plan`.
    pub fn combat_run_ai_traced(
        &mut self,
        combat: Guid,
        max_turns: usize,
        on_plan: &mut dyn FnMut(&Sim, &AiPlan),
    ) -> Vec<(Guid, AiAction)> {
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
            let action = self
                .ai_turn_traced(who, on_plan)
                .unwrap_or(AiAction::Nothing);
            if self.combat_of(who).is_some_and(|c| c.is_active(who)) {
                self.combat_end_turn(who).ok();
            }
            out.push((who, action));
        }
        out
    }
}
