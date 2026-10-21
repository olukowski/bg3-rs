//! Terrain: heightfield objects from `Mods/<Module>/Levels/<Level>/Terrains/`.
//!
//! See `docs/research/terrain.md`. A level's `Terrains/_merged.lsf` lists terrain objects
//! (`Type=terrain`) with a transform, a size in cells (`Visual/Width`, `Visual/Height`) and the
//! terrain brushes used as texture layers. The heights live next to it in one `.patch` file per
//! 64×64-cell tile: `<MapKey>_<px>_<py>.patch`. The `<MapKey>_<n>.bin` files are physics
//! heightfields (not read here).
//!
//! The grid spacing is one world unit; vertex `(x, z)` of a terrain is at local position
//! `(x, height, z)`, then the object transform applies.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::level::read_transform;
use crate::{Result, Transform, parse, str_attr};

/// Cells per patch side.
// bg3: ls::TerrainPatchFileReader::ReadHeader / ls::TerrainPatch::UpdateBound (4.1.1.7631656)
pub const PATCH_CELLS: usize = 64;

const MAGIC: &[u8; 8] = b"PVersion";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PatchError {
    #[error("not a terrain patch (bad magic)")]
    BadMagic,
    #[error("unsupported patch version {0}")]
    Version(u32),
    #[error("unexpected header size {0}")]
    HeaderSize(u32),
    #[error("truncated at offset {0}")]
    Truncated(usize),
    #[error("bad header: {0}")]
    Header(&'static str),
    #[error("bad layer name")]
    LayerName,
}

/// One `.patch` file: a tile of up to 64×64 cells of a terrain.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainPatch {
    pub version: u32,
    /// Vertices in this patch (`cells + 1` per axis).
    pub vertices: [usize; 2],
    pub cells: [usize; 2],
    /// Vertices of the whole terrain.
    pub terrain_vertices: [usize; 2],
    /// Tile index; this patch starts at cell `index * 64`.
    pub index: [usize; 2],
    /// Row-major heights (`vertices[0]` per row; rows run along local +Z).
    pub heights: Vec<f32>,
    /// Hole cells, as indices into the whole terrain's grid with a row stride of
    /// `terrain_vertices[0]` (verified against all patches in the reference build).
    pub holes: Vec<u32>,
    /// Texture layers: brush ID and one weight byte per cell of this patch (row-major).
    pub layers: Vec<PatchLayer>,
    /// 16 bytes per 4×4 vertex block (version ≥ 7). Meaning unknown.
    pub blocks: Vec<u8>,
    /// Anything after the block data (present when there are more than 4 layers). Not decoded.
    pub extra: Vec<u8>,
    /// Two `IVec4`s in the version-8 header (often all −16384). Meaning unknown.
    pub header_extra: [i32; 8],
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatchLayer {
    pub brush: String,
    pub weights: Vec<u8>,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> std::result::Result<&'a [u8], PatchError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or(PatchError::Truncated(self.pos))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u32(&mut self) -> std::result::Result<u32, PatchError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> std::result::Result<i32, PatchError> {
        Ok(self.u32()? as i32)
    }
    fn len(&mut self, max: usize) -> std::result::Result<usize, PatchError> {
        let at = self.pos;
        let n = self.u32()? as usize;
        if n > max {
            return Err(PatchError::Truncated(at));
        }
        Ok(n)
    }
}

