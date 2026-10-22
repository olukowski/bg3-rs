//! Terrain rendering: one mesh per 64×64-cell patch, holes left out.
//!
//! By default terrain is drawn with the game's terrain shader (`shaders/terrain.wesl`): the
//! brushes' maps in texture arrays, per-terrain layer parameters, weight maps, vertex normals
//! and base layer ramp (`bg3_world::terrain_shading`), lit by the game's light loop.
//! `BG3_STANDARD_TERRAIN=1` instead bakes a base colour texture per patch from the brushes'
//! `BaseColorMap`s and the layer weights (`bg3_world::terrain_albedo`, linear blend) on Bevy's
//! `StandardMaterial` (A/B checks); `BG3_TERRAIN_BEVY_LIGHTING=1` lights the game's terrain
//! inputs with Bevy's PBR. See `docs/research/terrain.md` ("Drawing").

use std::collections::HashMap;

use anyhow::Result;
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::shader::ShaderRef;
use bg3_material::convert::TexelFormat;
use bg3_material::dds::DdsFormat;
use bg3_vfs::Vfs;
use bg3_world::terrain::{PATCH_CELLS, load_level_terrains, load_terrain_brushes};
use bg3_world::terrain_albedo::{LayerColor, bake_albedo, load_brush_albedo};
use bg3_world::terrain_shading::{
    self as shading, ArrayKind, BaseRamp, LayerParams, MAX_LAYERS, TextureArray, TextureArrays,
};
use bg3_world::{ResourceBanks, Terrain, TerrainBrush};
use rayon::prelude::*;

use crate::space;

const SHADER: &str = "embedded://bg3_viewer/shaders/terrain.wesl";
/// `TerrainUniform::flags`: light with Bevy's PBR instead of the game's loop.
const FLAG_BEVY_LIGHTING: u32 = 256;

/// Whether terrain gets the game's shading (`BG3_STANDARD_TERRAIN=1` bakes and uses Bevy's).
pub fn game_shading() -> bool {
    std::env::var_os("BG3_STANDARD_TERRAIN").is_none()
}

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;

/// `CB_Terrain` (the parts the pixel shader reads).
#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct TerrainUniform {
    /// 1/(cells+1) per axis (vertex normal map), 1/cells (weight maps).
    pub texel_size: Vec4,
    pub scale_offset: Vec2,
    pub base_layers: UVec2,
    pub height_transition: f32,
    pub ramp_width: u32,
    pub layer_count: u32,
    pub flags: u32,
}

/// `TerrainLayerParam` (112 bytes, as in the game).
#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct LayerParamGpu {
    pub textures: Vec4,
    pub gradient_bottom_color: Vec4,
    pub gradient_mid_color: Vec4,
    pub height_scale: f32,
    pub height_offset: f32,
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
    pub _pad0: u32,
    pub _pad1: u32,
}

impl From<&LayerParams> for LayerParamGpu {
    fn from(p: &LayerParams) -> Self {
        Self {
            textures: Vec4::from_array(p.textures),
            gradient_bottom_color: Vec4::from_array(p.gradient_bottom_color),
            gradient_mid_color: Vec4::from_array(p.gradient_mid_color),
            height_scale: p.height_scale,
            height_offset: p.height_offset,
            height_contrast: p.height_contrast,
            alpha_threshold: p.alpha_threshold,
            tiling: p.tiling,
            detail_normal_tiling: p.detail_normal_tiling,
            detail_normal_strength: p.detail_normal_strength,
            gradient_bottom_height: p.gradient_bottom_height,
            gradient_mid_point: p.gradient_mid_point,
            gradient_top_height: p.gradient_top_height,
            gradient_roughness_reach: p.gradient_roughness_reach,
            gradient_roughness_power: p.gradient_roughness_power,
            flags: p.flags,
            material_type: p.material_type,
            _pad0: 0,
            _pad1: 0,
        }
    }
}

/// `SB_TerrainLayerParams`.
#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct LayerParamsGpu {
    pub v: [LayerParamGpu; MAX_LAYERS],
}

