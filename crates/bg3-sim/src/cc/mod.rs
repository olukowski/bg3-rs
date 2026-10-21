//! Character creation: the options (races, classes, backgrounds, deities, the level-1
//! progressions and their choices), the rules a choice has to satisfy, and the character it
//! makes. See `docs/research/character-creation.md`.
//!
//! * [`data`]: [`CcData`], everything read from the install (`Races.lsx`,
//!   `ClassDescriptions.lsx`, `Backgrounds.lsx`, `Gods.lsx`, `Progressions.lsx`, the `Lists`,
//!   `CharacterCreationPresets`, `AbilityDistributionPresets`, `DefaultValues`, `Origins.lsx`,
//!   stats `Equipment.txt`, `Passive`, `Weapon`, `Armor` entries).
//! * [`rules`]: [`CharacterChoice`] (what the player picks), the selectors of the
//!   progressions, point buy, validation and [`CreatedCharacter`] (the result).
//! * [`apply`]: putting a [`CreatedCharacter`] on the sim's avatar at
//!   `CharacterCreationFinished` (sheet, tags, combat set-up).
//!
//! Bevy-free and deterministic. It lives in `bg3-sim` rather than its own crate because the
//! result is sim state (the avatar's [`crate::CharacterSheet`], tags, combat origin) and the
//! data shares the sim's loaders (`crate::load`).

pub mod apply;
pub mod data;
pub mod rules;

#[cfg(test)]
mod tests;

pub use data::{
    CcBackground, CcClass, CcData, CcGod, CcItem, CcOrigin, CcPreset, CcRace, DefaultKind,
    DefaultValue, Progression,
};
pub use rules::{
    CcError, CharacterChoice, CreatedCharacter, POINT_BUY_BASE, POINT_BUY_MAX, POINT_BUY_POINTS,
    Pick, Selector, SelectorKind, SelectorSlot, point_buy_cost, point_buy_spent,
};