impl TerrainPatch {
    /// Parse a `.patch` file (versions 7 and 8; version 5 files in the install are stale leftovers
    /// not referenced by any level).
    // bg3: ls::TerrainPatchFileReader::ReadHeader, ls::TerrainPatchFileReader::Read (4.1.1.7631656)
    pub fn parse(data: &[u8]) -> std::result::Result<Self, PatchError> {
        let mut r = Reader { data, pos: 0 };
        if r.bytes(8)? != MAGIC {
            return Err(PatchError::BadMagic);
        }
        let version = r.u32()?;
        if !(7..=8).contains(&version) {
            return Err(PatchError::Version(version));
        }
        let header_size = r.u32()?;
        let expected = if version >= 8 { 72 } else { 40 };
        if header_size != expected {
            return Err(PatchError::HeaderSize(header_size));
        }
        let mut h = [0i32; 10];
        for v in &mut h {
            *v = r.i32()?;
        }
        let mut header_extra = [0i32; 8];
        if version >= 8 {
            for v in &mut header_extra {
                *v = r.i32()?;
            }
        }
        let dim = |v: i32| usize::try_from(v).map_err(|_| PatchError::Header("negative size"));
        let vertices = [dim(h[0])?, dim(h[1])?];
        let cells = [dim(h[2])?, dim(h[3])?];
        let terrain_vertices = [dim(h[4])?, dim(h[5])?];
        let index = [dim(h[6])?, dim(h[7])?];
        let hole_count = dim(h[8])?;
        let layer_count = dim(h[9])?;
        if vertices[0] != cells[0] + 1 || vertices[1] != cells[1] + 1 {
            return Err(PatchError::Header("vertices != cells + 1"));
        }
        if cells[0] > PATCH_CELLS || cells[1] > PATCH_CELLS {
            return Err(PatchError::Header("patch larger than 64 cells"));
        }

        let n = vertices[0] * vertices[1];
        let heights = r
            .bytes(n * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect();
        let holes = r
            .bytes(
                hole_count
                    .checked_mul(4)
                    .ok_or(PatchError::Truncated(r.pos))?,
            )?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| u32::from_le_bytes(b))
            .collect();
        let mut layers = Vec::with_capacity(layer_count.min(64));
        for _ in 0..layer_count {
            let len = r.len(256)?;
            let brush = std::str::from_utf8(r.bytes(len)?)
                .map_err(|_| PatchError::LayerName)?
                .to_owned();
            let len = r.len(data.len())?;
            let weights = r.bytes(len)?.to_vec();
            layers.push(PatchLayer { brush, weights });
        }
        let len = r.len(data.len())?;
        let blocks = r.bytes(len)?.to_vec();
        let extra = data[r.pos..].to_vec();
        Ok(Self {
            version,
            vertices,
            cells,
            terrain_vertices,
            index,
            heights,
            holes,
            layers,
            blocks,
            extra,
            header_extra,
        })
    }

    pub fn height(&self, x: usize, z: usize) -> f32 {
        self.heights[z * self.vertices[0] + x]
    }
}

/// A terrain brush layer used by a terrain (`TerrainLayers/TerrainLayer`).
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainLayer {
    /// `TerrainBrushBank` resource ID.
    pub brush: String,
}

/// A terrain object from `Terrains/_merged.lsf`, plus its assembled heightfield.
#[derive(Debug, Clone)]
pub struct Terrain {
    pub id: String,
    pub name: String,
    pub template: Option<String>,
    pub transform: Transform,
    /// Cells per axis (`Visual/Width`, `Visual/Height`).
    pub cells: [usize; 2],
    /// Brush IDs, in `TerrainLayers` order.
    pub layers: Vec<TerrainLayer>,
    /// Row-major heights of the whole terrain, `(cells[0] + 1) × (cells[1] + 1)`.
    pub heights: Vec<f32>,
    /// One bit per cell (row stride `cells[0]`), set where the terrain has a hole.
    pub holes: Vec<bool>,
    /// Per-cell weight per entry of `layers` (row stride `cells[0]`).
    pub weights: Vec<Vec<u8>>,
    /// Patches that were found and parsed.
    pub patches: usize,
    /// Patch files that were missing or failed to parse (path, error).
    pub errors: Vec<(String, String)>,
    pub module: String,
}

impl Terrain {
    pub fn vertices(&self) -> [usize; 2] {
        [self.cells[0] + 1, self.cells[1] + 1]
    }

    pub fn height(&self, x: usize, z: usize) -> f32 {
        self.heights[z * (self.cells[0] + 1) + x]
    }

    pub fn is_hole(&self, x: usize, z: usize) -> bool {
        self.holes[z * self.cells[0] + x]
    }