/// The terrain shader's bindings (see `shaders/terrain.wesl`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct TerrainExtension {
    #[uniform(100)]
    pub uniform: TerrainUniform,
    #[uniform(101)]
    pub layers: LayerParamsGpu,
    #[texture(102, dimension = "2d_array")]
    #[sampler(103)]
    pub base_color_array: Handle<Image>,
    #[texture(104, dimension = "2d_array")]
    pub normal_array: Handle<Image>,
    #[texture(105, dimension = "2d_array")]
    pub physical_array: Handle<Image>,
    #[texture(106, dimension = "2d_array")]
    #[sampler(107)]
    pub weight_maps: Handle<Image>,
    #[texture(108)]
    #[sampler(109)]
    pub normal_map: Handle<Image>,
    #[texture(110, sample_type = "float", filterable = false)]
    pub base_ramp: Handle<Image>,
    /// The level's box lights (`lights::BOX_LIGHTS`), read by the shared game light loop.
    #[storage(120, read_only)]
    pub box_lights: Handle<bevy::render::storage::ShaderBuffer>,
}

impl MaterialExtension for TerrainExtension {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/terrain.wesl");
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
    }
}

/// Baked base colour texels per cell side (cells are 1 m): 6.25 cm texels.
const TEXELS_PER_CELL: usize = 16;

/// A patch's baked base colour: sRGB RGBA8 with its mip chain.
pub struct BakedTile {
    pub size: [u32; 2],
    pub mips: u32,
    pub data: Vec<u8>,
}

/// The game's terrain shading inputs of one terrain.
pub struct TerrainInputs {
    pub params: Vec<LayerParams>,
    pub base_layers: [u32; 2],
    pub scale_offset: [f32; 2],
    /// Weight maps, RGBA8 slice-major (`terrain_shading::weight_slices`).
    pub weights: Vec<u8>,
    pub normals: Vec<[f32; 3]>,
    pub ramp: BaseRamp,
}

pub enum TerrainShading {
    /// The game's shader: shared brush texture arrays and per-terrain inputs.
    Game {
        arrays: TextureArrays,
        inputs: Vec<TerrainInputs>,
    },
    /// `BG3_STANDARD_TERRAIN=1`: (terrain index, patch index) → baked base colour.
    Baked(HashMap<(usize, [usize; 2]), BakedTile>),
}

#[derive(Resource)]
pub struct LevelTerrain {
    pub terrains: Vec<Terrain>,
    pub shading: TerrainShading,
}

impl LevelTerrain {
    /// Terrains of `level` plus any extra (sub-)levels.
    pub fn load(vfs: &Vfs, levels: &[String], banks: &ResourceBanks) -> Result<Self> {
        let started = std::time::Instant::now();
        let mut terrains = Vec::new();
        for level in levels {
            terrains.extend(load_level_terrains(vfs, level)?);
        }
        let brushes = load_terrain_brushes(vfs)?;
        let cells: usize = terrains.iter().map(|t| t.cells[0] * t.cells[1]).sum();
        eprintln!(
            "loaded {} terrains ({cells} cells, {} brushes) from {levels:?} in {:.2?}",
            terrains.len(),
            brushes.len(),
            started.elapsed()
        );
        for t in &terrains {
            for (path, err) in &t.errors {
                eprintln!("  terrain {}: {path}: {err}", t.name);
            }
        }
        let shading = if game_shading() {
            prepare_game_shading(vfs, banks, &terrains, &brushes)
        } else {
            TerrainShading::Baked(bake_tiles(vfs, banks, &terrains, &brushes))
        };
        let out = Self { terrains, shading };
        if let Some((lo, hi)) = out.bounds() {
            eprintln!("terrain bounds (Bevy space): {lo} .. {hi}");
        }
        Ok(out)
    }

