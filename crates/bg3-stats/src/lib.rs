//! Parser for Baldur's Gate 3 stats text files (`Public/<Module>/Stats/Generated/**.txt`) and a
//! merged, inheritance-resolved view across modules. See `docs/formats/stats.md`.
//!
//! Layers:
//! - [`lexer`]: line/token level.
//! - [`StatsFile`]: typed model of one file (entries, treasure tables, equipment, …).
//! - [`Stats`]: files applied in load order, `using` resolved.
//! - [`install`]: find and load all stats files from a set of paks.
//!
//! Expression-valued fields (functors, conditions, boosts, …) are kept as raw strings.

pub mod db;
pub mod file;
pub mod install;
pub mod lexer;

pub use db::{Source, Stats, StatsObject, Table, Unresolved};
pub use file::{RawEntry, StatsFile};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("line {line}: {message}")]
    Syntax { line: u32, message: String },
    #[error("file is not valid UTF-8")]
    NotUtf8,
    #[error("{path}: {source}")]
    InFile {
        path: String,
        #[source]
        source: Box<Error>,
    },
    #[error("{path}: {source}")]
    Pak {
        path: String,
        #[source]
        source: bg3_pak::Error,
    },
}

#[cfg(test)]
mod tests;
