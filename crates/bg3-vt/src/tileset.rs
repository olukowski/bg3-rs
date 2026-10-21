//! `.gts` tile set parsing.

use std::collections::HashMap;

use crate::bc::BcFormat;
use crate::bytes::{Cursor, slice};
use crate::fourcc::{self, FourCcNode};
use crate::pagefile::Chunk;
use crate::{Error, Result, corrupt, fastlz};

pub(crate) const GTS_MAGIC: &[u8; 4] = b"GRPG";
pub(crate) const HEADER_SIZE: usize = 0xC0;
const PAGE_FILE_INFO_SIZE: usize = 512 + 4 + 16 + 4;
/// Set in a per-level tile index when the tile isn't stored at that level; the remaining bits
/// index the flat tile of a coarser level that covers the area instead.
const FALLBACK_BIT: u32 = 0x8000_0000;

/// Pixel data type of a layer or parameter block (Granite's `DataType` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    R8G8B8Srgb,
    R8G8B8A8Srgb,
    X8Y8Z0Tangent,
    R8G8B8Linear,
    R8G8B8A8Linear,
    X8,
    X8Y8,
    X8Y8Z8,
    X8Y8Z8W8,
    Other(u32),
}

impl DataType {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::R8G8B8Srgb,
            1 => Self::R8G8B8A8Srgb,
            2 => Self::X8Y8Z0Tangent,
            3 => Self::R8G8B8Linear,
            4 => Self::R8G8B8A8Linear,
            5 => Self::X8,
            6 => Self::X8Y8,
            7 => Self::X8Y8Z8,
            8 => Self::X8Y8Z8W8,
            v => Self::Other(v),
        }
    }

    /// Whether the colour channels are sRGB-encoded (otherwise linear data).
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::R8G8B8Srgb | Self::R8G8B8A8Srgb)
    }
}

/// Tile codec (Granite's `Codec` enum). BG3 only uses [`Codec::Uniform`] and [`Codec::Bc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// The whole tile is one colour; the chunk holds 4 bytes RGBA.
    Uniform,
    /// Block-compressed tile, optionally LZ-compressed on top.
    Bc,
    Other(u32),
}

