//! Thoth: the scripting language behind stats conditions (`.khn` files).
//!
//! Thoth is Lua 5.3 (the engine embeds the stock interpreter, bound to C++ through Larian's
//! "khonsu" layer) with one syntax extension, `try … catch e then … end`. Stats conditions are
//! compiled to Thoth source by `Scripts/thoth/generateConditionString.khn` (see
//! [`crate::condition`]); the helper functions they call are Thoth scripts in
//! `Mods/<Module>/Scripts/thoth/helpers/*.khn`, and the rest are engine natives.
//! See `docs/formats/stats-expressions.md`.

pub mod ast;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod pattern;
pub mod value;

pub use interp::{Host, LResult, LuaError, Machine, NoHost};
pub use parser::{parse_chunk, parse_expr};
pub use value::{ConditionError, ConditionResult, Value};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct SyntaxError {
    pub line: u32,
    pub message: String,
}

#[cfg(test)]
mod tests;
