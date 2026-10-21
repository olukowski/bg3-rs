//! World data for Baldur's Gate 3: resource banks, root templates and level contents.
//!
//! Pure data, no rendering. See `docs/research/level-structure.md` for the reference chain
//! level object → root template → visual resource → GR2 + materials.

pub mod banks;
pub mod level;
pub mod templates;

pub use banks::{MaterialResource, MeshObject, ResourceBanks, TextureResource, VisualResource};
pub use level::{Level, LevelObject, Transform};
pub use templates::{RootTemplate, RootTemplates};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Vfs(#[from] bg3_vfs::Error),
    #[error("{path}: {source}")]
    Resource {
        path: String,
        #[source]
        source: bg3_lsf::Error,
    },
    #[error("level {0} not found in any module")]
    LevelNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn parse(vfs: &bg3_vfs::Vfs, path: &str) -> Result<bg3_lsf::Resource> {
    let data = vfs.read(path)?;
    bg3_lsf::parse(&data).map_err(|source| Error::Resource {
        path: path.to_owned(),
        source,
    })
}

pub(crate) fn str_attr(node: &bg3_lsf::Node, name: &str) -> Option<String> {
    node.attr(name)
        .map(|v| v.to_string())
        .filter(|s| !s.is_empty())
}