    /// Minimum and maximum height (local space), if the terrain has any vertices.
    pub fn height_range(&self) -> Option<(f32, f32)> {
        let mut it = self.heights.iter().copied();
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), h| (lo.min(h), hi.max(h))))
    }

    /// Build the heightfield from the patches.
    pub fn assemble(&mut self, patches: &[TerrainPatch]) {
        let [cw, ch] = self.cells;
        let vw = cw + 1;
        self.heights = vec![0.0; vw * (ch + 1)];
        self.holes = vec![false; cw * ch];
        self.weights = vec![vec![0; cw * ch]; self.layers.len()];
        for p in patches {
            let (ox, oz) = (p.index[0] * PATCH_CELLS, p.index[1] * PATCH_CELLS);
            for z in 0..p.vertices[1] {
                for x in 0..p.vertices[0] {
                    let (gx, gz) = (ox + x, oz + z);
                    if gx < vw && gz <= ch {
                        self.heights[gz * vw + gx] = p.height(x, z);
                    }
                }
            }
            let stride = p.terrain_vertices[0].max(1);
            for &i in &p.holes {
                let (x, z) = (i as usize % stride, i as usize / stride);
                if x < cw && z < ch {
                    self.holes[z * cw + x] = true;
                }
            }
            for l in &p.layers {
                let Some(slot) = self.layers.iter().position(|t| t.brush == l.brush) else {
                    continue;
                };
                if l.weights.len() != p.cells[0] * p.cells[1] {
                    continue;
                }
                for z in 0..p.cells[1] {
                    for x in 0..p.cells[0] {
                        let (gx, gz) = (ox + x, oz + z);
                        if gx < cw && gz < ch {
                            self.weights[slot][gz * cw + gx] = l.weights[z * p.cells[0] + x];
                        }
                    }
                }
            }
        }
        self.patches = patches.len();
    }
}

/// Read terrain objects from a `Terrains/_merged.lsf` resource (no heights yet).
pub fn read_terrain_objects(res: &bg3_lsf::Resource, module: &str) -> Vec<Terrain> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children.iter().filter(|c| c.name == "GameObjects"))
        .filter_map(|n| read_terrain_object(n, module))
        .collect()
}

fn read_terrain_object(n: &Node, module: &str) -> Option<Terrain> {
    let visual = n.child("Visual")?;
    let size = |name: &str| {
        visual
            .attr(name)
            .and_then(|v| v.as_i64())
            .and_then(|v| usize::try_from(v).ok())
    };
    let layers = n
        .child("TerrainLayers")
        .into_iter()
        .flat_map(|l| l.children_named("TerrainLayer"))
        .filter_map(|l| {
            Some(TerrainLayer {
                brush: str_attr(l, "TerrainBrushResource")?,
            })
        })
        .collect();
    Some(Terrain {
        id: str_attr(n, "MapKey")?,
        name: str_attr(n, "Name").unwrap_or_default(),
        template: str_attr(n, "TemplateName"),
        transform: n.child("Transform").map(read_transform).unwrap_or_default(),
        cells: [size("Width")?, size("Height")?],
        layers,
        heights: Vec::new(),
        holes: Vec::new(),
        weights: Vec::new(),
        patches: 0,
        errors: Vec::new(),
        module: module.to_owned(),
    })
}

/// All terrains of a level, merged across modules (a later module replaces a terrain by `MapKey`).
pub fn load_level_terrains(vfs: &Vfs, level: &str) -> Result<Vec<Terrain>> {
    load_level_terrains_modules(vfs, level, BASE_LOAD_ORDER)
}

pub fn load_level_terrains_modules(
    vfs: &Vfs,
    level: &str,
    modules: &[&str],
) -> Result<Vec<Terrain>> {
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Terrain> = Vec::new();
    for module in modules {
        let path = format!("Mods/{module}/Levels/{level}/Terrains/_merged.lsf");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        for t in read_terrain_objects(&res, module) {
            match by_id.get(&t.id) {
                Some(&i) => out[i] = t,
                None => {
                    by_id.insert(t.id.clone(), out.len());
                    out.push(t);
                }
            }
        }
    }
    for t in &mut out {
        let dir = format!("Mods/{}/Levels/{level}/Terrains", t.module);
        let tiles = [
            t.cells[0].div_ceil(PATCH_CELLS),
            t.cells[1].div_ceil(PATCH_CELLS),
        ];
        let mut patches = Vec::new();
        for py in 0..tiles[1] {
            for px in 0..tiles[0] {
                let path = format!("{dir}/{}_{px}_{py}.patch", t.id);
                match vfs.read(&path) {
                    Ok(data) => match TerrainPatch::parse(&data) {
                        Ok(p) => patches.push(p),
                        Err(e) => t.errors.push((path, e.to_string())),
                    },
                    Err(e) => t.errors.push((path, e.to_string())),
                }
            }
        }
        t.assemble(&patches);
    }
    Ok(out)
}

