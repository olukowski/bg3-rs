//! Baldur's Gate 3 navigation data and path finding.
//!
//! * [`aigrid`]: `Mods/<Module>/Levels/<Level>/Ai/aigrid.data`, the walkable-tile grid that
//!   characters path on.
//! * [`navcloud`]: `Mods/GustavDev/Levels/<Level>/NavCloud/navcloud.{meta,data}`, a 3D voxel
//!   volume the game uses for projectile/throw previews, spell casting and following.
//! * [`grid::NavGrid`]: queries (walkable? project a point, raycast) and [`path`] finding.
//!
//! Format and behaviour notes: `docs/formats/navcloud.md`. Coordinates are BG3 world space
//! (left-handed, Y up).

pub mod aigrid;
mod bytes;
mod compress;
pub mod grid;
pub mod navcloud;
pub mod path;

pub use aigrid::{AiGridFile, Subgrid, Tile};
pub use grid::{Agent, NavGrid, TileRef};
pub use navcloud::NavCloud;
pub use path::{Path, PathError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated at byte {0}")]
    Truncated(usize),
    #[error("unsupported version {0}")]
    Version(u32),
    #[error("{what}: expected {expected} bytes, got {got}")]
    Size {
        what: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("decompression failed: {0}")]
    Decompress(String),
    #[error("{0}: not found in any module")]
    NotFound(String),
    #[error(transparent)]
    Vfs(#[from] bg3_vfs::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Path of a level's AI grid in the last module (of `modules`, in load order) that has one.
pub fn aigrid_path(vfs: &bg3_vfs::Vfs, level: &str, modules: &[&str]) -> Option<String> {
    modules
        .iter()
        .rev()
        .map(|m| format!("Mods/{m}/Levels/{level}/Ai/aigrid.data"))
        .find(|p| vfs.exists(p))
}

/// Load a level's AI grid (base modules, later modules override earlier ones — assumption: the
/// whole file is replaced, as for other per-level files).
pub fn load_aigrid(vfs: &bg3_vfs::Vfs, level: &str) -> Result<AiGridFile> {
    let path = aigrid_path(vfs, level, bg3_vfs::modules::BASE_LOAD_ORDER)
        .ok_or_else(|| Error::NotFound(format!("{level}/Ai/aigrid.data")))?;
    AiGridFile::parse(&vfs.read(&path)?)
}

/// Load a level's nav cloud, if it has one (only the main levels do).
pub fn load_navcloud(vfs: &bg3_vfs::Vfs, level: &str) -> Result<NavCloud> {
    for m in bg3_vfs::modules::BASE_LOAD_ORDER.iter().rev() {
        let dir = format!("Mods/{m}/Levels/{level}/NavCloud");
        let meta = format!("{dir}/navcloud.meta");
        if vfs.exists(&meta) {
            return NavCloud::parse(
                &vfs.read(&meta)?,
                &vfs.read(&format!("{dir}/navcloud.data"))?,
            );
        }
    }
    Err(Error::NotFound(format!("{level}/NavCloud/navcloud.meta")))
}

#[cfg(test)]
mod tests;
