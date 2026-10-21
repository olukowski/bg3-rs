//! Headless, deterministic Baldur's Gate 3 world simulation: the engine side of the Osiris
//! story.
//!
//! The story (`bg3_osiris`) asks the engine questions (`Exists`, `IsTagged`, `GetFlag`, …),
//! tells it to do things (`SetFlag`, `SetTag`, `ObjectTimerLaunch`, …) and is told what
//! happened through events (`FlagSet`, `TagSet`, `ObjectTimerFinished`, …). This crate keeps
//! the world state those functions work on and implements them with the original's
//! semantics, as read in the game binary (each function cites its symbol) and checked
//! against a runtime trace of the original (see `docs/research/osiris-engine-api.md`).
//!
//! * [`Sim`]: entities (from level/globals objects and root templates), flags, tags,
//!   statuses, timers on a sim clock, and the queue of events for the story.
//! * [`SimHost`] / [`SimFunctions`]: the `bg3_osiris::Host` over the sim. Calls change the
//!   state and queue events; events are never delivered during a call (the engine doesn't).
//! * [`StoryState`]: a small trait over flags, tags and entities for other systems.
//! * [`dialog`]: the dialog manager. A [`bg3_dialog::DialogRunner`] plays each instance with
//!   the sim as its [`bg3_dialog::DialogWorld`] ([`SimDialogWorld`]); what it does becomes
//!   story events (`DialogStarted`, `FlagSet`, `RollResult`, …) and [`Notification`]s.
//! * [`rules`]: [`bg3_rules::RulesContext`] for the sim, over the characters' [`CharacterSheet`]s
//!   ([`character`]).
//! * [`replay`]: runs a captured trace with the sim answering what it can, and reports how
//!   much of the story's behaviour the sim reproduces.
//! * [`driver`]: deliver the sim's events to the story without a trace; [`Game`] owns the
//!   story VM and the sim, ticks them and takes the player's input.
//! * [`load`]: [`GameData`] from the install.
//!
//! No Bevy, no I/O besides [`load`]; the same inputs give the same results.

pub mod api;
pub mod character;
pub mod combat;
pub mod dialog;
pub mod driver;
pub mod entity;
pub mod events;
pub mod flags;
pub mod guid;
pub mod host;
pub mod inventory;
pub mod load;
pub mod replay;
pub mod rules;
pub mod sim;
pub mod status;
pub mod tags;
pub mod timers;

pub use api::StoryState;
pub use character::{CharacterDef, CharacterSheet};
pub use combat::{Combat, CombatEvent, Combatant};
pub use dialog::{
    DialogBank, DialogError, DialogInstance, DialogRollRecord, DialogScript, InstanceState,
    Notification, SimDialogWorld,
};
pub use driver::Game;

pub use entity::{Entities, Entity, EntityKind, Origin};
pub use events::{SimEvent, SimValue};
pub use flags::{FlagDef, FlagDefs, FlagError, FlagUsage};
pub use guid::Guid;
pub use host::{SimFunctions, SimHost};
pub use sim::{
    GameData, ModuleLevels, ObservedArg, PlacedObject, RaceInfo, Sim, TemplateInfo, WeightCategory,
};
pub use status::{StatusDef, StatusDefs};
pub use tags::{TagDef, TagDefs, TagSource};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_integration;