/// A `TerrainBrushBank` resource: the material of one terrain texture layer.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainBrush {
    pub id: String,
    pub name: String,
    /// `TextureBank` IDs.
    pub base_color_map: Option<String>,
    pub normal_map: Option<String>,
    pub physical_map: Option<String>,
    /// UV tiling (texture repeats per world unit).
    pub tiling: f32,
    pub material_type: u8,
    pub is_black_brush: bool,
    /// `GradientMidHeightColor`, used by brushes with `UseHeightGradient`.
    pub gradient_mid_color: Option<[f32; 3]>,
    pub use_height_gradient: bool,
}

/// Terrain brushes from every bank file under `Public/<Module>/Content/` that has a
/// `TerrainBrushBank` region (in the reference build: `Assets/TerrainTextures`).
pub fn load_terrain_brushes(vfs: &Vfs) -> Result<HashMap<String, TerrainBrush>> {
    let mut out = HashMap::new();
    for module in BASE_LOAD_ORDER {
        let prefix = format!("Public/{module}/Content/");
        let mut paths: Vec<String> = vfs
            .files_under(&prefix)
            .map(|f| f.entry.name.clone())
            .filter(|n| n.contains("TerrainTextures") && n.ends_with("_merged.lsf"))
            .collect();
        paths.sort();
        for path in paths {
            let res = parse(vfs, &path)?;
            for b in read_terrain_brushes(&res) {
                out.insert(b.id.clone(), b);
            }
        }
    }
    Ok(out)
}

