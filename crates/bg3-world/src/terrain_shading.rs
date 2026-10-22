//! Terrain shading inputs, as the engine prepares them for the terrain pixel shaders
//! (`Terrain_PS<n>` in `Core.metallib`), and a CPU model of the shader's layer blend.
//!
//! See `docs/research/terrain.md` ("Drawing"). Per terrain the engine uploads:
//!
//! - one [`LayerParams`] per layer (`SB_TerrainLayerParams`), from the brush and the layer's
//!   `ParamOverrides` ([`LayerParams::new`]);
//! - the layer weights, one RGBA texel per cell and per four layers ([`weight_slices`]);
//! - per-vertex normals in terrain space ([`vertex_normals`]);
//! - the base layers' slope and height curves baked into a 4-row ramp ([`base_ramp`]);
//! - `CB_Terrain`: the height range, the base layer indices ([`base_layer_indices`]) and the
//!   height transition.
//!
//! The brushes' maps live in texture arrays shared by all terrains ([`TextureArrays`]).

use std::collections::HashMap;

use bg3_material::convert::{self, TexelFormat};
use bg3_material::dds::DdsFormat;
use bg3_vfs::Vfs;

use crate::ResourceBanks;
use crate::terrain::{Terrain, TerrainBrush, TerrainLayer};

/// Layers a terrain can draw (`SB_TerrainLayerParams` holds 64 entries of 112 bytes).
// bg3: ls::Terrain::UpdateTerrainBrushData (4.1.1.7631656)
pub const MAX_LAYERS: usize = 64;
/// A base layer index above 63 means "no base layer": the shader draws a grey checker
/// (0.1 / 0.2, 2 m squares) with the left-over weight instead.
pub const NO_BASE_LAYER: u32 = 64;

/// `TerrainLayerParam::Flags`.
pub const FLAG_DETAIL_NORMAL: u32 = 1;
pub const FLAG_HEIGHT_GRADIENT: u32 = 2;
pub const FLAG_BLACK: u32 = 4;

/// One layer's shader parameters (`TerrainLayerParam`, 112 bytes in the game).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerParams {
    /// Texture array slices of the base colour (alpha = height), normal, physical and detail
    /// normal maps; −1 where the brush has none.
    pub textures: [f32; 4],
    pub gradient_bottom_color: [f32; 4],
    pub gradient_mid_color: [f32; 4],
    pub height_scale: f32,
    pub height_offset: f32,
    /// The exponent applied to the height: 1 / the brush's `HeightContrast`.
    pub height_contrast: f32,
    pub alpha_threshold: f32,
    pub tiling: f32,
    pub detail_normal_tiling: f32,
    pub detail_normal_strength: f32,
    pub gradient_bottom_height: f32,
    pub gradient_mid_point: f32,
    pub gradient_top_height: f32,
    pub gradient_roughness_reach: f32,
    pub gradient_roughness_power: f32,
    pub flags: u32,
    pub material_type: u32,
}