    /// World-space (Bevy) bounds of all terrains, ignoring rotation.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut out: Option<(Vec3, Vec3)> = None;
        for t in &self.terrains {
            let Some((lo, hi)) = t.height_range() else {
                continue;
            };
            let [x, y, z] = t.transform.position;
            let s = t.transform.scale;
            for corner in [
                [x, y + lo * s, z],
                [
                    x + t.cells[0] as f32 * s,
                    y + hi * s,
                    z + t.cells[1] as f32 * s,
                ],
            ] {
                let p = Vec3::from_array(space::vec3(corner));
                out = Some(match out {
                    None => (p, p),
                    Some((a, b)) => (a.min(p), b.max(p)),
                });
            }
        }
        out
    }
}

/// Load the brush texture arrays and every terrain's shader inputs.
fn prepare_game_shading(
    vfs: &Vfs,
    banks: &ResourceBanks,
    terrains: &[Terrain],
    brushes: &HashMap<String, TerrainBrush>,
) -> TerrainShading {
    let started = std::time::Instant::now();
    let used: std::collections::BTreeSet<&str> = terrains
        .iter()
        .flat_map(|t| t.layers.iter().map(|l| l.brush.as_str()))
        .collect();
    let arrays = shading::load_texture_arrays(
        vfs,
        banks,
        used.iter().filter_map(|id| brushes.get(*id)),
        shading::TEXTURE_SIZE,
    );
    for (id, e) in &arrays.errors {
        eprintln!("  terrain texture {id}: {e}");
    }
    let missing = used.iter().filter(|id| !brushes.contains_key(**id)).count();
    if missing > 0 {
        eprintln!("  {missing} terrain brushes not in any TerrainBrushBank");
    }
    let fallback = TerrainBrush::default();
    let inputs: Vec<TerrainInputs> = terrains
        .par_iter()
        .map(|t| {
            if t.layers.len() > MAX_LAYERS {
                eprintln!(
                    "  terrain {}: {} layers, drawing the first {MAX_LAYERS}",
                    t.name,
                    t.layers.len()
                );
            }
            let params = t
                .layers
                .iter()
                .take(MAX_LAYERS)
                .map(|l| {
                    let brush = brushes.get(&l.brush).unwrap_or(&fallback);
                    let slices = arrays.slices.get(&l.brush).copied().unwrap_or([-1.0; 4]);
                    LayerParams::new(brush, l, slices)
                })
                .collect();
            TerrainInputs {
                params,
                base_layers: shading::base_layer_indices(t),
                scale_offset: shading::height_scale_offset(t),
                weights: shading::weight_slices(t),
                normals: shading::vertex_normals(t),
                ramp: shading::base_ramp(t),
            }
        })
        .collect();
    let describe = |k: ArrayKind| {
        arrays.arrays.get(&k).map_or("none".to_owned(), |a| {
            format!("{} × {:?} {}²", a.layers, a.format, a.size)
        })
    };
    eprintln!(
        "terrain shading: {} brushes; base colour {}, normal {}, physical {} in {:.2?}",
        arrays.slices.len(),
        describe(ArrayKind::BaseColor),
        describe(ArrayKind::Normal),
        describe(ArrayKind::Physical),
        started.elapsed()
    );
    TerrainShading::Game { arrays, inputs }
}

