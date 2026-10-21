//! Baldur's Gate 3 rules engine (Bevy-free).
//!
//! - Parsers for the stats expression languages: [`functor`] (effects), [`condition`]
//!   (conditions and roll conditions, compiled to Thoth), [`expr`] (formulas like
//!   `1d8+StrengthModifier`), [`fields`] (boosts, requirements, costs, description parameters).
//! - [`thoth`]: the Lua 5.3 dialect behind conditions and `.khn` scripts — parser and
//!   interpreter.
//! - Evaluation against a pluggable [`context::RulesContext`]: [`dice`] (seeded roll streams),
//!   [`rules`] (attack rolls, saving throws, damage and resistance), [`natives`] (engine
//!   functions for conditions) and [`exec`] (running functors, spell casts, a combat log).
//! - [`survey`]: classify and parse every expression field of a loaded stats database.
//!
//! See `docs/formats/stats-expressions.md` for the languages and the sources of each semantic.

pub mod condition;
pub mod context;
pub mod dice;
pub mod exec;
pub mod expr;
pub mod fields;
pub mod functor;
pub mod natives;
pub mod rules;
pub mod survey;
pub mod testworld;
pub mod textkey;
pub mod thoth;

pub use condition::{Condition, ConditionContext, ConditionEngine, RollConditions};
pub use context::{EntityId, RulesContext};
pub use dice::{DiceRng, RollDefinition, RollStream};
pub use functor::FunctorList;

#[cfg(test)]
mod tests;