impl LayerParams {
    /// The parameters of `layer` drawn with `brush`, its maps at `textures`.
    ///
    /// `ParamOverridesMask` bits select the override: 1, 2, 4, 8 take `ParamOverrides1`'s
    /// x–w for `HeightScale`, `HeightOffset`, `HeightContrast`, `AlphaThreshold`; 16, 32, 64
    /// take `ParamOverrides2`'s x, z, w for `Tiling`, `DetailNormalTiling`,
    /// `DetailNormalScale` (128 selects `ParamOverrides2.y`, which feeds nothing).
    // bg3: ls::Terrain::UpdateTerrainBrushData, ls::TerrainBrushSystem::RequestDeferred
    // (brush instance layout) (4.1.1.7631656)
    pub fn new(brush: &TerrainBrush, layer: &TerrainLayer, textures: [f32; 4]) -> Self {
        let pick = |bit: u32, over: f32, own: f32| {
            if layer.override_mask & bit != 0 {
                over
            } else {
                own
            }
        };
        let [o1, o2] = layer.overrides;
        let mut flags = 0;
        if brush.use_detail_normal_map {
            flags |= FLAG_DETAIL_NORMAL;
        }
        if brush.use_height_gradient {
            flags |= FLAG_HEIGHT_GRADIENT;
        }
        if brush.is_black_brush {
            flags |= FLAG_BLACK;
        }
        // The engine clamps the gradient colours to 0–1 and sets alpha 1.
        let rgb1 = |c: [f32; 3]| {
            let c = c.map(|v| v.clamp(0.0, 1.0));
            [c[0], c[1], c[2], 1.0]
        };
        Self {
            textures,
            gradient_bottom_color: rgb1(brush.gradient_bottom_color),
            gradient_mid_color: rgb1(brush.gradient_mid_color),
            // **Assumption**: every layer is enabled. The engine zeroes `HeightScale` for
            // layers missing from a per-terrain bit set (which turns them off); what clears
            // those bits wasn't traced.
            height_scale: pick(1, o1[0], brush.height_scale),
            height_offset: pick(2, o1[1], brush.height_offset),
            height_contrast: 1.0 / pick(4, o1[2], brush.height_contrast),
            alpha_threshold: pick(8, o1[3], brush.alpha_threshold),
            tiling: pick(16, o2[0], brush.tiling),
            detail_normal_tiling: pick(32, o2[2], brush.detail_normal_tiling),
            detail_normal_strength: pick(64, o2[3], brush.detail_normal_scale),
            gradient_bottom_height: brush.gradient_height_bottom,
            gradient_mid_point: brush.gradient_height_mid_point,
            gradient_top_height: brush.gradient_height_top,
            gradient_roughness_reach: brush.gradient_roughness_reach,
            gradient_roughness_power: brush.gradient_roughness_power,
            flags,
            material_type: brush.material_type as u32,
        }
    }
}

/// `Terrain_BaseLayerIndices`: each base layer's index among the terrain's layers (the base
/// brush is always one of them in the reference build), [`NO_BASE_LAYER`] where there is none.
// bg3: ls::Terrain::InitRenderResources (4.1.1.7631656)
pub fn base_layer_indices(t: &Terrain) -> [u32; 2] {
    std::array::from_fn(|i| {
        t.base_layers
            .get(i)
            .and_then(|b| t.layers.iter().position(|l| l.brush == b.brush))
            .filter(|&p| p < MAX_LAYERS)
            .map_or(NO_BASE_LAYER, |p| p as u32)
    })
}

/// `Terrain_ScaleOffset`: the heightmap stores heights normalised to the terrain's range;
/// `local height = normalised · scale + offset`.
pub fn height_scale_offset(t: &Terrain) -> [f32; 2] {
    let (lo, hi) = t.height_range().unwrap_or((0.0, 0.0));
    [hi - lo, lo]
}

/// The base layer ramp (`Terrain_BaseLayerRamp`): `width` texels per row; rows 0 and 1 are the
/// slope curves of base layers 0 and 1 over `u = cos(slope)`, rows 2 and 3 their height curves
/// over the terrain's height range (normalised by the largest height-curve key, at least 1).
/// Values are 16-bit unorm.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseRamp {
    pub width: usize,
    pub rows: [Vec<u16>; 4],
}