/// Bake every patch's base colour from its terrain's brush textures, in parallel.
fn bake_tiles(
    vfs: &Vfs,
    banks: &ResourceBanks,
    terrains: &[Terrain],
    brushes: &HashMap<String, TerrainBrush>,
) -> HashMap<(usize, [usize; 2]), BakedTile> {
    let started = std::time::Instant::now();
    let used: std::collections::HashSet<&str> = terrains
        .iter()
        .flat_map(|t| t.layers.iter().map(|l| l.brush.as_str()))
        .collect();
    let colors: HashMap<&str, LayerColor> = used
        .into_par_iter()
        .map(|id| {
            let brush = brushes.get(id);
            let texture = brush
                .filter(|b| !b.is_black_brush)
                .map(|b| load_brush_albedo(vfs, banks, b, TEXELS_PER_CELL as f32));
            let color = match texture {
                Some(Ok(Some(t))) => LayerColor::Texture(t),
                other => {
                    if let Some(Err(e)) = other {
                        eprintln!("  terrain brush {id}: {e}");
                    }
                    LayerColor::Flat(brush_color(brush))
                }
            };
            (id, color)
        })
        .collect();
    let jobs: Vec<(usize, [usize; 2])> = terrains
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            let [tw, th] = [
                t.cells[0].div_ceil(PATCH_CELLS),
                t.cells[1].div_ceil(PATCH_CELLS),
            ];
            (0..th).flat_map(move |tz| (0..tw).map(move |tx| (i, [tx, tz])))
        })
        .collect();
    let baked: HashMap<_, _> = jobs
        .into_par_iter()
        .map(|(i, tile)| {
            let t = &terrains[i];
            let layers: Vec<LayerColor> = t
                .layers
                .iter()
                .map(|l| colors[l.brush.as_str()].clone())
                .collect();
            let rect = tile_rect(t, tile);
            let linear = bake_albedo(t, &layers, rect, TEXELS_PER_CELL);
            let size = [
                ((rect[2] - rect[0]) * TEXELS_PER_CELL) as u32,
                ((rect[3] - rect[1]) * TEXELS_PER_CELL) as u32,
            ];
            ((i, tile), srgb_mip_chain(size, linear))
        })
        .collect();
    eprintln!(
        "baked {} terrain tiles from {} brushes ({} textured) in {:.2?}",
        baked.len(),
        colors.len(),
        colors
            .values()
            .filter(|c| matches!(c, LayerColor::Texture(_)))
            .count(),
        started.elapsed()
    );
    baked
}

/// The cells `[x0, z0, x1, z1)` of a patch-sized tile.
fn tile_rect(t: &Terrain, [tx, tz]: [usize; 2]) -> [usize; 4] {
    let (x0, z0) = (tx * PATCH_CELLS, tz * PATCH_CELLS);
    [
        x0,
        z0,
        (x0 + PATCH_CELLS).min(t.cells[0]),
        (z0 + PATCH_CELLS).min(t.cells[1]),
    ]
}

fn linear_to_srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// sRGB RGBA8 levels of a linear RGB image, each half the previous (2×2 box filter on linear
/// colour), down to 1×1.
fn srgb_mip_chain([w, h]: [u32; 2], mut level: Vec<[f32; 3]>) -> BakedTile {
    let (mut lw, mut lh) = (w as usize, h as usize);
    let mut data = Vec::new();
    let mut mips = 0;
    loop {
        data.extend(level.iter().flat_map(|c| {
            [c[0], c[1], c[2]]
                .map(linear_to_srgb8)
                .into_iter()
                .chain([255])
        }));
        mips += 1;
        if lw <= 1 && lh <= 1 {
            break;
        }
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let at = |x: usize, y: usize| level[y.min(lh - 1) * lw + x.min(lw - 1)];
        let mut next = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            for x in 0..nw {
                let q = [
                    at(2 * x, 2 * y),
                    at(2 * x + 1, 2 * y),
                    at(2 * x, 2 * y + 1),
                    at(2 * x + 1, 2 * y + 1),
                ];
                next.push(std::array::from_fn(|i| {
                    q.iter().map(|c| c[i]).sum::<f32>() / 4.0
                }));
            }
        }
        (level, lw, lh) = (next, nw, nh);
    }
    BakedTile {
        size: [w, h],
        mips,
        data,
    }
}

fn baked_image(tile: &BakedTile) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: tile.size[0],
            height: tile.size[1],
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(tile.data.clone());
    image.texture_descriptor.mip_level_count = tile.mips;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    image
}

fn sampler(address: ImageAddressMode, mipmaps: bool, anisotropy: u16) -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: if mipmaps {
            ImageFilterMode::Linear
        } else {
            ImageFilterMode::Nearest
        },
        anisotropy_clamp: anisotropy,
        ..default()
    })
}

/// A 2D array image from slice-major data.
fn array_image(
    size: [u32; 2],
    layers: u32,
    mips: u32,
    format: TextureFormat,
    data: Vec<u8>,
    sampler: ImageSampler,
) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: layers.max(1),
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mips;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = sampler;
    image
}

