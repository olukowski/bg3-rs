//! Terrain base colour from the brushes' textures, blended by the per-cell layer weights: the
//! viewer's simplified path (`BG3_STANDARD_TERRAIN=1`); the game's shading is ported in
//! `terrain_shading` and the viewer's `terrain.wesl`.
//!
//! The game draws a terrain with one texture layer per brush (`TerrainLayers`): each brush's
//! `BaseColorMap` tiles over the terrain, and the layer weights stored per cell in the
//! patches say how much of each layer shows. The engine keeps the brush textures in texture
//! arrays (`ls::TerrainBrushSystem::ManagedTextureArray`, 4.1.1.7631656) and blends them in the
//! terrain pixel shader (`Terrain_PS<n>` in `Core.metallib`). Here the blend is baked on the
//! CPU into a plain texture per terrain tile. See `docs/research/terrain.md`.
//!
//! From the shader: the weights are a texture with one texel per cell, sampled bilinearly
//! (texel centres at cell centres); on level ground a layer's texture coordinates are the
//! terrain-local `(x, -z)` times the layer's `Tiling`.
//!
//! **Simplification**: layer weights are linear blend factors normalised by their sum, on
//! linear colour. The shader actually blends by height (the
//! albedo's alpha with per-layer contrast, scale and offset, and a transition width) and
//! switches to side projections on steep slopes; neither is applied, nor are the per-layer
//! `ParamOverrides`.

use bg3_material::convert;
use bg3_vfs::Vfs;

use crate::{ResourceBanks, Terrain, TerrainBrush};

/// A brush's base colour map, decoded to linear RGB at one mip level.
#[derive(Debug, Clone)]
pub struct BrushAlbedo {
    pub width: usize,
    pub height: usize,
    /// Row-major linear RGB.
    pub texels: Vec<[f32; 3]>,
    /// Texture repeats per world unit (the brush's `Tiling`).
    pub tiling: f32,
}