/// Bake the base layer ramp.
// bg3: ls::Terrain::InitRenderResources (4.1.1.7631656)
pub fn base_ramp(t: &Terrain) -> BaseRamp {
    let width = t.curves_resolution.max(1) as usize;
    let (lo, hi) = t.height_range().unwrap_or((0.0, 0.0));
    let range = if hi != lo { hi - lo } else { 0.0 };
    let peak = t
        .base_layers
        .iter()
        .flat_map(|b| b.height_curve.keys.iter().map(|k| k.position[1]))
        .fold(0.0f32, f32::max)
        .max(1.0);
    let unorm = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
    let mut rows: [Vec<u16>; 4] = Default::default();
    for i in 0..width {
        // Texel i: slope acos(i / (width − 1)) in degrees (90° at the left edge, 0° at the
        // right), height lo + range · i / (width − 1).
        let (t01, slope) = if i == 0 {
            (if width == 1 { 1.0 } else { 0.0 }, 90.0)
        } else if i == width - 1 {
            (1.0, 0.0)
        } else {
            let t01 = i as f32 / (width - 1) as f32;
            (t01, t01.acos() * 57.29578)
        };
        let height = lo + t01 * range;
        for (b, layer) in t.base_layers.iter().take(2).enumerate() {
            rows[b].push(unorm(layer.slope_curve.value_at(slope)));
            rows[2 + b].push(unorm(layer.height_curve.value_at(height) / peak));
        }
        for row in &mut rows {
            if row.len() <= i {
                row.push(0);
            }
        }
    }
    BaseRamp { width, rows }
}

/// Layer weights as RGBA8 texture array slices (four layers per slice, one texel per cell,
/// rows along local +Z), slice-major.
pub fn weight_slices(t: &Terrain) -> Vec<u8> {
    let [cw, ch] = t.cells;
    let slices = t.layers.len().div_ceil(4).max(1);
    let mut out = vec![0u8; slices * cw * ch * 4];
    for (l, w) in t.weights.iter().enumerate() {
        let base = (l / 4) * cw * ch * 4 + l % 4;
        for (cell, &v) in w.iter().enumerate() {
            out[base + cell * 4] = v;
        }
    }
    out
}

/// Terrain-space vertex normals (`Terrain_LocalNormalMap`), row-major `(cells+1)²`.
/// **Assumption**: central differences of the heights (one-sided at the edges); the engine
/// builds this texture itself and its filter wasn't traced.
pub fn vertex_normals(t: &Terrain) -> Vec<[f32; 3]> {
    let [cw, ch] = t.cells;
    let mut out = Vec::with_capacity((cw + 1) * (ch + 1));
    for z in 0..=ch {
        for x in 0..=cw {
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(cw));
            let (z0, z1) = (z.saturating_sub(1), (z + 1).min(ch));
            let dx = if x1 > x0 {
                (t.height(x1, z) - t.height(x0, z)) / (x1 - x0) as f32
            } else {
                0.0
            };
            let dz = if z1 > z0 {
                (t.height(x, z1) - t.height(x, z0)) / (z1 - z0) as f32
            } else {
                0.0
            };
            let len = (dx * dx + 1.0 + dz * dz).sqrt();
            out.push([-dx / len, 1.0 / len, -dz / len]);
        }
    }
    out
}

/// A layer's height value at a pixel: `HeightScale · w · |a|^contrast + HeightOffset` where the
/// weight `w` and the base colour alpha `a` pass the threshold (`w + a − 1 > AlphaThreshold`),
/// −1 where the layer is absent. A layer without a base colour map has `a = 0.5`.
// bg3: Terrain_PS4 (Core.metallib, 4.1.1.7631656)
pub fn height_value(p: &LayerParams, weight: f32, alpha: f32) -> f32 {
    if weight <= 1e-3 {
        return -1.0;
    }
    let v = if weight + alpha - 1.0 > p.alpha_threshold {
        weight * alpha.abs().powf(p.height_contrast)
    } else {
        0.0
    };
    let mut v = p.height_scale * v;
    if v > 0.01 {
        v += p.height_offset;
    }
    if v > 0.01 { v } else { -1.0 }
}

