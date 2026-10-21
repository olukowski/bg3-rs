//! Fights the engine starts by itself: hostile characters that notice each other.
//!
//! The engine starts most fights without the story (`esv::combat::System::ProcessStartCombat`,
//! 0x1052835d0; see `docs/research/combat.md`): in the reference combat trace the Nautiloid's
//! imp fight starts right after Lae'zel joins the party (`CharacterJoinedParty`,
//! `BaseFactionChanged` to her companion faction), with no story call, while the imps can see
//! the avatar and her.
//!
//! **Approximate**: noticing is distance only, within the `Sight` of either character
//! (`Character` stats, centimetres; `_Base` 1600). The engine's sight system (`Saw` /
//! `LostSightOf` events: field of view, line of sight, darkness, stealth, hearing) is not
//! modelled; characters in a dialog (requested or running) don't start fights, standing in for
//! the engine keeping dialog participants out of combat.

use super::distance;
use crate::entity::EntityKind;
use crate::guid::Guid;
use crate::sim::Sim;

/// `Sight` when a character has no stats entry (`_Base`: 1600 cm).
const DEFAULT_SIGHT: f32 = 16.0;
/// Characters further than this are not considered at all (a cheap bound; no `Sight` in the
/// reference build's stats is this long).
const SCAN_RADIUS: f32 = 60.0;

impl Sim {
    fn sight(&self, id: Guid) -> f32 {
        self.entity(id)
            .and_then(|e| e.stats.as_deref())
            .and_then(|s| self.data.combat.characters.get(s))
            .and_then(|c| c.sight)
            .unwrap_or(DEFAULT_SIGHT)
    }

    /// Whether `id` could notice or be noticed: on stage, in the current level (or the
    /// party), alive, not in a fight or a dialog.
    fn can_notice(&mut self, id: Guid) -> bool {
        let Some(e) = self.entity(id) else {
            return false;
        };
        if e.kind != EntityKind::Character
            || e.holder.is_some()
            || (!e.global && e.level != self.level && !self.party.contains(&id))
            || !self.on_stage(id)
            || self.combat.off_stage.contains(&id)
            || self.in_combat(id)
            || self.is_speaker_reserved(id)
        {
            return false;
        }
        if e.sheet.as_ref().is_some_and(|s| s.hp <= 0) {
            return false;
        }
        self.ensure_combatant(id) && self.combatant(id).is_some_and(|c| !c.dead && !c.downed)
    }

    /// Start a fight between a party member and the nearest hostile character that notices
    /// it (or that it notices), if any (*approximate*, see the module docs). Returns the
    /// combat GUID. A live front end calls this once per frame.
    pub fn start_combat_on_sight(&mut self) -> Option<Guid> {
        let party = self.party.clone();
        let mut best: Option<(f32, Guid, Guid)> = None;
        for p in party {
            if !self.can_notice(p) {
                continue;
            }
            let Some(pp) = self.entity(p).map(|e| e.position) else {
                continue;
            };
            let near: Vec<(Guid, f32)> = self
                .entities
                .iter()
                .filter(|e| e.kind == EntityKind::Character && e.id != p)
                .filter(|e| !self.party.contains(&e.id))
                .map(|e| (e.id, distance(pp, e.position)))
                .filter(|(_, d)| *d <= SCAN_RADIUS)
                .collect();
            for (h, d) in near {
                if best.is_some_and(|b| b.0 <= d) {
                    continue;
                }
                if d > self.sight(h).max(self.sight(p)) || !self.is_hostile(h, p) {
                    continue;
                }
                if self.can_notice(h) {
                    best = Some((d, h, p));
                }
            }
        }
        let (_, h, p) = best?;
        self.enter_combat(h, p)
    }
}