/// A brush texture array as an image; `None` (no map of this kind) is one grey slice. The
/// layer textures repeat, filtered trilinearly (**assumption**: 8× anisotropic, as level
/// textures here; the game's `_DefaultWrapSampler` follows the graphics settings).
fn brush_array_image(kind: ArrayKind, array: Option<&TextureArray>) -> Image {
    let srgb = kind == ArrayKind::BaseColor;
    let wrap = sampler(ImageAddressMode::Repeat, true, 8);
    let Some(a) = array else {
        let format = if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        };
        return array_image([1, 1], 1, 1, format, vec![128, 128, 255, 128], wrap);
    };
    let format = match (a.format, srgb) {
        (TexelFormat::Bc(DdsFormat::Bc1), false) => TextureFormat::Bc1RgbaUnorm,
        (TexelFormat::Bc(DdsFormat::Bc1), true) => TextureFormat::Bc1RgbaUnormSrgb,
        (TexelFormat::Bc(_), false) => TextureFormat::Bc3RgbaUnorm,
        (TexelFormat::Bc(_), true) => TextureFormat::Bc3RgbaUnormSrgb,
        (_, false) => TextureFormat::Rgba8Unorm,
        (_, true) => TextureFormat::Rgba8UnormSrgb,
    };
    array_image(
        [a.size, a.size],
        a.layers,
        a.mips,
        format,
        a.data.clone(),
        wrap,
    )
}

/// Placeholder linear colour for a brush without a base colour texture, from keywords in its
/// name.
fn brush_color(brush: Option<&TerrainBrush>) -> [f32; 3] {
    let Some(b) = brush else {
        return [0.5, 0.5, 0.5];
    };
    if b.is_black_brush {
        return [0.02, 0.02, 0.02];
    }
    let name = b.name.to_ascii_lowercase();
    let palette: &[(&str, [f32; 3])] = &[
        ("flesh", [0.45, 0.12, 0.12]),
        ("membrane", [0.35, 0.10, 0.18]),
        ("nautiloid", [0.30, 0.18, 0.20]),
        ("black", [0.02, 0.02, 0.02]),
        ("grass", [0.18, 0.30, 0.08]),
        ("moss", [0.14, 0.26, 0.08]),
        ("needles", [0.30, 0.20, 0.10]),
        ("sand", [0.55, 0.47, 0.32]),
        ("ash", [0.22, 0.21, 0.20]),
        ("mud", [0.22, 0.16, 0.10]),
        ("soil", [0.25, 0.18, 0.11]),
        ("dirt", [0.32, 0.24, 0.15]),
        ("gravel", [0.35, 0.32, 0.28]),
        ("cliff", [0.45, 0.38, 0.30]),
        ("rock", [0.38, 0.36, 0.33]),
        ("paving", [0.45, 0.44, 0.42]),
        ("marble", [0.60, 0.58, 0.55]),
        ("stone", [0.42, 0.41, 0.39]),
        ("roots", [0.25, 0.17, 0.10]),
        ("snow", [0.85, 0.87, 0.90]),
        ("water", [0.10, 0.18, 0.22]),
    ];
    let base = palette
        .iter()
        .find(|(k, _)| name.contains(k))
        .map_or([0.4, 0.4, 0.4], |(_, c)| *c);
    // Small stable variation so neighbouring brushes of the same family stay distinguishable.
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    b.id.hash(&mut h);
    let v = 0.85 + (h.finish() % 1000) as f32 / 1000.0 * 0.3;
    base.map(|c| (c * v).min(1.0))
}