/// The blend of the shader: from the layers' height values and weights, the four highest
/// layers' factors (layer index, factor) and the base layers' share. `transition` is
/// `HeightTransition`.
// bg3: Terrain_PS4 / Terrain_PS8 (top four by height) (Core.metallib, 4.1.1.7631656)
pub fn blend(heights: &[f32], weights: &[f32], transition: f32) -> (Vec<(usize, f32)>, f32) {
    let mut order: Vec<usize> = (0..heights.len()).collect();
    order.sort_by(|&a, &b| heights[b].total_cmp(&heights[a]));
    order.truncate(4);
    let h: Vec<f32> = order.iter().map(|&i| heights[i].max(0.0)).collect();
    let base = (1.0 - h.iter().sum::<f32>()).clamp(0.0, 1.0);
    let top = h.iter().copied().fold(base, f32::max);
    let g: Vec<f32> = order
        .iter()
        .zip(&h)
        .map(|(&i, &v)| weights[i] * ((v - top + transition).max(0.0) + 1e-6))
        .collect();
    let total = g.iter().sum::<f32>() + base * transition;
    let factors: Vec<(usize, f32)> = order
        .iter()
        .zip(&g)
        .map(|(&i, &v)| (i, v / total))
        .collect();
    let rest = (1.0 - factors.iter().map(|f| f.1).sum::<f32>()).clamp(0.0, 1.0);
    (factors, rest)
}

/// Which texture array a brush map goes to, as in the engine: `DXT5_sRGB` (base colour),
/// `DXT5` (normal and detail normal maps), `DXT1` (physical maps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArrayKind {
    BaseColor,
    Normal,
    Physical,
}

impl ArrayKind {
    fn format(self) -> DdsFormat {
        match self {
            Self::BaseColor | Self::Normal => DdsFormat::Bc3,
            Self::Physical => DdsFormat::Bc1,
        }
    }
}

/// One texture array: `layers` slices of `size²` texels with `mips` levels each, data
/// slice-major (all mips of slice 0, then slice 1, …).
#[derive(Debug, Clone, PartialEq)]
pub struct TextureArray {
    /// `Bc(..)` when every slice kept its block-compressed data, else `Rgba8`.
    pub format: TexelFormat,
    pub size: u32,
    pub mips: u32,
    pub layers: u32,
    pub data: Vec<u8>,
}

/// The brush texture arrays (the engine's `ls::TerrainBrushSystem::ManagedTextureArray`s) and
/// each brush's slices.
#[derive(Debug, Default)]
pub struct TextureArrays {
    pub arrays: HashMap<ArrayKind, TextureArray>,
    /// Brush ID → slices of base colour, normal, physical, detail normal map (−1 = none).
    pub slices: HashMap<String, [f32; 4]>,
    /// Maps that failed to load (texture ID, error).
    pub errors: Vec<(String, String)>,
}

/// Array texture size: the engine's highest texture quality (`TerrainBrushSystem::TEX_RES`
/// 1024/512/256/128 with 11/10/9/8 mips by setting).
// bg3: ls::TerrainBrushSystem::TEX_RES, TEX_MIPS (4.1.1.7631656)
pub const TEXTURE_SIZE: u32 = 1024;

/// One decoded or block-compressed map, cut to `size²` with its mips.
struct Slice {
    format: TexelFormat,
    mips: Vec<Vec<u8>>,
}

/// Load the maps of `brushes` into texture arrays of `size²` texels.
pub fn load_texture_arrays<'a>(
    vfs: &Vfs,
    banks: &ResourceBanks,
    brushes: impl IntoIterator<Item = &'a TerrainBrush>,
    size: u32,
) -> TextureArrays {
    use rayon::prelude::*;
    let mut out = TextureArrays::default();
    // Texture ID → (kind, slice index).
    let mut wanted: Vec<(ArrayKind, String)> = Vec::new();
    let mut index: HashMap<(ArrayKind, String), usize> = HashMap::new();
    let mut slot = |kind: ArrayKind, id: &Option<String>| -> f32 {
        let Some(id) = id
            .as_ref()
            .filter(|id| banks.textures.contains_key(id.as_str()))
        else {
            return -1.0;
        };
        let key = (kind, id.clone());
        let next = wanted.iter().filter(|w| w.0 == kind).count();
        *index.entry(key.clone()).or_insert_with(|| {
            wanted.push(key);
            next
        }) as f32
    };
    for b in brushes {
        let s = [
            slot(ArrayKind::BaseColor, &b.base_color_map),
            slot(ArrayKind::Normal, &b.normal_map),
            slot(ArrayKind::Physical, &b.physical_map),
            if b.use_detail_normal_map {
                slot(ArrayKind::Normal, &b.detail_normal_map)
            } else {
                -1.0
            },
        ];
        out.slices.insert(b.id.clone(), s);
    }
    let loaded: Vec<Result<Slice, String>> = wanted
        .par_iter()
        .map(|(kind, id)| load_slice(vfs, banks, *kind, id, size))
        .collect();
    let mut by_kind: HashMap<ArrayKind, Vec<Option<Slice>>> = HashMap::new();
    for ((kind, id), r) in wanted.iter().zip(loaded) {
        let slot = by_kind.entry(*kind).or_default();
        match r {
            Ok(s) => slot.push(Some(s)),
            Err(e) => {
                out.errors.push((id.clone(), e));
                slot.push(None);
            }
        }
    }
    for (kind, slices) in by_kind {
        out.arrays.insert(kind, build_array(kind, slices, size));
    }
    out
}

