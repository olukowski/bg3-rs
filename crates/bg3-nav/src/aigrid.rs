//! `Ai/aigrid.data`: the walkable-tile grid of a level (see `docs/formats/navcloud.md`).
//!
//! A level's AI grid is a set of rectangular *subgrids* of 0.5 m tiles, one per walkable
//! object (terrain patch, floor mesh, platform …). Subgrids overlap where surfaces are stacked
//! (decks, bridges); every tile carries its own height.

use crate::bytes::Reader;
use crate::{Error, Result};

/// Edge length of a tile in metres.
// bg3: eoc::AiGrid::ToWorldPos (4.1.1.7631656) — 128-tile subgrids are spaced 64 m apart.
pub const TILE_SIZE: f32 = 0.5;

/// Height units per metre: a tile height `h` is `origin.y + h * 2 / 100`.
// bg3: eoc::AiGrid::GetHeightAtTile (4.1.1.7631656)
pub const HEIGHT_SCALE: f32 = 0.02;

/// The only file version seen in the reference build.
pub const VERSION: u32 = 21;

/// Tile flag bits that have a known meaning. The rest are named by bit number in the docs.
pub mod flags {
    /// No tile here: the area/collision scans skip it.
    // bg3: eoc::AiGrid::GetStateInArea (4.1.1.7631656) — tiles with bit 0 set contribute nothing.
    pub const EMPTY: u64 = 1 << 0;
    /// Collision bit included in the default path collision mask (static geometry blocker).
    pub const BLOCK_2: u64 = 1 << 2;
    /// Collision bit included in the default path collision mask.
    pub const BLOCK_7: u64 = 1 << 7;
    /// Set when the character may not pass doors (`CanWalkThroughDoors` = false); not present in
    /// the static grid (doors add it at runtime).
    // bg3: esv::aigrid::GetAdjustedCollisionMask (4.1.1.7631656)
    pub const DOOR: u64 = 1 << 22;
    /// Collision bit included in the default path collision mask.
    pub const BLOCK_54: u64 = 1 << 54;
    /// The tile overlaps a tile of another subgrid; area scans take the slow multi-subgrid path.
    // bg3: eoc::AiGrid::GetStateInArea, eoc::AiGrid::ForEachConnection (4.1.1.7631656)
    pub const MULTIPLE_SUBGRIDS: u64 = 1 << 55;

    /// Default collision mask of a path request (`eoc::PathSettings` default, bits 2, 7, 22, 54).
    // bg3: (anonymous)::CreateUnprocessedSettings (4.1.1.7631656) — 16-byte constant whose first
    // quadword is the collision mask.
    pub const DEFAULT_COLLISION_MASK: u64 = BLOCK_2 | BLOCK_7 | DOOR | BLOCK_54;
}

/// One 16-byte tile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tile {
    /// State flags (`ls::Flags<u64>`); see [`flags`].
    pub flags: u64,
    /// Height above the subgrid origin, in units of [`HEIGHT_SCALE`] metres.
    pub height: u16,
    /// A second height in the same units, at or below `height` on most tiles (meaning unknown;
    /// possibly the bottom of the walkable surface).
    pub height2: u16,
    /// Metadata index (`0xFFFFFFFF` = none).
    pub metadata: u32,
}

impl Tile {
    pub const SIZE: usize = 16;

    pub fn is_empty(&self) -> bool {
        self.flags & flags::EMPTY != 0
    }

    fn read(b: &[u8]) -> Self {
        let u = |r: std::ops::Range<usize>| b[r].iter().rev().fold(0u64, |a, &x| a << 8 | x as u64);
        Self {
            flags: u(0..8),
            height: u(8..10) as u16,
            height2: u(10..12) as u16,
            metadata: u(12..16) as u32,
        }
    }
}

/// A rectangle of tiles.
#[derive(Debug, Clone, PartialEq)]
pub struct Subgrid {
    /// Subgrid ID (`eoc::AiSubgridId`), unique within the file.
    pub id: u32,
    pub width: u32,
    pub height: u32,
    /// World position of the tile `(0, 0)` corner; `y` is the base of the tile heights.
    pub origin: [f32; 3],
    /// GUID of the object the subgrid was generated from (terrain, scenery, platform).
    pub object: String,
    /// Second GUID (all zero in the reference levels; possibly a platform/parent ID).
    pub parent: String,
    /// `width × height` tiles, row-major (`x + y * width`); `y` runs along world `z`.
    pub tiles: Vec<Tile>,
}

/// A tile override of an AI grid layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerTile {
    /// Tile coordinates within the subgrid.
    pub x: i16,
    pub y: i16,
    /// Subgrid [`Subgrid::id`] (not index).
    pub subgrid_id: u32,
    pub flags: u64,
    /// Surface height in world metres.
    pub height: f32,
    /// Unknown (`0xFFFFFFFF` or a small number; possibly a metadata index).
    pub extra: u32,
}

/// A named set of tile overrides that the game switches on and off at runtime (for example
/// when something is destroyed or a platform moves). The all-zero GUID occurs as well.
// bg3: eoc::AiGrid::RequestActivateLayer, eoc::AiGridChangeLayersTask (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Layer GUID, as the 16 stored bytes.
    pub guid: [u8; 16],
    pub tiles: Vec<LayerTile>,
}

