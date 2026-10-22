//! Putting a created character on the sim's avatar.
//!
//! At `CharacterCreationFinished` the engine turns the character-creation definition into
//! the character's components (`esv::character_creation` systems; the stats through
//! `eoc::CharacterCreationStatsComponent`, the progressions through
//! `eoc::ProgressionContainerComponent`, tags through `esv::TagSystem::Update`). What the sim
//! keeps of that:
//!
//! * the [`crate::CharacterSheet`]: final abilities, level 1, class hit points, saving
//!   throws, skills, expertise, spellcasting ability, passives; armour class from the starting
//!   equipment;
//! * the tags of the background, class, subclass, deity and the custom origin (`Engine`
//!   source), announced with `TagSet` at the next tag merge. *Verified by trace*
//!   (`full-nautiloid.log`): the avatar gets `GUILD_ARTISAN`, `CLERIC`, `MYSTRA`, `ALIGN_GOOD`,
//!   `GENERIC`, `REALLY_GENERIC` after `CharacterCreationFinished`, in that order, and the
//!   story's `IsTagged(avatar, MYSTRA)` already answers 1 before the `TagSet`;
//! * for combat: the race/class origin (progression resources and `AddSpells`) and the
//!   created character itself ([`combat_setup`]: starting equipment, proficiencies, spells).
//!
//! Not modelled: the starting items as entities (the trace's `AddedTo` / `Equipped` /
//! `TemplateAddedTo` events), appearance, voice.

use crate::character::CharacterSheet;
use crate::combat::Combatant;
use crate::combat::data::{CombatData, OriginDef, Slot};
use crate::guid::Guid;
use crate::sim::Sim;
use crate::tags::TagSource;

use super::rules::CreatedCharacter;

impl Sim {
    /// Make `id` the character `c` describes (its stats entry and template are the body
    /// preset's, set when it entered the level).
    pub fn apply_created_character(&mut self, id: Guid, c: &CreatedCharacter) {
        let choice = &c.choice;
        self.combat.created_origins.insert(
            id,
            OriginDef {
                class: Some(choice.class),
                subclass: choice.subclass,
                race: Some(choice.race),
                subrace: choice.subrace,
                really_tags: Vec::new(),
            },
        );
        self.combat.created_characters.insert(id, c.clone());
        self.combat.combatants.remove(&id);
        let proficiency_bonus = self
            .entity(id)
            .and_then(|e| e.stats.as_deref())
            .and_then(|s| self.data.characters.get(s))
            .map_or(2, |d| d.proficiency_at(c.level, &self.data.level_maps));
        let Some(e) = self.entities.get_mut(id) else {
            return;
        };
        e.sheet = Some(CharacterSheet {
            abilities: c.abilities,
            level: c.level,
            max_hp: c.max_hp,
            hp: c.max_hp,
            armor_class: c.armor_class,
            proficiency_bonus,
            saving_throws: c.saving_throws.clone(),
            skills: c.skills.clone(),
            expertise: c.expertise.clone(),
            skill_bonus: Vec::new(),
            spellcasting_ability: c.spellcasting_ability,
            passives: c.passives.clone(),
        });
        let mut changed = false;
        for t in &c.tags {
            changed |= e.tags.add(TagSource::Engine, *t);
        }
        if changed {
            self.mark_tags_dirty(id);
        }
    }
}

/// Combat state of a created character: its starting equipment in hand and worn, its
/// proficiencies, its weapon attacks and spells. Called by `Sim::build_combatant` after the
/// generic set-up (which gives it its race and class resources and `AddSpells` lists).
pub fn combat_setup(d: &CombatData, c: &CreatedCharacter, out: &mut Combatant) {
    let mut eq = crate::combat::Equipment {
        weapon_set: c.weapon_set,
        ..Default::default()
    };
    for item in &c.equipment {
        if let Some(w) = d.weapons.get(item) {
            let slot = match w.slot {
                Slot::RangedMain | Slot::RangedOff => &mut eq.ranged_main,
                Slot::MeleeOff => &mut eq.melee_off,
                _ => &mut eq.melee_main,
            };
            if slot.is_none() {
                *slot = Some(item.clone());
            }
        } else if let Some(a) = d.armors.get(item) {
            let slot = if a.shield {
                &mut eq.shield
            } else if a.slot == Slot::Breast {
                &mut eq.body
            } else {
                continue;
            };
            if slot.is_none() {
                *slot = Some(item.clone());
            }
        }
    }
    let mut spells: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        if d.spells.contains_key(s) && !spells.iter().any(|x| x == s) {
            spells.push(s.to_owned());
        }
    };
    if eq.melee_main.is_some() {
        add("Target_MainHandAttack");
    }
    if eq.ranged_main.is_some() {
        add("Projectile_MainHandAttack");
    }
    for s in &out.spells {
        if s == "Target_UnarmedAttack" && eq.melee_main.is_some() {
            continue;
        }
        add(s);
    }
    for s in &c.spells {
        add(s);
    }
    if eq.melee_main.is_none() {
        add("Target_UnarmedAttack");
    }
    out.equipment = eq;
    out.spells = spells;
    out.proficiency = c.proficiencies.clone();
}