fn load_slice(
    vfs: &Vfs,
    banks: &ResourceBanks,
    kind: ArrayKind,
    id: &str,
    size: u32,
) -> Result<Slice, String> {
    let path = banks
        .textures
        .get(id)
        .and_then(|t| t.source_file.as_deref())
        .ok_or("no source file")?;
    let dds = vfs.read(path).map_err(|e| format!("{path}: {e}"))?;
    let t = convert::passthrough(&dds).map_err(|e| format!("{path}: {e}"))?;
    if t.format == TexelFormat::Bc(kind.format())
        && t.width == t.height
        && t.width >= size
        && let Some(mips) = cut_levels(&t, size)
    {
        return Ok(Slice {
            format: t.format,
            mips,
        });
    }
    let t = convert::rgba(&dds).map_err(|e| format!("{path}: {e}"))?;
    Ok(Slice {
        format: TexelFormat::Rgba8,
        mips: rgba_levels(&t, size),
    })
}

/// Bytes of a level of `w × h` texels.
fn level_bytes(format: TexelFormat, w: u32, h: u32) -> usize {
    match format {
        TexelFormat::Bc(f) => {
            let block = if f == DdsFormat::Bc1 || f == DdsFormat::Bc4 {
                8
            } else {
                16
            };
            w.max(1).div_ceil(4) as usize * h.max(1).div_ceil(4) as usize * block
        }
        TexelFormat::Rg8 => (w * h * 2) as usize,
        TexelFormat::Rgba8 => (w * h * 4) as usize,
    }
}

