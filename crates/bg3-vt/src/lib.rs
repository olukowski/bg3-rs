//! Reader for Granite virtual textures as shipped with Baldur's Gate 3.
//!
//! BG3 streams many environment materials from Granite (Graphine) virtual textures. The data is
//! split into:
//!
//! - **Tile sets** (`.gts`): one per layer configuration and GTex-name prefix, e.g.
//!   `Generated/Public/VirtualTextures/Albedo_Normal_Physical_2.gts`. A tile set describes a huge
//!   virtual image (layers × mip levels × tiles), which textures occupy which sub-rectangles of
//!   it, the codec parameter blocks, and where each tile is stored.
//! - **Page files** (`.gtp`): the tile payloads, grouped into fixed-size pages of chunks.
//!
//! [`TileSet`] parses a `.gts`, [`PageFile`] gives access to the chunks of a `.gtp`, and
//! [`TileSet::extract`] stitches one texture's tiles (borders stripped) into a single BCn or RGBA
//! image. [`VirtualTextures`] ties it to the VFS: material `VirtualTextureParameters` ID →
//! `VirtualTextureBank` resource ([`VirtualTextureResource`]) → `GTexFileName` → tile set and
//! texture rectangle.
//!
//! Format details: `docs/formats/virtual-textures.md`. Prior art: Norbyte's LSLib (MIT), whose
//! `VirtualTextures` reader documents the tile set layout; FastLZ by Ariya Hidayat (MIT) for the
//! LZ77 tile codec. Everything here is written fresh.

mod bank;
pub mod bc;
mod bytes;
pub mod fastlz;
pub mod fourcc;
mod pagefile;
mod texture;
mod tileset;
mod vfs;

#[cfg(test)]
mod tests;

pub use bank::{MaterialVirtualTexture, VirtualTextureResource, material_virtual_textures};
pub use bc::BcFormat;
pub use fourcc::{FourCc, FourCcNode, FourCcValue};
pub use pagefile::{Chunk, PageFile};
pub use texture::{Image, ImageFormat, OutputFormat};
pub use tileset::{
    BcParameters, Codec, DataType, FlatTileInfo, Layer, Level, PackedTileId, PageFileInfo,
    ParameterBlock, ParameterKind, TextureInfo, TileData, TileRef, TileSet,
};
pub use vfs::{LayerConfig, Located, PageSource, VT_DIR, VfsPages, VirtualTextures};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("corrupt virtual texture data: {0}")]
    Corrupt(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("texture not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Vfs(#[from] bg3_vfs::Error),
    #[error(transparent)]
    Pak(#[from] bg3_pak::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn corrupt(msg: impl Into<String>) -> Error {
    Error::Corrupt(msg.into())
}
