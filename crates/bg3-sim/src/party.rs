//! Recruitment in a live game: the engine calls that make a character a player and a party
//! member (`MakePlayer`, `RegisterAsCompanion`), as the story's `PROC_GLO_PartyMembers_Add`
//! uses them when a companion joins (Lae'zel on the Nautiloid).
//!
//! Like [`crate::presentation`], these answer the story only in a live game
//! ([`crate::presentation::Stage::live`]); the trace harness learns the party from the
//! trace's `CharacterJoinedParty` events instead.
//!
//! Event order (*verified by trace* `combat-20261021-183321` for Lae'zel, *approximate* in
//! timing): `CharacterMadePlayer(x)`, `CharacterReservedUserIDChanged(x, -65536, user)`, and
//! later `CharacterJoinedParty(x)`; the engine raises the last one frames later (after the
//! story's reactions to the first two), the sim queues it right away.

use bg3_osiris::{HostCtx, Value};

use crate::events::SimValue;
use crate::guid::Guid;
use crate::sim::Sim;

/// `CharacterReservedUserIDChanged`'s "no user" value (as in `start.rs`).
const NO_USER: i32 = -65536;

pub(crate) fn call(
    sim: &mut Sim,
    ctx: &mut HostCtx<'_>,
    name: &str,
    args: &[Value],
) -> Option<bool> {
    if !sim.stage.live {
        return None;
    }
    let g = |i: usize| {
        ctx.text(&args[i])
            .and_then(Guid::from_osiris)
            .unwrap_or(Guid::NULL)
    };
    Some(match (name, args.len()) {
        // bg3: esv::OsirisCharacterFunctions::MakePlayer (4.1.1.7631656)
        ("MakePlayer", 2 | 3) => sim.make_player(g(0), g(1)),
        // bg3: esv::OsirisCharacterFunctions::RegisterAsCompanion (4.1.1.7631656)
        ("RegisterAsCompanion", 2) => sim.register_as_companion(g(0)),
        _ => return None,
    })
}

impl Sim {
    /// `MakePlayer(character, player)`: the character is controlled by `player`'s user.
    pub fn make_player(&mut self, who: Guid, player: Guid) -> bool {
        if !self.exists(who) {
            return false;
        }
        let user = self.entity(player).and_then(|e| e.user);
        if let Some(e) = self.entities.get_mut(who) {
            e.user = user;
        }
        let w = SimValue::Guid(self.render(who));
        self.push_event("CharacterMadePlayer", vec![w.clone()]);
        if let Some(u) = user {
            self.push_event(
                "CharacterReservedUserIDChanged",
                vec![w, SimValue::Int(NO_USER), SimValue::Int(u)],
            );
        }
        true
    }

    /// `RegisterAsCompanion(character, player)`: the character joins the party.
    pub fn register_as_companion(&mut self, who: Guid) -> bool {
        if !self.exists(who) {
            return false;
        }
        if !self.party.contains(&who) {
            self.party.push(who);
            let w = SimValue::Guid(self.render(who));
            self.push_event("CharacterJoinedParty", vec![w]);
        }
        true
    }
}