impl Subgrid {
    pub fn tile(&self, x: u32, y: u32) -> Option<&Tile> {
        (x < self.width && y < self.height).then(|| &self.tiles[(x + y * self.width) as usize])
    }

    /// World position of a tile centre at its height.
    // bg3: eoc::AiGrid::ToWorldPos (4.1.1.7631656)
    pub fn tile_center(&self, x: u32, y: u32) -> [f32; 3] {
        let h = self.tile(x, y).map_or(0, |t| t.height);
        [
            self.origin[0] + (x as f32 + 0.5) * TILE_SIZE,
            self.origin[1] + f32::from(h) * HEIGHT_SCALE,
            self.origin[2] + (y as f32 + 0.5) * TILE_SIZE,
        ]
    }

    /// Global tile column of tile `(0, 0)`: the grid is aligned to world multiples of
    /// [`TILE_SIZE`] (`floor(world * 2)`).
    pub fn global_origin(&self) -> (i32, i32) {
        (
            (self.origin[0] / TILE_SIZE).round() as i32,
            (self.origin[2] / TILE_SIZE).round() as i32,
        )
    }
}

/// A parsed `aigrid.data`.
#[derive(Debug, Clone, PartialEq)]
pub struct AiGridFile {
    pub version: u32,
    /// GUID of the level template the grid belongs to.
    pub level: String,
    pub subgrids: Vec<Subgrid>,
    /// Tile override layers (empty in most levels).
    pub layers: Vec<Layer>,
}

impl AiGridFile {
    // bg3: eoc::AiGrid::DoLoadAiGridFile (4.1.1.7631656)
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u32()?;
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let level = r.guid_string()?;
        let count = r.u32()?;
        let mut subgrids = Vec::with_capacity(count.min(4096) as usize);
        for _ in 0..count {
            let id = r.u32()?;
            let width = r.u32()?;
            let height = r.u32()?;
            let origin = [r.f32()?, r.f32()?, r.f32()?];
            let object = r.guid_string()?;
            let parent = r.guid_string()?;
            let n = width as usize * height as usize;
            let raw = crate::compress::read_block(&mut r, n * Tile::SIZE)?;
            if raw.len() != n * Tile::SIZE {
                return Err(Error::Size {
                    what: "subgrid tiles",
                    expected: n * Tile::SIZE,
                    got: raw.len(),
                });
            }
            let tiles = raw
                .as_chunks::<{ Tile::SIZE }>()
                .0
                .iter()
                .map(|b| Tile::read(b))
                .collect();
            subgrids.push(Subgrid {
                id,
                width,
                height,
                origin,
                object,
                parent,
                tiles,
            });
        }
        let count = r.u32()?;
        let mut layers = Vec::with_capacity(count.min(1024) as usize);
        for _ in 0..count {
            let guid: [u8; 16] = r.take(16)?.try_into().expect("16 bytes");
            let n = r.u32()?;
            let mut tiles = Vec::with_capacity(n.min(1 << 20) as usize);
            for _ in 0..n {
                let pos = r.take(4)?;
                tiles.push(LayerTile {
                    x: i16::from_le_bytes([pos[0], pos[1]]),
                    y: i16::from_le_bytes([pos[2], pos[3]]),
                    subgrid_id: r.u32()?,
                    flags: r.u64()?,
                    height: r.f32()?,
                    extra: r.u32()?,
                });
            }
            layers.push(Layer { guid, tiles });
        }
        let rest = r.rest();
        if !rest.is_empty() {
            return Err(Error::Size {
                what: "AI grid trailing bytes",
                expected: 0,
                got: rest.len(),
            });
        }
        Ok(Self {
            version,
            level,
            subgrids,
            layers,
        })
    }

    /// Serialise (used for self-authored test fixtures; tiles are stored zlib+LZ4 like the game).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(self.version.to_le_bytes());
        push_guid(&mut out, &self.level);
        out.extend((self.subgrids.len() as u32).to_le_bytes());
        for s in &self.subgrids {
            out.extend(s.id.to_le_bytes());
            out.extend(s.width.to_le_bytes());
            out.extend(s.height.to_le_bytes());
            for v in s.origin {
                out.extend(v.to_le_bytes());
            }
            push_guid(&mut out, &s.object);
            push_guid(&mut out, &s.parent);
            let mut raw = Vec::with_capacity(s.tiles.len() * Tile::SIZE);
            for t in &s.tiles {
                raw.extend(t.flags.to_le_bytes());
                raw.extend(t.height.to_le_bytes());
                raw.extend(t.height2.to_le_bytes());
                raw.extend(t.metadata.to_le_bytes());
            }
            crate::compress::write_block(&mut out, &raw);
        }
        out.extend((self.layers.len() as u32).to_le_bytes());
        for l in &self.layers {
            out.extend(l.guid);
            out.extend((l.tiles.len() as u32).to_le_bytes());
            for t in &l.tiles {
                out.extend(t.x.to_le_bytes());
                out.extend(t.y.to_le_bytes());
                out.extend(t.subgrid_id.to_le_bytes());
                out.extend(t.flags.to_le_bytes());
                out.extend(t.height.to_le_bytes());
                out.extend(t.extra.to_le_bytes());
            }
        }
        out
    }
}

fn push_guid(out: &mut Vec<u8>, s: &str) {
    let mut b = [0u8; 37];
    let n = s.len().min(36);
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
    out.extend(b);
}