impl Codec {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Uniform,
            9 => Self::Bc,
            v => Self::Other(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub data_type: DataType,
    pub default_color: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    /// Size in tiles.
    pub width: u32,
    pub height: u32,
    /// `layers × width × height` entries, index `layer + layers * (x + y * width)`.
    pub indices: Vec<u32>,
}

/// Parameters of a BC parameter block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BcParameters {
    pub version: u16,
    /// Outer compression: `"lz77"` (FastLZ), `"lz4"` or `"raw"`.
    pub compression: String,
    /// Compressor identification, e.g. `"fastlz0.1.0"`.
    pub compressor: String,
    pub data_type: DataType,
    pub fourcc: [u8; 4],
    pub format: Option<BcFormat>,
    /// The tile payload is followed by an embedded half-resolution mip of the tile.
    pub embedded_mip: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterKind {
    Bc(BcParameters),
    Uniform {
        version: u16,
        width: u32,
        height: u32,
        data_type: DataType,
    },
    Other(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterBlock {
    pub id: u32,
    pub codec: Codec,
    pub kind: ParameterKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFileInfo {
    /// File name relative to the tile set's directory.
    pub name: String,
    pub num_pages: u32,
    /// Must match the GUID in the page file's header.
    pub guid: [u8; 16],
    pub flags: u32,
}

/// `{layer:4, level:4, y:12, x:12}` from least to most significant bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedTileId(pub u32);

impl PackedTileId {
    pub fn new(layer: u32, level: u32, x: u32, y: u32) -> Self {
        Self((layer & 0xF) | ((level & 0xF) << 4) | ((y & 0xFFF) << 8) | ((x & 0xFFF) << 20))
    }
    pub fn layer(self) -> u32 {
        self.0 & 0xF
    }
    pub fn level(self) -> u32 {
        (self.0 >> 4) & 0xF
    }
    pub fn y(self) -> u32 {
        (self.0 >> 8) & 0xFFF
    }
    pub fn x(self) -> u32 {
        self.0 >> 20
    }
}

/// Where a stored tile lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatTileInfo {
    pub page_file: u16,
    pub page: u16,
    pub chunk: u16,
    pub unknown: u16,
    pub packed_tile: u32,
}

/// A texture's rectangle in the virtual image, in level-0 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureInfo {
    /// The GTex name (`GTexFileName` in the `VirtualTextureBank`).
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Result of looking up a tile position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileRef {
    /// Stored at this level: index into [`TileSet::flat_tiles`].
    Stored(u32),
    /// Not stored at this level; this flat tile (of a coarser level) covers the area.
    Fallback(u32),
}

/// Decoded payload of one tile (borders included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TileData {
    /// `tile_width × tile_height` BCn blocks (embedded mip, if any, removed).
    Bc { format: BcFormat, data: Vec<u8> },
    /// The whole tile is this RGBA colour.
    Uniform([u8; 4]),
}

/// A parsed `.gts` tile set. Owns all of its tables; the source bytes can be dropped.
#[derive(Debug, Clone)]
pub struct TileSet {
    pub version: u32,
    pub guid: [u8; 16],
    pub layers: Vec<Layer>,
    pub levels: Vec<Level>,
    /// Tile size in pixels, including the border on each side.
    pub tile_width: u32,
    pub tile_height: u32,
    pub tile_border: u32,
    /// Size of one page inside a page file.
    pub page_size: u32,
    pub parameter_blocks: Vec<ParameterBlock>,
    pub page_files: Vec<PageFileInfo>,
    pub fourcc: Vec<FourCcNode>,
    pub textures: Vec<TextureInfo>,
    pub packed_tiles: Vec<PackedTileId>,
    pub flat_tiles: Vec<FlatTileInfo>,
    param_index: HashMap<u32, usize>,
    texture_index: HashMap<String, usize>,
}

impl TileSet {
    /// Parse a whole `.gts` file.
    // Layout: see docs/formats/virtual-textures.md. Prior art: LSLib `VirtualTileSet`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE || &data[0..4] != GTS_MAGIC {
            return Err(corrupt("not a Granite tile set (missing GRPG magic)"));
        }
        let mut h = Cursor::new(data, "GTS header");
        h.skip(4)?;
        let version = h.u32()?;
        if version != 5 {
            return Err(Error::Unsupported(format!("GTS version {version}")));
        }
        h.skip(4)?;
        let guid = h.array()?;
        let num_layers = h.u32()? as usize;
        let layers_offset = h.u64()?;
        let num_levels = h.u32()? as usize;
        let levels_offset = h.u64()?;
        let tile_width = h.i32()?;
        let tile_height = h.i32()?;
        let tile_border = h.i32()?;
        let _ = h.u32()?;
        let num_flat = h.u32()? as usize;
        let flat_offset = h.u64()?;
        h.skip(8)?;
        let num_packed = h.u32()? as usize;
        let packed_offset = h.u64()?;
        h.skip(7 * 4)?;
        let page_size = h.u32()?;
        let num_page_files = h.u32()? as usize;
        let page_files_offset = h.u64()?;
        let fourcc_size = h.u32()?;
        let fourcc_offset = h.u64()?;
        let num_params = h.u32()? as usize;
        let params_offset = h.u64()?;

        if tile_width <= 0
            || tile_height <= 0
            || tile_border < 0
            || tile_width <= 2 * tile_border
            || tile_height <= 2 * tile_border
            || num_layers > 16
            || num_levels > 16
            || page_size == 0
        {
            return Err(corrupt("implausible tile set header"));
        }

        let mut c = Cursor::at(data, layers_offset, "layers")?;
        let layers = (0..num_layers)
            .map(|_| {
                Ok(Layer {
                    data_type: DataType::from_raw(c.u32()?),
                    default_color: c.u32()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut c = Cursor::at(data, levels_offset, "levels")?;
        let mut levels = Vec::with_capacity(num_levels);
        for _ in 0..num_levels {
            let width = c.u32()?;
            let height = c.u32()?;
            let offset = c.u64()?;
            let count = (width as usize)
                .checked_mul(height as usize)
                .and_then(|n| n.checked_mul(num_layers))
                .ok_or_else(|| corrupt("level size overflow"))?;
            let indices = Cursor::at(data, offset, "level tile indices")?.u32s(count)?;
            levels.push(Level {
                width,
                height,
                indices,
            });
        }

        let mut c = Cursor::at(data, params_offset, "parameter block headers")?;
        let mut parameter_blocks = Vec::with_capacity(num_params);
        for _ in 0..num_params {
            let id = c.u32()?;
            let codec = Codec::from_raw(c.u32()?);
            let size = c.u32()?;
            let offset = c.u64()?;
            let raw = slice(data, offset, size as u64, "parameter block")?;
            parameter_blocks.push(ParameterBlock {
                id,
                codec,
                kind: parse_parameter_block(codec, raw)?,
            });
        }

        let raw = slice(
            data,
            page_files_offset,
            (num_page_files * PAGE_FILE_INFO_SIZE) as u64,
            "page file table",
        )?;
        let page_files = raw
            .as_chunks::<PAGE_FILE_INFO_SIZE>()
            .0
            .iter()
            .map(|r| {
                let units: Vec<u16> = r[..512]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .take_while(|&u| u != 0)
                    .collect();
                let mut c = Cursor::new(&r[512..], "page file info");
                Ok(PageFileInfo {
                    name: String::from_utf16_lossy(&units),
                    num_pages: c.u32()?,
                    guid: c.array()?,
                    flags: c.u32()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let fourcc = fourcc::parse(data, fourcc_offset, fourcc_size as u64)?;
        let textures = texture_catalogue(&fourcc);

        let packed_tiles = Cursor::at(data, packed_offset, "packed tile IDs")?
            .u32s(num_packed)?
            .into_iter()
            .map(PackedTileId)
            .collect();

        let mut c = Cursor::at(data, flat_offset, "flat tile infos")?;
        let flat_tiles = (0..num_flat)
            .map(|_| {
                Ok(FlatTileInfo {
                    page_file: c.u16()?,
                    page: c.u16()?,
                    chunk: c.u16()?,
                    unknown: c.u16()?,
                    packed_tile: c.u32()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let param_index = parameter_blocks
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        let texture_index = textures
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.to_ascii_lowercase(), i))
            .collect();

        Ok(Self {
            version,
            guid,
            layers,
            levels,
            tile_width: tile_width as u32,
            tile_height: tile_height as u32,
            tile_border: tile_border as u32,
            page_size,
            parameter_blocks,
            page_files,
            fourcc,
            textures,
            packed_tiles,
            flat_tiles,
            param_index,
            texture_index,
        })
    }

    /// Tile content size (border stripped).
    pub fn content_width(&self) -> u32 {
        self.tile_width - 2 * self.tile_border
    }

    pub fn content_height(&self) -> u32 {
        self.tile_height - 2 * self.tile_border
    }

    pub fn parameter_block(&self, id: u32) -> Option<&ParameterBlock> {
        self.param_index
            .get(&id)
            .map(|&i| &self.parameter_blocks[i])
    }

    /// Find a texture by GTex name (case-insensitive).
    pub fn texture(&self, name: &str) -> Option<&TextureInfo> {
        self.texture_index
            .get(&name.to_ascii_lowercase())
            .map(|&i| &self.textures[i])
    }

    /// Look up the tile at `(x, y)` (in tiles) of `layer` at `level`. `None` when out of range or
    /// when the index is invalid.
    pub fn tile(&self, level: usize, layer: usize, x: u32, y: u32) -> Option<TileRef> {
        let lv = self.levels.get(level)?;
        if layer >= self.layers.len() || x >= lv.width || y >= lv.height {
            return None;
        }
        let i = layer + self.layers.len() * (x as usize + y as usize * lv.width as usize);
        let v = *lv.indices.get(i)?;
        let flat = v & !FALLBACK_BIT;
        if flat as usize >= self.flat_tiles.len() {
            return None;
        }
        Some(if v & FALLBACK_BIT == 0 {
            TileRef::Stored(flat)
        } else {
            TileRef::Fallback(flat)
        })
    }

    /// Decode a chunk read from one of this tile set's page files.
    pub fn decode_chunk(&self, chunk: &Chunk<'_>) -> Result<TileData> {
        let block = self.parameter_block(chunk.parameter_block).ok_or_else(|| {
            corrupt(format!(
                "chunk refers to unknown parameter block {}",
                chunk.parameter_block
            ))
        })?;
        if block.codec != chunk.codec {
            return Err(corrupt("chunk codec differs from its parameter block"));
        }
        match &block.kind {
            ParameterKind::Uniform { .. } => {
                let c = chunk
                    .data
                    .get(..4)
                    .ok_or_else(|| corrupt("short uniform chunk"))?;
                Ok(TileData::Uniform([c[0], c[1], c[2], c[3]]))
            }
            ParameterKind::Bc(p) => {
                let format = p.format.ok_or_else(|| {
                    Error::Unsupported(format!(
                        "BC FourCC {:?}",
                        String::from_utf8_lossy(&p.fourcc)
                    ))
                })?;
                let main = format.image_bytes(self.tile_width, self.tile_height);
                let mip = if p.embedded_mip {
                    format.image_bytes(self.tile_width / 2, self.tile_height / 2)
                } else {
                    0
                };
                let mut data = match p.compression.as_str() {
                    "lz77" => fastlz::decompress(chunk.data, main + mip)
                        .map_err(|e| corrupt(format!("tile: {e}")))?,
                    "lz4" => lz4_flex::block::decompress(chunk.data, main + mip)
                        .map_err(|e| corrupt(format!("tile LZ4: {e}")))?,
                    "raw" => chunk.data.to_vec(),
                    other => {
                        return Err(Error::Unsupported(format!(
                            "tile compression {other:?} ({})",
                            p.compressor
                        )));
                    }
                };
                if data.len() < main {
                    return Err(corrupt(format!(
                        "tile decoded to {} bytes, expected {}",
                        data.len(),
                        main + mip
                    )));
                }
                data.truncate(main);
                Ok(TileData::Bc { format, data })
            }
            ParameterKind::Other(_) => {
                Err(Error::Unsupported(format!("tile codec {:?}", block.codec)))
            }
        }
    }
}

fn parse_parameter_block(codec: Codec, raw: &[u8]) -> Result<ParameterKind> {
    let mut c = Cursor::new(raw, "parameter block");
    Ok(match codec {
        Codec::Bc if raw.len() >= 0x38 => {
            let version = c.u16()?;
            let name = |b: &[u8]| {
                let n = b.iter().position(|&x| x == 0).unwrap_or(b.len());
                String::from_utf8_lossy(&b[..n]).into_owned()
            };
            let compression = name(c.bytes(16)?);
            let compressor = name(c.bytes(16)?);
            c.skip(4 + 3)?;
            let data_type = DataType::from_raw(c.u8()? as u32);
            c.skip(2)?;
            let fourcc: [u8; 4] = c.array()?;
            c.skip(1)?;
            let embedded_mip = c.u8()? != 0;
            ParameterKind::Bc(BcParameters {
                version,
                compression,
                compressor,
                data_type,
                fourcc,
                format: BcFormat::from_fourcc(fourcc),
                embedded_mip,
            })
        }
        Codec::Uniform if raw.len() >= 16 => {
            let version = c.u16()?;
            c.skip(2)?;
            ParameterKind::Uniform {
                version,
                width: c.u32()?,
                height: c.u32()?,
                data_type: DataType::from_raw(c.u32()?),
            }
        }
        _ => ParameterKind::Other(raw.to_vec()),
    })
}

fn texture_catalogue(root: &[FourCcNode]) -> Vec<TextureInfo> {
    let mut out = Vec::new();
    let Some(txts) = root
        .iter()
        .find(|n| &n.fourcc == b"META")
        .map(|m| m.children())
        .unwrap_or(root)
        .iter()
        .chain(root.iter())
        .find(|n| &n.fourcc == b"ATLS")
        .and_then(|a| a.child(b"TXTS"))
    else {
        return out;
    };
    for t in txts.children() {
        let u = |tag: &[u8; 4]| t.child(tag).and_then(FourCcNode::as_u32);
        if let (Some(name), Some(width), Some(height), Some(x), Some(y)) = (
            t.child(b"NAME").and_then(FourCcNode::as_str),
            u(b"WDTH"),
            u(b"HGHT"),
            u(b"XXXX"),
            u(b"YYYY"),
        ) {
            out.push(TextureInfo {
                name: name.to_owned(),
                x,
                y,
                width,
                height,
            });
        }
    }
    out
}
