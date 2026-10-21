//! Loading tile sets and page files through the VFS.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bg3_pak::Compression;
use bg3_vfs::{FileRef, Vfs};

use crate::bank::VirtualTextureResource;
use crate::texture::{Image, OutputFormat};
use crate::tileset::{TextureInfo, TileSet};
use crate::{Error, Result};

/// Directory holding BG3's tile sets and page files.
pub const VT_DIR: &str = "Generated/Public/VirtualTextures/";

/// Provides page file bytes by file name (as listed in the tile set).
pub trait PageSource {
    fn page_file(&self, name: &str) -> Result<Cow<'_, [u8]>>;
}

/// In-memory page files (tests, tools).
impl PageSource for HashMap<String, Vec<u8>> {
    fn page_file(&self, name: &str) -> Result<Cow<'_, [u8]>> {
        self.get(name)
            .map(|v| Cow::Borrowed(v.as_slice()))
            .ok_or_else(|| Error::NotFound(name.to_owned()))
    }
}

/// Page files next to a tile set in the VFS. Stored (uncompressed) entries, which is all of BG3's,
/// are borrowed straight from the memory-mapped pak.
#[derive(Debug, Clone)]
pub struct VfsPages<'v> {
    pub vfs: &'v Vfs,
    /// Directory prefix including the trailing `/`.
    pub dir: String,
}

impl<'v> VfsPages<'v> {
    /// Page files in the same directory as the tile set at `tile_set_path`.
    pub fn for_tile_set(vfs: &'v Vfs, tile_set_path: &str) -> Self {
        let dir = match tile_set_path.rfind('/') {
            Some(i) => tile_set_path[..=i].to_owned(),
            None => String::new(),
        };
        Self { vfs, dir }
    }
}

impl PageSource for VfsPages<'_> {
    fn page_file(&self, name: &str) -> Result<Cow<'_, [u8]>> {
        let path = format!("{}{name}", self.dir);
        let file = self.vfs.get(&path).ok_or(Error::NotFound(path))?;
        file_bytes(file)
    }
}

fn file_bytes(file: FileRef<'_>) -> Result<Cow<'_, [u8]>> {
    Ok(match file.entry.compression()? {
        Compression::None => Cow::Borrowed(file.package.raw(file.entry)?),
        _ => Cow::Owned(file.read()?),
    })
}

/// `VirtualTextureLayerConfig` of a virtual texture resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerConfig {
    /// Value 3, the only one used by BG3's banks: layers 0 = albedo (sRGB), 1 = normal,
    /// 2 = physical (linear).
    AlbedoNormalPhysical,
    Other(u32),
}

impl LayerConfig {
    pub fn from_raw(v: u32) -> Self {
        match v {
            3 => Self::AlbedoNormalPhysical,
            v => Self::Other(v),
        }
    }

    /// Base name of this configuration's tile sets.
    pub fn tile_set_name(self) -> Option<&'static str> {
        match self {
            Self::AlbedoNormalPhysical => Some("Albedo_Normal_Physical"),
            Self::Other(_) => None,
        }
    }

    /// Path of the tile set holding `gtex`: `<dir>/<Config>_<first char of gtex>.gts`.
    // bg3: ls::VirtualTextureLayerConfig::GetPath (4.1.1.7631656) builds this path from the
    // config and an optional character; verified against every texture by the install test.
    pub fn tile_set_path(self, gtex: &str) -> Option<String> {
        let name = self.tile_set_name()?;
        let c = gtex.chars().next()?.to_ascii_lowercase();
        Some(format!("{VT_DIR}{name}_{c}.gts"))
    }
}

/// Lazily loaded, cached tile sets from the VFS.
pub struct VirtualTextures<'v> {
    vfs: &'v Vfs,
    tile_sets: Mutex<HashMap<String, Arc<TileSet>>>,
}

impl std::fmt::Debug for VirtualTextures<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualTextures").finish_non_exhaustive()
    }
}

/// A texture located in a tile set.
#[derive(Debug, Clone)]
pub struct Located {
    pub tile_set_path: String,
    pub tile_set: Arc<TileSet>,
    pub texture: TextureInfo,
}

impl<'v> VirtualTextures<'v> {
    pub fn new(vfs: &'v Vfs) -> Self {
        Self {
            vfs,
            tile_sets: Mutex::default(),
        }
    }

    pub fn vfs(&self) -> &'v Vfs {
        self.vfs
    }

    /// Paths of every `.gts` in the VFS, sorted.
    pub fn tile_set_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .vfs
            .files_under("")
            .map(|f| f.entry.name.clone())
            .filter(|n| n.to_ascii_lowercase().ends_with(".gts"))
            .collect();
        v.sort();
        v
    }

    /// Parse (or fetch from the cache) the tile set at `path`.
    pub fn tile_set(&self, path: &str) -> Result<Arc<TileSet>> {
        let key = bg3_vfs::normalize(path);
        if let Some(ts) = self.tile_sets.lock().expect("poisoned").get(&key) {
            return Ok(ts.clone());
        }
        let file = self
            .vfs
            .get(path)
            .ok_or_else(|| Error::NotFound(path.to_owned()))?;
        let ts = Arc::new(TileSet::parse(&file_bytes(file)?)?);
        self.tile_sets
            .lock()
            .expect("poisoned")
            .insert(key, ts.clone());
        Ok(ts)
    }

    /// Page source for the tile set at `tile_set_path`.
    pub fn pages(&self, tile_set_path: &str) -> VfsPages<'v> {
        VfsPages::for_tile_set(self.vfs, tile_set_path)
    }

    /// Find a texture by GTex name and layer config.
    pub fn locate_gtex(&self, gtex: &str, config: LayerConfig) -> Result<Located> {
        let path = config.tile_set_path(gtex).ok_or_else(|| {
            Error::Unsupported(format!("virtual texture layer config {config:?}"))
        })?;
        let tile_set = self.tile_set(&path)?;
        let texture = tile_set
            .texture(gtex)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("{gtex} in {path}")))?;
        Ok(Located {
            tile_set_path: path,
            tile_set,
            texture,
        })
    }

    /// Find the texture of a `VirtualTextureBank` resource.
    pub fn locate(&self, res: &VirtualTextureResource) -> Result<Located> {
        self.locate_gtex(&res.gtex_file_name, LayerConfig::from_raw(res.layer_config))
    }

    /// Extract one layer of a bank resource's texture at `level` (clamped to the finest level
    /// actually stored for that layer).
    pub fn extract(
        &self,
        res: &VirtualTextureResource,
        layer: usize,
        level: usize,
        format: OutputFormat,
    ) -> Result<Image> {
        let loc = self.locate(res)?;
        let finest = loc
            .tile_set
            .finest_stored_level(&loc.texture, layer)
            .ok_or_else(|| Error::NotFound(format!("{} layer {layer}", res.gtex_file_name)))?;
        loc.tile_set.extract(
            &self.pages(&loc.tile_set_path),
            &loc.texture,
            layer,
            level.max(finest),
            format,
        )
    }

    /// Extract a mip chain (see [`TileSet::extract_mips`]) of one layer of a bank resource.
    pub fn extract_mips(
        &self,
        res: &VirtualTextureResource,
        layer: usize,
        first_level: usize,
        format: OutputFormat,
    ) -> Result<Vec<Image>> {
        let loc = self.locate(res)?;
        loc.tile_set.extract_mips(
            &self.pages(&loc.tile_set_path),
            &loc.texture,
            layer,
            first_level,
            format,
        )
    }
}