impl BrushAlbedo {
    /// Bilinear sample with wrapping, at texture coordinates in repeats.
    pub fn sample(&self, s: f32, t: f32) -> [f32; 3] {
        let (w, h) = (self.width as f32, self.height as f32);
        let x = s.rem_euclid(1.0) * w - 0.5;
        let y = t.rem_euclid(1.0) * h - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| {
            let xi = (xi as i64).rem_euclid(self.width as i64) as usize;
            let yi = (yi as i64).rem_euclid(self.height as i64) as usize;
            self.texels[yi * self.width + xi]
        };
        let (a, b, c, d) = (
            at(x0, y0),
            at(x0 + 1.0, y0),
            at(x0, y0 + 1.0),
            at(x0 + 1.0, y0 + 1.0),
        );
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// How one terrain layer is coloured.
#[derive(Debug, Clone)]
pub enum LayerColor {
    Texture(BrushAlbedo),
    /// No texture (or a black brush): one linear colour.
    Flat([f32; 3]),
}

impl LayerColor {
    fn sample(&self, x: f32, z: f32) -> [f32; 3] {
        match self {
            // bg3: Terrain_PS4 (Core.metallib, 4.1.1.7631656): level-ground projection.
            Self::Texture(t) => t.sample(x * t.tiling, -z * t.tiling),
            Self::Flat(c) => *c,
        }
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Load a brush's `BaseColorMap` from the mip level that has at least `texels_per_unit`
/// texels per world unit (the finest one if none has fewer), so baking at that density
/// doesn't alias. `Ok(None)` when the brush has no base colour map or it doesn't resolve.
pub fn load_brush_albedo(
    vfs: &Vfs,
    banks: &ResourceBanks,
    brush: &TerrainBrush,
    texels_per_unit: f32,
) -> Result<Option<BrushAlbedo>, String> {
    let Some(texture) = brush
        .base_color_map
        .as_deref()
        .and_then(|id| banks.textures.get(id))
    else {
        return Ok(None);
    };
    let Some(path) = texture.source_file.as_deref() else {
        return Ok(None);
    };
    let dds = vfs.read(path).map_err(|e| format!("{path}: {e}"))?;
    let decoded = convert::rgba(&dds).map_err(|e| format!("{path}: {e}"))?;
    let tiling = if brush.tiling > 0.0 {
        brush.tiling
    } else {
        1.0
    };
    // Texels per world unit at level 0: width texels per repeat, `tiling` repeats per unit.
    let wanted = (texels_per_unit / tiling).max(1.0);
    let (mut w, mut h, mut offset) = (decoded.width as usize, decoded.height as usize, 0);
    for _ in 1..decoded.mip_count {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        if (nw as f32) < wanted {
            break;
        }
        offset += w * h * 4;
        (w, h) = (nw, nh);
    }
    let bytes = decoded
        .data
        .get(offset..offset + w * h * 4)
        .ok_or_else(|| format!("{path}: truncated mip chain"))?;
    let decode = |b: u8| {
        let c = b as f32 / 255.0;
        if texture.srgb { srgb_to_linear(c) } else { c }
    };
    let texels = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| [decode(p[0]), decode(p[1]), decode(p[2])])
        .collect();
    Ok(Some(BrushAlbedo {
        width: w,
        height: h,
        texels,
        tiling,
    }))
}

/// Layer weights of a terrain at a point in local cell coordinates: bilinear between cell
/// centres (clamped at the edges), normalised to sum to 1. `None` where no layer has weight.
pub fn weights_at(t: &Terrain, x: f32, z: f32, out: &mut Vec<f32>) -> bool {
    let [cw, ch] = t.cells;
    out.clear();
    out.resize(t.weights.len(), 0.0);
    if cw == 0 || ch == 0 {
        return false;
    }
    let cx = (x - 0.5).clamp(0.0, (cw - 1) as f32);
    let cz = (z - 0.5).clamp(0.0, (ch - 1) as f32);
    let (x0, z0) = (cx.floor() as usize, cz.floor() as usize);
    let (x1, z1) = ((x0 + 1).min(cw - 1), (z0 + 1).min(ch - 1));
    let (fx, fz) = (cx - x0 as f32, cz - z0 as f32);
    let corners = [
        (x0, z0, (1.0 - fx) * (1.0 - fz)),
        (x1, z0, fx * (1.0 - fz)),
        (x0, z1, (1.0 - fx) * fz),
        (x1, z1, fx * fz),
    ];
    let mut total = 0.0;
    for (layer, w) in t.weights.iter().zip(out.iter_mut()) {
        for &(x, z, f) in &corners {
            *w += layer[z * cw + x] as f32 * f;
        }
        total += *w;
    }
    if total <= 0.0 {
        return false;
    }
    out.iter_mut().for_each(|w| *w /= total);
    true
}

/// Bake the linear base colour of the cells `[x0, x1) × [z0, z1)` of a terrain at
/// `texels_per_cell` texels per cell side, row-major along local +X then +Z. `layers` has one
/// entry per terrain layer; texels where no layer has weight take the first layer's colour.
pub fn bake_albedo(
    t: &Terrain,
    layers: &[LayerColor],
    [x0, z0, x1, z1]: [usize; 4],
    texels_per_cell: usize,
) -> Vec<[f32; 3]> {
    let (w, h) = ((x1 - x0) * texels_per_cell, (z1 - z0) * texels_per_cell);
    let step = 1.0 / texels_per_cell as f32;
    let mut weights = Vec::new();
    let mut out = Vec::with_capacity(w * h);
    for j in 0..h {
        let z = z0 as f32 + (j as f32 + 0.5) * step;
        for i in 0..w {
            let x = x0 as f32 + (i as f32 + 0.5) * step;
            if !weights_at(t, x, z, &mut weights) {
                out.push(layers.first().map_or([0.0; 3], |l| l.sample(x, z)));
                continue;
            }
            let mut c = [0.0f32; 3];
            for (layer, &f) in layers.iter().zip(&weights) {
                if f <= 0.0 {
                    continue;
                }
                let s = layer.sample(x, z);
                for k in 0..3 {
                    c[k] += s[k] * f;
                }
            }
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainLayer;

    fn terrain(cells: [usize; 2], weights: Vec<Vec<u8>>) -> Terrain {
        Terrain {
            id: "t".into(),
            name: "t".into(),
            template: None,
            transform: Default::default(),
            cells,
            layers: (0..weights.len())
                .map(|i| TerrainLayer {
                    brush: format!("b{i}"),
                    ..Default::default()
                })
                .collect(),
            heights: vec![0.0; (cells[0] + 1) * (cells[1] + 1)],
            holes: vec![false; cells[0] * cells[1]],
            weights,
            patches: 1,
            ..Default::default()
        }
    }

    #[test]
    fn weights_are_bilinear_between_cell_centres_and_normalised() {
        // Two cells along X: layer 0 owns the left one, layer 1 the right one.
        let t = terrain([2, 1], vec![vec![255, 0], vec![0, 255]]);
        let mut w = Vec::new();
        assert!(weights_at(&t, 0.5, 0.5, &mut w));
        assert_eq!(w, [1.0, 0.0]);
        assert!(weights_at(&t, 1.0, 0.5, &mut w));
        assert_eq!(w, [0.5, 0.5]);
        // Clamped beyond the outer cell centres.
        assert!(weights_at(&t, 2.0, 0.9, &mut w));
        assert_eq!(w, [0.0, 1.0]);
        let empty = terrain([1, 1], vec![vec![0]]);
        assert!(!weights_at(&empty, 0.5, 0.5, &mut w));
    }

    #[test]
    fn bake_blends_layers_and_tiles_textures() {
        let t = terrain([2, 1], vec![vec![255, 0], vec![0, 255]]);
        // A 2×1 texture, half black and half white, repeating every 2 units.
        let stripes = BrushAlbedo {
            width: 2,
            height: 1,
            texels: vec![[0.0; 3], [1.0; 3]],
            tiling: 0.5,
        };
        let layers = [
            LayerColor::Flat([1.0, 0.0, 0.0]),
            LayerColor::Texture(stripes),
        ];
        let out = bake_albedo(&t, &layers, [0, 0, 2, 1], 2);
        assert_eq!(out.len(), 4 * 2);
        // Left edge: all layer 0.
        assert_eq!(out[0], [1.0, 0.0, 0.0]);
        // Right edge, x = 1.75: all layer 1, sampled at s = 0.875 → between the white texel's
        // centre (0.75) and the black one's (1.25 ≡ 0.25), 3/4 white.
        let right = out[3];
        assert!((right[0] - 0.75).abs() < 1e-5, "{right:?}");
        assert_eq!(right[0], right[1]);
        // Rows repeat along Z (the texture is one texel high).
        assert_eq!(out[4..], out[..4]);
    }

    #[test]
    fn texture_sampling_wraps() {
        let t = BrushAlbedo {
            width: 2,
            height: 2,
            texels: vec![[0.0; 3], [1.0; 3], [0.0; 3], [1.0; 3]],
            tiling: 1.0,
        };
        assert_eq!(t.sample(0.25, 0.25), [0.0; 3]);
        assert_eq!(t.sample(0.75, 0.75), [1.0; 3]);
        assert_eq!(t.sample(1.25, -0.75), [0.0; 3]);
        // On the seam: halfway between the last texel and the first.
        assert_eq!(t.sample(1.0, 0.5), [0.5; 3]);
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn nautiloid_terrain_bakes_from_brush_textures() {
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
        let lower = terrains
            .iter()
            .find(|t| t.name == "WLD_TUT_Avernus_C_Lower_A")
            .expect("the lower deck terrain");
        // Every brush of the Nautiloid's terrains has a base colour map that decodes, at a mip
        // coarse enough for 16 texels per unit.
        let layers: Vec<LayerColor> = lower
            .layers
            .iter()
            .map(|l| {
                let b = &brushes[&l.brush];
                let t = load_brush_albedo(&vfs, &banks, b, 16.0)
                    .unwrap()
                    .unwrap_or_else(|| panic!("{}: no base colour map", b.name));
                assert!(t.width as f32 >= 16.0 / t.tiling, "{}: {}", b.name, t.width);
                LayerColor::Texture(t)
            })
            .collect();
        // A patch bakes to a textured (not flat), dark-ish flesh colour.
        let texels = bake_albedo(lower, &layers, [0, 0, 64, 64], 4);
        assert_eq!(texels.len(), 256 * 256);
        let mean: [f32; 3] =
            std::array::from_fn(|k| texels.iter().map(|c| c[k]).sum::<f32>() / texels.len() as f32);
        let spread = texels
            .iter()
            .map(|c| (c[0] - mean[0]).abs())
            .fold(0.0f32, f32::max);
        eprintln!("mean linear albedo {mean:?}, max red deviation {spread}");
        assert!(mean.iter().all(|&c| c < 0.5), "{mean:?}");
        assert!(spread > 0.02, "flat bake");
    }
}