/// Mesh for one patch-sized tile of a terrain, in the terrain's local space (Bevy convention).
/// With `game_coords` UV 0 is the game's terrain-local `(x, z)` and UV 1 `(height, 0)` (the
/// terrain shader's inputs); otherwise UV 0 spans the tile's baked texture.
fn tile_mesh(t: &Terrain, tile: [usize; 2], game_coords: bool) -> Option<Mesh> {
    let [cw, ch] = t.cells;
    let [x0, z0, x1, z1] = tile_rect(t, tile);
    let vw = x1 - x0 + 1;
    let vertex = |x: usize, z: usize| ((z - z0) * vw + (x - x0)) as u32;

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut uvs_b = Vec::new();
    for z in z0..=z1 {
        for x in x0..=x1 {
            let h = t.height(x, z);
            positions.push(space::vec3([x as f32, h, z as f32]));
            // Central differences in BG3 local space, then mirrored like any other direction.
            let hx = |x: usize| t.height(x, z);
            let hz = |z: usize| t.height(x, z);
            let dx = (hx((x + 1).min(cw)) - hx(x.saturating_sub(1)))
                / ((x + 1).min(cw) - x.saturating_sub(1)) as f32;
            let dz = (hz((z + 1).min(ch)) - hz(z.saturating_sub(1)))
                / ((z + 1).min(ch) - z.saturating_sub(1)) as f32;
            normals.push(space::vec3(Vec3::new(-dx, 1.0, -dz).normalize().to_array()));
            if game_coords {
                uvs.push([x as f32, z as f32]);
                uvs_b.push([h, 0.0]);
            } else {
                uvs.push([
                    (x - x0) as f32 / (x1 - x0) as f32,
                    (z - z0) as f32 / (z1 - z0) as f32,
                ]);
            }
        }
    }
    let mut indices = Vec::new();
    for z in z0..z1 {
        for x in x0..x1 {
            if t.is_hole(x, z) {
                continue;
            }
            let (a, b, c, d) = (
                vertex(x, z),
                vertex(x, z + 1),
                vertex(x + 1, z),
                vertex(x + 1, z + 1),
            );
            // Counter-clockwise seen from +Y in BG3's coordinates (like GR2 index data), so
            // `flip_winding` below applies to terrain the same way as to meshes.
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    if indices.is_empty() {
        return None;
    }
    space::flip_winding(&mut indices);
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, crate::pick::mesh_usages());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    if game_coords {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvs_b);
    }
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

pub fn spawn_terrain(
    mut commands: Commands,
    terrain: Res<LevelTerrain>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
) {
    // The game-shaded path: one material per terrain over the shared brush arrays.
    let game: Option<Vec<Handle<TerrainMaterial>>> = match &terrain.shading {
        TerrainShading::Game { arrays, inputs } => {
            let array = |kind| brush_array_image(kind, arrays.arrays.get(&kind));
            let base_color = images.add(array(ArrayKind::BaseColor));
            let normal = images.add(array(ArrayKind::Normal));
            let physical = images.add(array(ArrayKind::Physical));
            let bevy_lighting = std::env::var_os("BG3_TERRAIN_BEVY_LIGHTING").is_some();
            Some(
                terrain
                    .terrains
                    .iter()
                    .zip(inputs)
                    .map(|(t, inp)| {
                        let ext = terrain_extension(t, inp, &mut images, bevy_lighting);
                        terrain_materials.add(TerrainMaterial {
                            base: StandardMaterial::default(),
                            extension: TerrainExtension {
                                base_color_array: base_color.clone(),
                                normal_array: normal.clone(),
                                physical_array: physical.clone(),
                                ..ext
                            },
                        })
                    })
                    .collect(),
            )
        }
        TerrainShading::Baked(_) => None,
    };
    let mut tiles = 0;
    for (i, t) in terrain.terrains.iter().enumerate() {
        let parent = commands
            .spawn((
                Name::new(format!("terrain {}", t.name)),
                space::transform(&t.transform),
                Visibility::default(),
            ))
            .id();
        for tz in 0..t.cells[1].div_ceil(PATCH_CELLS) {
            for tx in 0..t.cells[0].div_ceil(PATCH_CELLS) {
                let Some(mesh) = tile_mesh(t, [tx, tz], game.is_some()) else {
                    continue;
                };
                tiles += 1;
                let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), ChildOf(parent)));
                match (&game, &terrain.shading) {
                    (Some(game), _) => {
                        e.insert(MeshMaterial3d(game[i].clone()));
                    }
                    (None, TerrainShading::Baked(baked)) => {
                        let texture = baked
                            .get(&(i, [tx, tz]))
                            .map(|b| images.add(baked_image(b)));
                        e.insert(MeshMaterial3d(materials.add(StandardMaterial {
                            base_color_texture: texture,
                            perceptual_roughness: 0.9,
                            ..default()
                        })));
                    }
                    (None, TerrainShading::Game { .. }) => unreachable!(),
                }
            }
        }
    }
    eprintln!(
        "spawned {} terrains as {tiles} tile meshes ({} shading)",
        terrain.terrains.len(),
        if game.is_some() { "game" } else { "baked" }
    );
}

