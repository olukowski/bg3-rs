//! Headless, deterministic Osiris rule-engine VM for Baldur's Gate 3 stories.
//!
//! Osiris is Larian's forward-chaining rule engine (`libOsiris`). A compiled story
//! (`story.div.osi`, loaded by [`bg3_story`]) is a rete network: start nodes for events,
//! procedures and databases, join/negation/compare nodes, and rule terminals with action
//! lists, grouped into goals. This crate runs such a story with the original's semantics,
//! as established by reading libOsiris 4.1.1.7631656 (see `docs/research/osiris-vm.md`):
//!
//! * databases keep facts in insertion order, and every lookup returns them in that order;
//! * evaluation is synchronous and depth-first (an inserted fact or called PROC runs every
//!   affected rule before the next action);
//! * goals sleep until their parents complete, then run INIT; `GoalCompleted` runs EXIT;
//! * engine calls/queries go through a [`Host`]; events come in through [`Vm::event`].
//!
//! ```no_run
//! # let story: bg3_story::Story = unimplemented!();
//! let mut vm = bg3_osiris::Vm::new(&story);
//! let mut host = bg3_osiris::RecordingHost::new();
//! vm.init_game(&mut host);
//! let rows = vm.db("DB_Players", 1);
//! ```

pub mod builder;
mod db;
mod host;
mod program;
mod value;
mod vm;

pub use host::{Host, HostCtx, Recorded, RecordingHost};
pub use value::{
    Data, OsiStr, Root, StringTable, TypeId, TypeTable, Value, parse_uuid_suffix, types,
};
pub use vm::{EventOutcome, GoalState, Stats, Token, Vm, VmError};

#[cfg(test)]
mod tests;