pub fn read_terrain_brushes(res: &bg3_lsf::Resource) -> Vec<TerrainBrush> {
    let f = |n: &Node, name: &str| n.attr(name).and_then(|v| v.as_f64());
    let b = |n: &Node, name: &str| n.attr(name).and_then(|v| v.as_bool()).unwrap_or(false);
    res.region("TerrainBrushBank")
        .into_iter()
        .flat_map(|n| n.children_named("Resource"))
        .filter_map(|n| {
            Some(TerrainBrush {
                id: str_attr(n, "ID")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                base_color_map: str_attr(n, "BaseColorMap"),
                normal_map: str_attr(n, "NormalMap"),
                physical_map: str_attr(n, "PhysicalMap"),
                tiling: f(n, "Tiling").unwrap_or(1.0) as f32,
                material_type: n.attr("MaterialType").and_then(|v| v.as_i64()).unwrap_or(0) as u8,
                is_black_brush: b(n, "IsBlackBrush"),
                gradient_mid_color: n
                    .attr("GradientMidHeightColor")
                    .and_then(|v| v.components())
                    .filter(|c| c.len() == 3)
                    .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]),
                use_height_gradient: b(n, "UseHeightGradient"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic version-8 patch.
    fn patch_bytes(
        vertices: [i32; 2],
        terrain_vertices: [i32; 2],
        index: [i32; 2],
        heights: &[f32],
        holes: &[u32],
        layers: &[(&str, Vec<u8>)],
        extra: &[u8],
    ) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(MAGIC);
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&72u32.to_le_bytes());
        let cells = [vertices[0] - 1, vertices[1] - 1];
        for v in [
            vertices[0],
            vertices[1],
            cells[0],
            cells[1],
            terrain_vertices[0],
            terrain_vertices[1],
            index[0],
            index[1],
            holes.len() as i32,
            layers.len() as i32,
        ] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..8 {
            d.extend_from_slice(&(-16384i32).to_le_bytes());
        }
        for h in heights {
            d.extend_from_slice(&h.to_le_bytes());
        }
        for h in holes {
            d.extend_from_slice(&h.to_le_bytes());
        }
        for (name, w) in layers {
            d.extend_from_slice(&(name.len() as u32).to_le_bytes());
            d.extend_from_slice(name.as_bytes());
            d.extend_from_slice(&(w.len() as u32).to_le_bytes());
            d.extend_from_slice(w);
        }
        let blocks = (vertices[0] as usize).div_ceil(4) * (vertices[1] as usize).div_ceil(4) * 16;
        d.extend_from_slice(&(blocks as u32).to_le_bytes());
        d.extend(std::iter::repeat_n(0xAB, blocks));
        d.extend_from_slice(extra);
        d
    }

    #[test]
    fn parses_synthetic_patch() {
        let heights: Vec<f32> = (0..12).map(|i| i as f32 * 0.5).collect();
        let data = patch_bytes(
            [4, 3],
            [4, 3],
            [0, 0],
            &heights,
            &[5],
            &[("brush-a", vec![255; 6]), ("brush-b", vec![0; 6])],
            &[1, 2, 3],
        );
        let p = TerrainPatch::parse(&data).unwrap();
        assert_eq!(p.version, 8);
        assert_eq!(p.vertices, [4, 3]);
        assert_eq!(p.cells, [3, 2]);
        assert_eq!(p.height(1, 2), 4.5);
        assert_eq!(p.holes, vec![5]);
        assert_eq!(p.layers.len(), 2);
        assert_eq!(p.layers[1].brush, "brush-b");
        assert_eq!(p.blocks.len(), 16);
        assert_eq!(p.extra, vec![1, 2, 3]);
        assert_eq!(p.header_extra, [-16384; 8]);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            TerrainPatch::parse(b"NotAPatch___"),
            Err(PatchError::BadMagic)
        );
        let mut v5 = MAGIC.to_vec();
        v5.extend_from_slice(&5u32.to_le_bytes());
        assert_eq!(TerrainPatch::parse(&v5), Err(PatchError::Version(5)));
        let data = patch_bytes([2, 2], [2, 2], [0, 0], &[0.0; 4], &[], &[], &[]);
        assert!(matches!(
            TerrainPatch::parse(&data[..data.len() - 5]),
            Err(PatchError::Truncated(_))
        ));
    }

    #[test]
    fn assembles_patches_into_terrain() {
        // A 66×2-cell terrain: patch (0,0) covers cells 0..64, patch (1,0) cells 64..66.
        let tv = [67, 3];
        let a_heights: Vec<f32> = (0..65 * 3).map(|i| (i % 65) as f32).collect();
        let b_heights: Vec<f32> = (0..3 * 3).map(|i| 64.0 + (i % 3) as f32).collect();
        // Hole at global cell (65, 1): index 1 * 67 + 65.
        let a = patch_bytes(
            [65, 3],
            tv,
            [0, 0],
            &a_heights,
            &[],
            &[("g", vec![7; 128])],
            &[],
        );
        let b = patch_bytes(
            [3, 3],
            tv,
            [1, 0],
            &b_heights,
            &[67 + 65],
            &[("g", vec![9; 4])],
            &[],
        );
        let mut t = Terrain {
            id: "t".into(),
            name: String::new(),
            template: None,
            transform: Transform::default(),
            cells: [66, 2],
            layers: vec![TerrainLayer { brush: "g".into() }],
            heights: vec![],
            holes: vec![],
            weights: vec![],
            patches: 0,
            errors: vec![],
            module: "M".into(),
        };
        t.assemble(&[
            TerrainPatch::parse(&a).unwrap(),
            TerrainPatch::parse(&b).unwrap(),
        ]);
        assert_eq!(t.vertices(), [67, 3]);
        assert_eq!(t.height(10, 2), 10.0);
        assert_eq!(t.height(66, 1), 66.0);
        assert!(t.is_hole(65, 1));
        assert!(!t.is_hole(64, 1));
        assert_eq!(t.weights[0][0], 7);
        assert_eq!(t.weights[0][66 + 65], 9);
        assert_eq!(t.height_range(), Some((0.0, 66.0)));
    }

    fn install_vfs() -> Option<Vfs> {
        match bg3_install::Install::locate() {
            Ok(install) => Some(Vfs::from_install(&install).unwrap()),
            Err(e) => {
                eprintln!("skipping: {e}");
                None
            }
        }
    }

    fn check_level(vfs: &Vfs, level: &str, min_terrains: usize) -> Vec<Terrain> {
        let terrains = load_level_terrains(vfs, level).unwrap();
        let brushes = load_terrain_brushes(vfs).unwrap();
        assert!(
            terrains.len() >= min_terrains,
            "{level}: {}",
            terrains.len()
        );
        for t in &terrains {
            let (lo, hi) = t.height_range().unwrap();
            let holes = t.holes.iter().filter(|&&h| h).count();
            eprintln!(
                "{level} {} {:?} cells {:?} at {:?}: {} patches, heights {lo:.2}..{hi:.2}, {holes} holes, layers {:?}",
                t.name,
                t.id,
                t.cells,
                t.transform.position,
                t.patches,
                t.layers
                    .iter()
                    .map(|l| brushes.get(&l.brush).map_or("?", |b| b.name.as_str()))
                    .collect::<Vec<_>>()
            );
            assert!(t.errors.is_empty(), "{:?}", t.errors);
            assert_eq!(
                t.patches,
                t.cells[0].div_ceil(PATCH_CELLS) * t.cells[1].div_ceil(PATCH_CELLS)
            );
            assert!(lo > -500.0 && hi < 500.0, "implausible heights {lo}..{hi}");
            for l in &t.layers {
                assert!(brushes.contains_key(&l.brush), "unknown brush {}", l.brush);
            }
        }
        terrains
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn level_terrains_load() {
        let Some(vfs) = install_vfs() else { return };
        let naut = check_level(&vfs, "TUT_Avernus_C", 10);
        // Some Nautiloid terrains are sculpted (non-flat) and have holes.
        assert!(naut.iter().any(|t| t.height_range().unwrap().1 > 5.0));
        assert!(naut.iter().any(|t| t.holes.iter().any(|&h| h)));
        check_level(&vfs, "WLD_Main_A", 2);
        let crash = check_level(&vfs, "WLD_Crashsite_D", 5);
        assert!(crash.iter().any(|t| t.cells[0] * t.cells[1] > 40_000));
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn every_referenced_patch_parses() {
        let Some(vfs) = install_vfs() else { return };
        let mut levels: Vec<String> = vfs
            .files_under("Mods/")
            .map(|f| f.entry.name.clone())
            .filter(|n| n.ends_with("/Terrains/_merged.lsf"))
            .filter_map(|n| n.split('/').nth(3).map(str::to_owned))
            .collect();
        levels.sort();
        levels.dedup();
        let (mut terrains, mut patches, mut errors) = (0, 0, Vec::new());
        for level in &levels {
            for t in load_level_terrains(&vfs, level).unwrap() {
                terrains += 1;
                patches += t.patches;
                errors.extend(t.errors);
            }
        }
        eprintln!(
            "{} levels, {terrains} terrains, {patches} patches, {} errors",
            levels.len(),
            errors.len()
        );
        // The reference build ships one terrain (EPI_CIN_VlaakithThroneRoom_A) without its patch;
        // tolerate missing files, but every patch that exists must parse.
        let parse_errors: Vec<_> = errors
            .iter()
            .filter(|(_, e)| !e.contains("not found"))
            .collect();
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        assert!(errors.len() <= 1, "{errors:?}");
    }

    #[test]
    fn reads_terrain_object_from_lsf_tree() {
        use bg3_lsf::Value;
        let obj = Node::new("GameObjects")
            .with_attr("MapKey", Value::FixedString("abc".into()))
            .with_attr("Name", Value::LSString("T".into()))
            .with_attr("Type", Value::FixedString("terrain".into()))
            .with_child(
                Node::new("TerrainLayers").with_child(
                    Node::new("TerrainLayer")
                        .with_attr("TerrainBrushResource", Value::FixedString("b1".into())),
                ),
            )
            .with_child(Node::new("Transform").with_attr("Position", Value::Vec3([1.0, 2.0, 3.0])))
            .with_child(
                Node::new("Visual")
                    .with_attr("Width", Value::Int(39))
                    .with_attr("Height", Value::Int(40)),
            );
        let t = read_terrain_object(&obj, "Gustav").unwrap();
        assert_eq!(t.id, "abc");
        assert_eq!(t.cells, [39, 40]);
        assert_eq!(t.layers[0].brush, "b1");
        assert_eq!(t.transform.position, [1.0, 2.0, 3.0]);
    }
}
