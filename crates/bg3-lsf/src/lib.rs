//! Readers for Larian resource files — LSF (binary), LSX (XML) and LSJ (JSON) — into one
//! in-memory tree ([`Resource`] → [`Region`] → [`Node`] → [`Attribute`]).
//!
//! Format notes live in `docs/formats/lsf.md`. Prior art: Norbyte's LSLib documents these
//! formats; this is an independent implementation written from those notes and from the files in
//! the reference install.

mod base64;
pub mod lsf;
pub mod lsj;
pub mod lsx;
mod value;

pub use lsf::{LsfInfo, WriteOptions as LsfWriteOptions, read as parse_lsf, write as to_lsf};
pub use lsj::read as parse_lsj;
pub use lsx::{read as parse_lsx, write as to_lsx};
pub use value::{AttributeType, FsArgument, Guid, TranslatedFSString, TranslatedString, Value};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not an LSF file (bad magic)")]
    BadMagic,
    #[error("unsupported LSF version {0}")]
    UnsupportedVersion(u32),
    #[error("unexpected end of data in {0}")]
    Eof(&'static str),
    #[error("decompression of {section} failed: {msg}")]
    Decompress { section: &'static str, msg: String },
    #[error("unknown attribute type {0:?}")]
    UnknownType(String),
    #[error("invalid string table reference {0:#010x}")]
    BadName(u32),
    #[error("malformed LSF: {0}")]
    Malformed(String),
    #[error("XML error: {0}")]
    Xml(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("malformed {0}")]
    Structure(String),
    #[error("invalid {ty} value {text:?}")]
    BadValue { ty: &'static str, text: String },
    #[error("unrecognised resource format")]
    UnknownFormat,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Which file format a resource was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SourceFormat {
    #[default]
    Lsf,
    Lsx,
    Lsj,
}

/// Engine version stamp (`major.minor.revision.build`) recorded in every resource file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EngineVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
    pub build: u32,
}

impl EngineVersion {
    /// Parses `"4.6.0.900"` (missing trailing parts default to 0).
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split('.').map(|p| p.parse::<u32>());
        let mut next = || parts.next().unwrap_or(Ok(0)).ok();
        let v = EngineVersion {
            major: next()?,
            minor: next()?,
            revision: next()?,
            build: next()?,
        };
        parts.next().is_none().then_some(v)
    }
}

impl std::fmt::Display for EngineVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let EngineVersion {
            major,
            minor,
            revision,
            build,
        } = self;
        write!(f, "{major}.{minor}.{revision}.{build}")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub format: SourceFormat,
    pub version: EngineVersion,
    /// LSF container details; `None` for LSX/LSJ.
    pub lsf: Option<LsfInfo>,
    /// LSX `<header time="…">` / LSJ `"header": {"time": …}` when present.
    pub timestamp: Option<i64>,
    /// LSX `<version lslib_meta="…">` (written by LSLib-converted files) when present.
    pub lslib_meta: Option<String>,
}

/// A parsed resource file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resource {
    pub metadata: Metadata,
    /// Regions in file order.
    pub regions: Vec<Region>,
}

/// A named top-level tree. In LSF the region name is the name of its root node; in LSX they are
/// separate (`<region id="Config"><node id="root">`).
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub name: String,
    pub node: Node,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub name: String,
    /// Map key attribute name (LSF keys section / LSX `key="…"`), if any.
    pub key: Option<String>,
    /// Attributes in file order. Names are normally unique per node but this is not enforced.
    pub attributes: Vec<Attribute>,
    /// Children in file order.
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: Value,
}

impl Resource {
    /// The root node of the first region called `name`.
    pub fn region(&self, name: &str) -> Option<&Node> {
        self.regions
            .iter()
            .find(|r| r.name == name)
            .map(|r| &r.node)
    }

    /// Total number of nodes in all regions.
    pub fn node_count(&self) -> usize {
        fn count(n: &Node) -> usize {
            1 + n.children.iter().map(count).sum::<usize>()
        }
        self.regions.iter().map(|r| count(&r.node)).sum()
    }
}

impl Node {
    pub fn new(name: impl Into<String>) -> Self {
        Node {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Value of the first attribute called `name`.
    pub fn attr(&self, name: &str) -> Option<&Value> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| &a.value)
    }

    /// Convenience: the attribute `name` as a string (see [`Value::as_str`]).
    pub fn attr_str(&self, name: &str) -> Option<&str> {
        self.attr(name).and_then(Value::as_str)
    }

    /// Appends an attribute (builder style).
    pub fn with_attr(mut self, name: impl Into<String>, value: Value) -> Self {
        self.attributes.push(Attribute {
            name: name.into(),
            value,
        });
        self
    }

    /// Appends a child (builder style).
    pub fn with_child(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    /// The first child called `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// All children called `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// Parses any of LSF, LSX or LSJ, detected from the content (not the file name).
pub fn parse(data: &[u8]) -> Result<Resource> {
    if data.starts_with(lsf::MAGIC) {
        return lsf::read(data);
    }
    let text = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    match text.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'<') => lsx::read(text),
        Some(b'{') => lsj::read(text),
        _ => Err(Error::UnknownFormat),
    }
}

#[cfg(test)]
mod tests;
