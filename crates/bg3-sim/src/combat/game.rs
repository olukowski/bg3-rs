//! Combat on [`Game`]: the sim's combat API with the story events delivered after each step,
//! and an auto-played fight for tools and tests.

use super::ai::AiAction;
use super::{CastReport, CombatError, CombatEvent};
use crate::driver::{Game, run_events};
use crate::guid::Guid;

impl Game {
    fn deliver(&mut self) {
        run_events(&mut self.vm, &mut self.sim, self.max_rounds);
    }

    /// Start a fight (`EnterCombat`), then let the story react.
    pub fn enter_combat(&mut self, source: Guid, target: Guid) -> Option<Guid> {
        let r = self.sim.enter_combat(source, target);
        self.deliver();
        r
    }

    pub fn combat_move(
        &mut self,
        who: Guid,
        distance: f32,
        to: Option<[f32; 3]>,
    ) -> Result<(), CombatError> {
        let r = self.sim.combat_move(who, distance, to);
        self.deliver();
        r
    }

    pub fn combat_cast(
        &mut self,
        who: Guid,
        spell: &str,
        target: Guid,
    ) -> Result<CastReport, CombatError> {
        let r = self.sim.combat_cast(who, spell, target);
        self.deliver();
        r
    }

    pub fn combat_end_turn(&mut self, who: Guid) -> Result<(), CombatError> {
        let r = self.sim.combat_end_turn(who);
        self.deliver();
        r
    }

    /// Let the AI play the non-player turns (see [`crate::Sim::combat_run_ai`]).
    pub fn combat_run_ai(&mut self, combat: Guid, max_turns: usize) -> Vec<(Guid, AiAction)> {
        let mut out = Vec::new();
        for _ in 0..max_turns {
            let step = self.sim.combat_run_ai(combat, 1);
            self.deliver();
            if step.is_empty() {
                break;
            }
            out.extend(step);
        }
        out
    }

    /// Recruited companions get the factions `Companion1`, `Companion2`, … (children of
    /// `Hero`), as the story does when they join (the reference trace: Lae'zel → Companion1).
    /// The story's recruitment is not played here, so set them directly.
    pub fn set_companion_factions(&mut self, party: &[Guid]) {
        for (i, p) in party.iter().enumerate() {
            let want = format!("Companion{}", i + 1);
            let f = self
                .sim
                .data
                .factions
                .iter()
                .find(|(_, n)| **n == want)
                .map(|(id, _)| *id);
            if let Some(f) = f {
                self.sim.set_faction(*p, f);
            }
        }
        self.tick(0.0);
    }

    /// `GameModeStarted("Campaign", 0, 0)`, `CharacterCreationFinished()`, `LevelLoaded(level)`,
    /// `LevelGameplayStarted(level, 0)`: the new-game events of the reference trace.
    pub fn start_level(&mut self, level: &str) {
        use crate::events::{SimEvent, SimValue};
        let s = |x: &str| SimValue::Str(x.to_owned());
        for e in [
            SimEvent::new(
                "GameModeStarted",
                vec![s("Campaign"), SimValue::Int(0), SimValue::Int(0)],
            ),
            SimEvent::new("CharacterCreationFinished", vec![]),
            SimEvent::new("LevelLoaded", vec![s(level)]),
            SimEvent::new("LevelGameplayStarted", vec![s(level), SimValue::Int(0)]),
        ] {
            self.event(e);
            self.tick(0.0);
        }
    }

    pub fn take_combat_events(&mut self) -> Vec<CombatEvent> {
        self.sim.take_combat_events()
    }

    /// Play a whole fight with every side on the (approximate) AI, the party included, until
    /// it ends or `max_turns` turns were taken. Returns the turns played.
    pub fn combat_auto(&mut self, combat: Guid, max_turns: usize) -> Vec<(Guid, AiAction)> {
        let mut out = Vec::new();
        for _ in 0..max_turns {
            let Some(c) = self.sim.combat_by_id(combat).filter(|c| !c.finished) else {
                break;
            };
            let Some(who) = c
                .current_group()
                .and_then(|g| g.members.iter().copied().find(|m| c.is_active(*m)))
            else {
                break;
            };
            let action = match self.sim.ai_turn(who) {
                Ok(a) => a,
                Err(e) => {
                    let n = self.sim.render(who);
                    self.sim
                        .combat_event(CombatEvent::Log(format!("{n}: AI action failed: {e}")));
                    AiAction::Nothing
                }
            };
            if self.sim.combat_of(who).is_some_and(|c| c.is_active(who)) {
                self.sim.combat_end_turn(who).ok();
            }
            self.deliver();
            out.push((who, action));
        }
        out
    }
}
