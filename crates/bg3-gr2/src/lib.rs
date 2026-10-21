//! Reader for Granny 3D `.GR2` files as shipped with Baldur's Gate 3.
//!
//! Layers:
//! * [`header`]: magic, file info and section table.
//! * [`bitknit`]: the BitKnit 2 section codec (the only one BG3 uses).
//! * [`file`]: [`Gr2File`] — decompressed sections with relocated pointers.
//! * [`reflect`]: Granny's embedded type descriptions and a generic object walker.
//! * [`model`]: typed meshes / skeletons / models extracted on top of the walker.
//!
//! See `docs/formats/gr2.md`.

pub mod bitknit;
pub mod file;
pub mod header;
pub mod model;
pub mod reflect;

pub use file::{Gr2File, Ptr};
pub use header::{Compression, Format, Header, SectionHeader, SectionRef};
pub use model::{
    ArtToolInfo, Bone, BoneBinding, IndexWidth, Mesh, MeshProperties, Model, Scene, Skeleton,
    TopologyGroup, VertexAttribute, VertexComponent,
};
pub use reflect::{Member, MemberType, Object, Transform, TypeDef, Types, Value};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a Granny file (bad magic)")]
    BadMagic,
    #[error("unsupported Granny format {0:?} (only little-endian 64-bit is supported)")]
    UnsupportedFormat(Format),
    #[error("unsupported Granny file revision {0} (expected 7)")]
    UnsupportedRevision(u32),
    #[error("file is truncated: {0}")]
    Truncated(&'static str),
    #[error("file is corrupt: {0}")]
    Corrupt(&'static str),
    #[error("unsupported section compression {0:?}")]
    UnsupportedCompression(Compression),
    #[error("section {section} {what}: {source}")]
    Decompress {
        section: usize,
        what: &'static str,
        #[source]
        source: bitknit::BitKnitError,
    },
    #[error("null pointer dereference")]
    NullPointer,
    #[error("pointer {0:?} out of range")]
    BadPointer(Ptr),
    #[error("unknown member type {0}")]
    UnknownMemberType(u32),
    #[error("missing member {0:?}")]
    MissingMember(String),
    #[error("member {name:?} has unexpected type {found:?}")]
    WrongMemberType { name: String, found: MemberType },
    #[error("unsupported vertex component {name:?} ({kind:?} x{count})")]
    UnsupportedVertexComponent {
        name: String,
        kind: MemberType,
        count: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
