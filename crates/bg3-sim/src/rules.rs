//! The rules engine over the sim: [`bg3_rules::RulesContext`] for [`Sim`], so conditions
//! (`HasStatus`, `Tagged`, ability scores, …), functors (`ApplyStatus`, `DealDamage`, …) and
//! rolls evaluate against sim entities.
//!
//! Entities are identified by their stable handle ([`crate::Entities::handle`]) as a
//! [`EntityId`]. Characters answer from their [`crate::CharacterSheet`]; objects without one
//! (items, triggers, characters without a `Character` stats entry) have neutral scores (10),
//! no hit points and are never dead.

use bg3_rules::context::{Ability, EntityId, RulesContext, Skill};
use bg3_rules::dice::{DiceRng, RollDefinition, RollStream};

use crate::character::CharacterSheet;
use crate::entity::EntityKind;
use crate::events::SimValue;
use crate::guid::Guid;
use crate::sim::Sim;

/// The sim's dice: one MT19937-64 roll stream (`bg3_rules::RollStream`).
///
/// The engine keeps several roll streams per entity and roll type
/// (`esv::roll::stream::StreamsComponent`); their seeds are not known, so the sim uses one
/// stream with a seed of its own. Same seed, same rolls.
#[derive(Clone)]
pub struct SimDice {
    stream: RollStream,
}

impl SimDice {
    pub fn new(seed: u64) -> Self {
        Self {
            stream: RollStream::new(seed),
        }
    }

    pub fn seed(&self) -> u64 {
        self.stream.seed()
    }

    /// Values drawn so far.
    pub fn counter(&self) -> u64 {
        self.stream.counter()
    }

    /// A uniform integer in `0..n` (`n` ≥ 1).
    pub fn below(&mut self, n: usize) -> usize {
        self.stream.next_in(0, n.max(1) as i64 - 1) as usize
    }
}

impl Default for SimDice {
    fn default() -> Self {
        Self::new(0)
    }
}

impl std::fmt::Debug for SimDice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimDice")
            .field("seed", &self.seed())
            .field("counter", &self.counter())
            .finish()
    }
}

impl DiceRng for SimDice {
    fn roll_die(&mut self, faces: u32) -> u32 {
        self.stream.roll_die(faces)
    }
}

impl Sim {
    /// The rules handle of an entity.
    pub fn entity_id(&self, id: Guid) -> Option<EntityId> {
        self.entities.handle(id).map(EntityId)
    }

    /// The UUID behind a rules handle.
    pub fn entity_guid(&self, e: EntityId) -> Option<Guid> {
        self.entities.id_of(e.0)
    }

    fn sheet(&self, e: EntityId) -> Option<&CharacterSheet> {
        self.entities.by_handle(e.0)?.sheet.as_ref()
    }

    fn with_sheet<T: Default>(&self, e: EntityId, f: impl FnOnce(&CharacterSheet) -> T) -> T {
        self.sheet(e).map(f).unwrap_or_default()
    }

    /// Change a character's hit points and raise `HitpointsChanged(character, percentage)`
    /// if they changed.
    // bg3: esv::StoryGameEventManager::ThrowHitpointsChanged (4.1.1.7631656)
    pub fn set_hitpoints(&mut self, id: Guid, hp: i32) {
        let Some(e) = self.entities.get_mut(id) else {
            return;
        };
        let Some(s) = e.sheet.as_mut() else {
            return;
        };
        let hp = hp.clamp(0, s.max_hp);
        if hp == s.hp {
            return;
        }
        s.hp = hp;
        let pct = if s.max_hp > 0 {
            100.0 * hp as f32 / s.max_hp as f32
        } else {
            0.0
        };
        let who = e.osiris_name();
        self.push_event(
            "HitpointsChanged",
            vec![SimValue::Guid(who), SimValue::Real(pct)],
        );
    }
}

impl RulesContext for Sim {
    fn exists(&self, e: EntityId) -> bool {
        self.entities.by_handle(e.0).is_some()
    }

    fn name(&self, e: EntityId) -> String {
        self.entity_guid(e)
            .map_or_else(String::new, |g| self.render(g))
    }

    fn is_character(&self, e: EntityId) -> bool {
        self.entities
            .by_handle(e.0)
            .is_some_and(|x| x.kind == EntityKind::Character)
    }