/// The levels of `t` from the one `size` wide down, if `t` has that level.
fn cut_levels(t: &convert::Texture, size: u32) -> Option<Vec<Vec<u8>>> {
    let (mut w, mut h, mut offset) = (t.width, t.height, 0usize);
    let mut out = Vec::new();
    for _ in 0..t.mip_count {
        let n = level_bytes(t.format, w, h);
        if w <= size {
            out.push(t.data.get(offset..offset + n)?.to_vec());
        }
        offset += n;
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    (!out.is_empty()).then_some(out)
}

/// RGBA8 levels from `size²` down to 1×1: the decoded level nearest above `size`, resampled
/// bilinearly to `size²` if needed, then halved with a 2×2 box filter.
fn rgba_levels(t: &convert::Texture, size: u32) -> Vec<Vec<u8>> {
    let (mut w, mut h, mut offset) = (t.width, t.height, 0usize);
    let mut src = (t.width, t.height, 0usize);
    for _ in 0..t.mip_count {
        if w < size || h < size {
            break;
        }
        src = (w, h, offset);
        offset += (w * h * 4) as usize;
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    let (sw, sh, so) = src;
    let texels = &t.data[so..so + (sw * sh * 4) as usize];
    let mut level: Vec<u8> = if sw == size && sh == size {
        texels.to_vec()
    } else {
        let mut v = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                // Bilinear with wrapping (the arrays tile).
                let fx = (x as f32 + 0.5) * sw as f32 / size as f32 - 0.5;
                let fy = (y as f32 + 0.5) * sh as f32 / size as f32 - 0.5;
                let (ax, ay) = (fx - fx.floor(), fy - fy.floor());
                let x0 = (fx.floor() as i64).rem_euclid(sw as i64) as u32;
                let y0 = (fy.floor() as i64).rem_euclid(sh as i64) as u32;
                let (x1, y1) = ((x0 + 1) % sw, (y0 + 1) % sh);
                let at = |x: u32, y: u32, c: usize| texels[((y * sw + x) * 4) as usize + c] as f32;
                for c in 0..4 {
                    let top = at(x0, y0, c) * (1.0 - ax) + at(x1, y0, c) * ax;
                    let bottom = at(x0, y1, c) * (1.0 - ax) + at(x1, y1, c) * ax;
                    v.push((top * (1.0 - ay) + bottom * ay).round() as u8);
                }
            }
        }
        v
    };
    let mut out = Vec::new();
    let mut s = size;
    loop {
        let next_s = (s / 2).max(1);
        let next = if s > 1 {
            let mut n = Vec::with_capacity((next_s * next_s * 4) as usize);
            for y in 0..next_s {
                for x in 0..next_s {
                    for c in 0..4 {
                        let at = |dx: u32, dy: u32| {
                            level[(((2 * y + dy) * s + 2 * x + dx) * 4) as usize + c] as u32
                        };
                        n.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
                    }
                }
            }
            Some(n)
        } else {
            None
        };
        out.push(std::mem::take(&mut level));
        match next {
            Some(n) => {
                level = n;
                s = next_s;
            }
            None => break,
        }
    }
    out
}

