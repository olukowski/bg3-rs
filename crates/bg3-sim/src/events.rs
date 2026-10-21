//! Events the sim raises for the story.
//!
//! The engine never calls `COsiris::Event` from inside an Osiris call: it collects events and
//! delivers them later, each at nesting depth 0 (see `docs/research/osiris-runtime-log.md`).
//! The sim does the same: calls push onto a queue, the driver drains it between events.

use std::fmt;

/// An event argument, typed with the Osiris root type the engine uses for it.
#[derive(Clone, Debug, PartialEq)]
pub enum SimValue {
    Int(i32),
    Real(f32),
    Str(String),
    /// A GUIDSTRING, as the engine renders it (`Name_<uuid>` for objects, bare UUIDs for
    /// tags and null sources, …).
    Guid(String),
    /// A value of an Osiris enum type, by type and label (`CRITICALITYTYPE.NONE`).
    Enum(String, String),
}

impl fmt::Display for SimValue {
    /// The libOsiris log format (`trace::LogValue::key`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(i) => write!(f, "{i}"),
            Self::Real(r) => write!(f, "{r:.1}"),
            Self::Str(s) => write!(f, "\"{s}\""),
            Self::Guid(s) => f.write_str(s),
            Self::Enum(t, l) => write!(f, "{t}.{l}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimEvent {
    pub name: String,
    pub args: Vec<SimValue>,
}

impl SimEvent {
    pub fn new(name: &str, args: Vec<SimValue>) -> Self {
        Self {
            name: name.to_owned(),
            args,
        }
    }

    /// `Name(a, b)` with values in the log format, for matching against a trace.
    pub fn key(&self) -> String {
        let args: Vec<String> = self.args.iter().map(ToString::to_string).collect();
        format!("{}({})", self.name, args.join(", "))
    }
}

impl fmt::Display for SimEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}