    fn is_item(&self, e: EntityId) -> bool {
        self.entities
            .by_handle(e.0)
            .is_some_and(|x| x.kind == EntityKind::Item)
    }

    /// In the party or controlled by a user.
    fn is_player(&self, e: EntityId) -> bool {
        self.entities
            .by_handle(e.0)
            .is_some_and(|x| x.user.is_some() || self.party.contains(&x.id))
    }

    fn is_dead(&self, e: EntityId) -> bool {
        self.sheet(e).is_some_and(|s| s.hp <= 0)
    }

    fn ability_score(&self, e: EntityId, a: Ability) -> i32 {
        self.sheet(e).map_or(10, |s| s.ability(a))
    }

    fn level(&self, e: EntityId) -> i32 {
        self.sheet(e).map_or(1, |s| s.level)
    }

    fn proficiency_bonus(&self, e: EntityId) -> i32 {
        self.sheet(e).map_or(2, |s| s.proficiency_bonus)
    }

    fn saving_throw_proficient(&self, e: EntityId, a: Ability) -> bool {
        self.with_sheet(e, |s| s.saving_throws.contains(&a))
    }

    fn skill_proficient(&self, e: EntityId, sk: Skill) -> bool {
        self.with_sheet(e, |s| s.skills.contains(&sk))
    }

    fn skill_expertise(&self, e: EntityId, sk: Skill) -> bool {
        self.with_sheet(e, |s| s.expertise.contains(&sk))
    }

    fn skill_bonus(&self, e: EntityId, sk: Skill) -> i32 {
        self.with_sheet(e, |s| {
            s.skill_bonus
                .iter()
                .filter(|(x, _)| *x == sk)
                .map(|(_, n)| n)
                .sum()
        })
    }

    fn spellcasting_ability(&self, e: EntityId) -> Option<Ability> {
        self.sheet(e)?.spellcasting_ability
    }

    fn armor_class(&self, e: EntityId) -> i32 {
        self.sheet(e).map_or(10, |s| s.armor_class)
    }

    fn hp(&self, e: EntityId) -> i32 {
        self.with_sheet(e, |s| s.hp)
    }

    fn max_hp(&self, e: EntityId) -> i32 {
        self.with_sheet(e, |s| s.max_hp)
    }

    fn level_map_value(&self, e: EntityId, name: &str) -> Option<RollDefinition> {
        self.data
            .level_maps
            .roll(name, RulesContext::level(self, e))
    }

    /// `Tagged('NAME')`: tags are named in conditions; resolved through the tag definitions.
    fn has_tag(&self, e: EntityId, tag: &str) -> bool {
        match (self.entity_guid(e), self.data.tags.by_name(tag)) {
            (Some(g), Some(t)) => self.is_tagged(g, t),
            _ => false,
        }
    }

    fn has_status(&self, e: EntityId, status: &str) -> bool {
        self.entity_guid(e)
            .is_some_and(|g| self.has_active_status(g, status))
    }

    fn has_passive(&self, e: EntityId, passive: &str) -> bool {
        self.with_sheet(e, |s| s.passives.iter().any(|p| p == passive))
    }

    fn set_hp(&mut self, e: EntityId, hp: i32) {
        if let Some(g) = self.entity_guid(e) {
            self.set_hitpoints(g, hp);
        }
    }

    /// Durations are in turns; the sim keeps seconds (a turn is 6 s), -1 = until removed.
    /// The status events carry story action id 0 (no story action; *unverified* for statuses
    /// applied by functors).
    fn apply_status(
        &mut self,
        target: EntityId,
        status: &str,
        duration: i32,
        source: Option<EntityId>,
    ) {
        let Some(t) = self.entity_guid(target) else {
            return;
        };
        let src = source
            .and_then(|s| self.entity_guid(s))
            .unwrap_or(Guid::NULL);
        let secs = if duration < 0 {
            -1.0
        } else {
            duration as f32 * 6.0
        };
        self.apply_status_with(t, status, secs, src, 0);
    }

    fn remove_status(&mut self, target: EntityId, status: &str) {
        if let Some(t) = self.entity_guid(target) {
            Sim::remove_status(self, t, status);
        }
    }
}