/// Assemble slices into one array; any slice that isn't in the array's block format turns the
/// whole array into RGBA8 (BC slices are decoded for it). Missing slices are mid-grey.
fn build_array(kind: ArrayKind, slices: Vec<Option<Slice>>, size: u32) -> TextureArray {
    let all_bc = slices
        .iter()
        .flatten()
        .all(|s| s.format == TexelFormat::Bc(kind.format()));
    let format = if all_bc {
        TexelFormat::Bc(kind.format())
    } else {
        TexelFormat::Rgba8
    };
    let full = size.max(1).ilog2() + 1;
    let mips = slices
        .iter()
        .flatten()
        .map(|s| s.mips.len() as u32)
        .min()
        .unwrap_or(full)
        .min(full);
    let mut data = Vec::new();
    for s in &slices {
        let (mut w, mut h) = (size, size);
        for m in 0..mips as usize {
            let n = level_bytes(format, w, h);
            match s {
                Some(s) if s.format == format => data.extend_from_slice(&s.mips[m][..n]),
                Some(s) => {
                    // A BC slice in an RGBA8 array.
                    let TexelFormat::Bc(f) = s.format else {
                        unreachable!()
                    };
                    let rgba = bg3_material::bc::decode_level(f, w, h, &s.mips[m])
                        .unwrap_or_else(|| vec![128; n]);
                    data.extend_from_slice(&rgba[..n]);
                }
                None => {
                    let grey = TextureArray::grey(format, w, h);
                    data.extend_from_slice(&grey);
                }
            }
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
    }
    TextureArray {
        format,
        size,
        mips,
        layers: slices.len() as u32,
        data,
    }
}

impl TextureArray {
    /// One mid-grey level (normal maps decode it to a flat normal with the shader's blue
    /// channel taken as is).
    fn grey(format: TexelFormat, w: u32, h: u32) -> Vec<u8> {
        match format {
            TexelFormat::Bc(DdsFormat::Bc1) => {
                // Both endpoints 0x8410 (mid grey in RGB565), all indices 0.
                [0x10, 0x84, 0x10, 0x84, 0, 0, 0, 0].repeat(level_bytes(format, w, h) / 8)
            }
            TexelFormat::Bc(_) => {
                // BC3: alpha endpoints 128/128, colour as BC1 above.
                [
                    128, 128, 0, 0, 0, 0, 0, 0, 0x10, 0x84, 0x10, 0x84, 0, 0, 0, 0,
                ]
                .repeat(level_bytes(format, w, h) / 16)
            }
            _ => vec![128; level_bytes(format, w, h)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{CurveKey, FCurve, Interpolation};
    use crate::terrain::TerrainBaseLayer;

    fn params() -> LayerParams {
        LayerParams::new(&TerrainBrush::default(), &TerrainLayer::default(), [0.0; 4])
    }

    #[test]
    fn overrides_follow_the_mask() {
        let brush = TerrainBrush {
            height_contrast: 2.0,
            tiling: 0.5,
            detail_normal_tiling: 0.1,
            detail_normal_scale: 0.3,
            ..TerrainBrush::default()
        };
        let layer = TerrainLayer {
            overrides: [[1.72, 5.64, 4.0, 0.03], [0.15, 9.0, 0.2, 0.4]],
            override_mask: 1 | 4 | 16 | 64,
            ..TerrainLayer::default()
        };
        let p = LayerParams::new(&brush, &layer, [1.0, 2.0, 3.0, -1.0]);
        assert_eq!(p.height_scale, 1.72);
        assert_eq!(p.height_offset, 0.0);
        assert_eq!(p.height_contrast, 0.25);
        assert_eq!(p.alpha_threshold, 0.0);
        assert_eq!(p.tiling, 0.15);
        assert_eq!(p.detail_normal_tiling, 0.1);
        assert_eq!(p.detail_normal_strength, 0.4);
        assert_eq!(p.flags, FLAG_DETAIL_NORMAL);
        assert_eq!(p.gradient_mid_color[3], 1.0);
    }

    #[test]
    fn height_values_and_blend() {
        let p = params();
        // Default brush: HeightScale 3.5, contrast 1.
        assert_eq!(height_value(&p, 0.0, 1.0), -1.0);
        assert!((height_value(&p, 1.0, 0.5) - 1.75).abs() < 1e-6);
        // Below the alpha threshold: absent.
        assert_eq!(height_value(&p, 0.2, 0.5), -1.0);
        // Two layers at full weight: the higher one wins outright once the difference is
        // above the transition width.
        let (f, base) = blend(&[2.0, 1.0], &[1.0, 1.0], 0.2);
        assert_eq!(f[0].0, 0);
        assert!(f[0].1 > 0.999 && f[1].1 < 1e-3, "{f:?}");
        assert!(base < 1e-3);
        // Within the transition both show, in proportion to their margin.
        let (f, _) = blend(&[1.0, 0.9], &[1.0, 1.0], 0.2);
        assert!((f[0].1 / f[1].1 - 2.0).abs() < 1e-3, "{f:?}");
        // Low layers leave weight to the base layer.
        let (f, base) = blend(&[0.1, -1.0], &[0.2, 0.0], 1.0);
        assert!(base > 0.8, "{f:?} {base}");
        // Only the four highest layers take part.
        let (f, _) = blend(&[0.5, 0.1, 0.4, 0.3, 0.2], &[1.0; 5], 1.0);
        let mut idx: Vec<usize> = f.iter().map(|f| f.0).collect();
        idx.sort();
        assert_eq!(idx, [0, 2, 3, 4]);
    }

    fn terrain() -> Terrain {
        let key = |x: f32, y: f32| CurveKey {
            position: [x, y],
            left_slope: 0.0,
            right_slope: 0.0,
            interpolation: Interpolation::Linear,
        };
        Terrain {
            cells: [2, 1],
            heights: vec![0.0, 1.0, 2.0, 0.0, 1.0, 2.0],
            holes: vec![false; 2],
            layers: vec![
                TerrainLayer {
                    brush: "a".into(),
                    ..TerrainLayer::default()
                },
                TerrainLayer {
                    brush: "b".into(),
                    ..TerrainLayer::default()
                },
            ],
            weights: vec![vec![255, 0], vec![0, 200]],
            base_layers: vec![TerrainBaseLayer {
                brush: "b".into(),
                height_curve: FCurve {
                    keys: vec![key(0.0, 0.0), key(2.0, 2.0)],
                    ..FCurve::default()
                },
                slope_curve: FCurve {
                    keys: vec![key(0.0, 1.0), key(90.0, 0.0)],
                    ..FCurve::default()
                },
            }],
            curves_resolution: 3,
            ..Terrain::default()
        }
    }

    #[test]
    fn terrain_inputs() {
        let t = terrain();
        assert_eq!(base_layer_indices(&t), [1, NO_BASE_LAYER]);
        assert_eq!(height_scale_offset(&t), [2.0, 0.0]);
        let w = weight_slices(&t);
        assert_eq!(w, [255, 0, 0, 0, 0, 200, 0, 0]);
        let n = vertex_normals(&t);
        assert_eq!(n.len(), 6);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!((n[1][0] + s).abs() < 1e-6 && (n[1][1] - s).abs() < 1e-6);
        // Ramp: slopes 90°, 60°, 0° and heights 0, 1, 2 (normalised by the peak key, 2).
        let r = base_ramp(&t);
        assert_eq!(r.width, 3);
        assert_eq!(r.rows[0][0], 0);
        assert!((r.rows[0][1] as f32 / 65535.0 - 1.0 / 3.0).abs() < 1e-3);
        assert_eq!(r.rows[0][2], 65535);
        assert_eq!(r.rows[2], [0, 32768, 65535]);
        assert_eq!(r.rows[1], [0, 0, 0]);
    }

    #[test]
    fn rgba_levels_resample_and_halve() {
        let t = convert::Texture {
            format: TexelFormat::Rgba8,
            width: 2,
            height: 2,
            mip_count: 1,
            data: vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
        };
        let l = rgba_levels(&t, 4);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].len(), 64);
        assert_eq!(l[2].len(), 4);
        assert!((l[2][0] as i32 - 128).abs() <= 2, "{:?}", l[2]);
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn nautiloid_terrain_textures_load_into_arrays() {
        let install = match bg3_install::Install::locate() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let vfs = Vfs::from_install(&install).unwrap();
        let banks = ResourceBanks::load(&vfs).unwrap();
        let brushes = crate::terrain::load_terrain_brushes(&vfs).unwrap();
        let terrains = crate::terrain::load_level_terrains(&vfs, "TUT_Avernus_C").unwrap();
        let used: std::collections::BTreeSet<&str> = terrains
            .iter()
            .flat_map(|t| t.layers.iter().map(|l| l.brush.as_str()))
            .collect();
        let arrays = load_texture_arrays(
            &vfs,
            &banks,
            used.iter().filter_map(|id| brushes.get(*id)),
            256,
        );
        for (kind, a) in &arrays.arrays {
            eprintln!(
                "{kind:?}: {:?} {}² × {} slices, {} mips, {} bytes",
                a.format,
                a.size,
                a.layers,
                a.mips,
                a.data.len()
            );
        }
        assert!(arrays.errors.is_empty(), "{:?}", arrays.errors);
        // The engine's formats: base colour and normals BC3, physical maps BC1.
        assert_eq!(
            arrays.arrays[&ArrayKind::BaseColor].format,
            TexelFormat::Bc(DdsFormat::Bc3)
        );
        assert_eq!(
            arrays.arrays[&ArrayKind::Physical].format,
            TexelFormat::Bc(DdsFormat::Bc1)
        );
        // Every used brush resolves its base colour map; base layers are among the layers.
        for t in &terrains {
            for l in &t.layers {
                assert!(arrays.slices[&l.brush][0] >= 0.0, "{}", l.brush);
            }
            for (i, b) in base_layer_indices(t).iter().enumerate() {
                assert_eq!(
                    *b == NO_BASE_LAYER,
                    t.base_layers.get(i).is_none(),
                    "{}",
                    t.name
                );
            }
        }
    }
}
