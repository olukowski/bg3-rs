//! Baldur's Gate 3 dialogs: the compiled dialog model ([`Dialog`]), a headless runner
//! ([`runner::DialogRunner`]) that walks a dialog with the original's branching rules, and the
//! [`DialogWorld`] trait through which the runner reads and writes game state (flags, tags,
//! rolls).
//!
//! Format notes: `docs/formats/dialog.md`. This crate does not depend on Bevy.

pub mod defs;
pub mod library;
pub mod model;
pub mod runner;
pub mod world;

pub use bg3_lsf::Guid;
pub use model::{
    Dialog, DialogOptions, DialogRef, Flag, FlagGroup, FlagKind, Node, NodeId, NodeKind, Roll,
    Speaker, TaggedText, TextLine,
};
pub use runner::{Choice, DialogEvent, DialogRunner, FlagSlot, Line, StartParams, Step};
pub use world::{
    ActorId, DialogWorld, FlagChange, FlagType, MemoryWorld, RollOutcome, RollRequest,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Lsf(#[from] bg3_lsf::Error),
    #[error(transparent)]
    Vfs(#[from] bg3_vfs::Error),
    #[error("malformed dialog: {0}")]
    Malformed(String),
    #[error("dialog not found: {0}")]
    NotFound(String),
    #[error("invalid runner operation: {0}")]
    InvalidOperation(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests;