/// One terrain's bindings (the brush arrays are filled in by the caller).
fn terrain_extension(
    t: &Terrain,
    inp: &TerrainInputs,
    images: &mut Assets<Image>,
    bevy_lighting: bool,
) -> TerrainExtension {
    let [cw, ch] = t.cells.map(|c| c.max(1) as u32);
    let clamp = |mips| sampler(ImageAddressMode::ClampToEdge, mips, 1);
    // Weight maps: one RGBA8 texel per cell and per four layers, filtered bilinearly
    // (**assumption**: `Terrain_Sampler` clamps).
    let slices = t.layers.len().div_ceil(4).max(1) as u32;
    let weights = if t.cells[0] * t.cells[1] > 0 {
        inp.weights.clone()
    } else {
        vec![0; 4 * slices as usize]
    };
    let weight_maps = images.add(array_image(
        [cw, ch],
        slices,
        1,
        TextureFormat::Rgba8Unorm,
        weights,
        clamp(false),
    ));
    // Vertex normals in terrain space, `n · 0.5 + 0.5` (**assumption**: RGBA8 like the game's
    // other normal textures), sampled with a clamping trilinear sampler as in the game.
    let normal_data: Vec<u8> = inp
        .normals
        .iter()
        .flat_map(|n| {
            let q = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
            [q(n[0]), q(n[1]), q(n[2]), 255]
        })
        .collect();
    let mut normal_map = Image::new(
        Extent3d {
            width: cw + 1,
            height: ch + 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        normal_data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    normal_map.sampler = clamp(true);
    // The base layer ramp (R16 unorm in the game), read with loads.
    let ramp: Vec<u8> = inp
        .ramp
        .rows
        .iter()
        .flat_map(|row| row.iter().flat_map(|&v| (v as f32 / 65535.0).to_le_bytes()))
        .collect();
    let base_ramp = Image::new(
        Extent3d {
            width: inp.ramp.width as u32,
            height: 4,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        ramp,
        TextureFormat::R32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    let mut layers = LayerParamsGpu {
        v: [LayerParamGpu::default(); MAX_LAYERS],
    };
    for (slot, p) in layers.v.iter_mut().zip(&inp.params) {
        *slot = p.into();
    }
    TerrainExtension {
        uniform: TerrainUniform {
            texel_size: Vec4::new(
                1.0 / (cw + 1) as f32,
                1.0 / (ch + 1) as f32,
                1.0 / cw as f32,
                1.0 / ch as f32,
            ),
            scale_offset: Vec2::from_array(inp.scale_offset),
            base_layers: UVec2::from_array(inp.base_layers),
            height_transition: t.height_transition,
            ramp_width: inp.ramp.width as u32,
            layer_count: inp.params.len() as u32,
            flags: if bevy_lighting { FLAG_BEVY_LIGHTING } else { 0 },
        },
        box_lights: crate::lights::BOX_LIGHTS,
        layers,
        base_color_array: Handle::default(),
        normal_array: Handle::default(),
        physical_array: Handle::default(),
        weight_maps,
        normal_map: images.add(normal_map),
        base_ramp: images.add(base_ramp),
    }
}

/// `--terrain-only`: hide every level object so only terrain (and lights) remain visible.
pub fn hide_objects(
    mut commands: Commands,
    roots: Query<(Entity, &Name, Has<ChildOf>, Has<Camera>)>,
) {
    for (e, name, _, _) in roots.iter().filter(|(_, _, child, cam)| !child && !cam) {
        let keep = ["terrain ", "light ", "sun"]
            .iter()
            .any(|p| name.as_str().starts_with(p));
        if !keep {
            commands.entity(e).insert(Visibility::Hidden);
        }
    }
}
