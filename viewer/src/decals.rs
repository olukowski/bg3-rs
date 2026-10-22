//! Level decals (`docs/research/decals.md`): the game draws them after the G-buffer pass,
//! projecting each decal's box onto the pixels of objects that receive decals and blending its
//! base colour, normal, roughness, reflectance and metalness into the G-buffer before lighting.
//! We have no G-buffer, so the level material shader (`shaders/level.wesl`, via
//! `shaders/decals.wesl`) does the same per pixel before it lights the surface: it looks up the
//! decals covering the pixel in a world-space grid and blends them in the game's draw order.
//!
//! This module prepares the data: decal records in a storage buffer, the grid, and one texture
//! array per map (base colour, normal, physical) with a layer per decal material. Receivers are
//! marked with [`RECEIVES_DECALS`] in their `MeshTag`. Terrain always receives decals (its
//! G-buffer shader sets the bit unconditionally); the game terrain material binds the same
//! resources. With `BG3_STANDARD_TERRAIN=1`, tiles get [`TerrainDecalMaterial`], Bevy's standard
//! material with the same decal step (`shaders/terrain_decals.wesl`).
//! `BG3_NO_DECALS=1` turns decals off; `BG3_DECAL_DEBUG=1` lists them and paints a checker of
//! each box's local x/z quarters where it projects.

use std::collections::HashMap;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bg3_material::{ShaderGraph, Slot, resolve};
use bg3_vfs::Vfs;
use bg3_world::ResourceBanks;
use bg3_world::decals::{Decal, FADE_DISTANCE, load_decals, sort_draw_order};
use rayon::prelude::*;

/// `MeshTag` bit of meshes whose object receives decals (`RecieveDecal`).
pub const RECEIVES_DECALS: u32 = 1;

/// The only decal graph ported; the others (`_VT`, `_WSTile`, the VFX decal graphs) are skipped.
const DEFAULT_DECAL_GRAPH: &str = "Engine_Default_Decal_Deferred.lsf";

/// Texture array sizes (square, with full mip chains). Decal maps are resampled to these:
/// base colour 1024², normal and physical maps 512² (`BG3_DECAL_SIZE` sets the base colour
/// size; the others are half of it).
const ALBEDO_SIZE: u32 = 1024;

/// World-space grid cell (m) of the per-pixel decal lookup.
const CELL: f32 = 4.0;
/// Cap on grid cells per axis (cells grow for very large levels).
const MAX_CELLS: f32 = 512.0;

/// Number of `vec4`s per decal record (`Decal` in `shaders/decals.wesl`).
const RECORD_VEC4S: usize = 9;
/// `u32`s in the grid header.
const GRID_HEADER: usize = 8;

pub fn enabled() -> bool {
    std::env::var_os("BG3_NO_DECALS").is_none()
}

fn albedo_size() -> u32 {
    std::env::var("BG3_DECAL_SIZE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|s| s.is_power_of_two() && *s >= 8)
        .unwrap_or(ALBEDO_SIZE)
}

/// One decal material: its texture IDs and reflectance.
#[derive(Debug, Clone, Default)]
struct DecalMaterial {
    basecolor: Option<String>,
    normal: Option<String>,
    physical: Option<String>,
    reflectance: f32,
}

/// CPU side of the level's decals, turned into GPU resources by [`upload`].
#[derive(Resource, Default)]
pub struct LevelDecals {
    records: Vec<[f32; 4]>,
    grid: Vec<u32>,
    albedo: Option<Image>,
    normal: Option<Image>,
    physical: Option<Image>,
}

/// The GPU resources shared by level and terrain materials.
#[derive(Resource, Clone, Debug)]
pub struct DecalBindings {
    /// Whether the level has any decal to draw.
    pub active: bool,
    pub decals: Handle<ShaderBuffer>,
    pub grid: Handle<ShaderBuffer>,
    pub albedo: Option<Handle<Image>>,
    pub normal: Option<Handle<Image>>,
    pub physical: Option<Handle<Image>>,
}

impl LevelDecals {
    pub fn load(vfs: &Vfs, level: &str, loaded: &crate::level::LoadedLevel) -> Self {
        if !enabled() {
            return Self::default();
        }
        let started = std::time::Instant::now();
        let mut decals = match load_decals(vfs, level, &loaded.templates) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("decals: {e}");
                return Self::default();
            }
        };
        let banks = &loaded.banks;
        let total = decals.len();

        // Resolve each decal material once; keep decals on the ported graph.
        let mut graphs: HashMap<String, Option<ShaderGraph>> = HashMap::new();
        let mut materials: HashMap<String, Option<DecalMaterial>> = HashMap::new();
        let mut skipped: HashMap<String, usize> = HashMap::new();
        decals.retain(|d| {
            let Some(id) = d.material.as_deref() else {
                *skipped.entry("no material".into()).or_default() += 1;
                return false;
            };
            let m = materials
                .entry(id.to_owned())
                .or_insert_with(|| decal_material(vfs, banks, id, &mut graphs));
            if m.is_none() {
                let graph = banks
                    .materials
                    .get(id)
                    .and_then(|m| m.source_file.as_deref())
                    .and_then(|p| p.rsplit('/').next())
                    .unwrap_or("unresolved material");
                *skipped.entry(graph.to_owned()).or_default() += 1;
            }
            m.is_some()
        });
        sort_draw_order(&mut decals);

        // One texture-array layer per material, in first-use order.
        let mut layers: Vec<(String, DecalMaterial)> = Vec::new();
        let mut layer_of: HashMap<String, usize> = HashMap::new();
        for d in &decals {
            let id = d.material.clone().unwrap_or_default();
            if !layer_of.contains_key(&id) {
                layer_of.insert(id.clone(), layers.len());
                let m = materials[&id].clone().unwrap_or_default();
                layers.push((id, m));
            }
        }

        let records: Vec<[f32; 4]> = decals
            .iter()
            .flat_map(|d| {
                let id = d.material.as_deref().unwrap_or_default();
                record(d, layer_of[id], layers[layer_of[id]].1.reflectance)
            })
            .collect();
        let mut grid = build_grid(&decals, CELL);
        if std::env::var_os("BG3_DECAL_DEBUG").is_some() {
            // Header flag: tint every decal box the shader finds (coverage check).
            grid[5] = 1;
        }

        let size = albedo_size();
        let small = (size / 2).max(8);
        let maps = |pick: fn(&DecalMaterial) -> Option<&String>, kind: MapKind, size: u32| {
            let layers: Vec<Vec<u8>> = layers
                .par_iter()
                .map(|(id, m)| {
                    let texture = pick(m).and_then(|t| banks.textures.get(t));
                    let decoded = texture.and_then(|t| {
                        let path = t.source_file.as_deref()?;
                        match vfs
                            .read(path)
                            .map_err(anyhow::Error::from)
                            .and_then(|dds| Ok(bg3_material::convert::rgba(&dds)?))
                        {
                            Ok(tex) => Some((tex, t.srgb)),
                            Err(e) => {
                                eprintln!("decal material {id}: {path}: {e}");
                                None
                            }
                        }
                    });
                    match decoded {
                        Some((tex, srgb)) => layer_chain(&tex, srgb, kind, size),
                        None => flat_chain(kind.neutral(), size),
                    }
                })
                .collect();
            array_image(&layers, size, kind)
        };
        let (albedo, normal, physical) = if layers.is_empty() {
            (None, None, None)
        } else {
            (
                Some(maps(|m| m.basecolor.as_ref(), MapKind::Albedo, size)),
                Some(maps(|m| m.normal.as_ref(), MapKind::Linear, small)),
                Some(maps(|m| m.physical.as_ref(), MapKind::Linear, small)),
            )
        };
        let mut skipped: Vec<_> = skipped.into_iter().collect();
        skipped.sort();
        eprintln!(
            "decals: {} of {total} drawn ({} materials){}, in {:.2?}",
            decals.len(),
            layers.len(),
            if skipped.is_empty() {
                String::new()
            } else {
                format!(
                    "; skipped {}",
                    skipped
                        .iter()
                        .map(|(g, n)| format!("{n} {g}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            started.elapsed()
        );
        if std::env::var_os("BG3_DECAL_DEBUG").is_some() {
            for d in &decals {
                eprintln!(
                    "  {} layer {} dims {:?} at {:?}",
                    d.name, d.layer, d.dimensions, d.transform.position
                );
            }
        }
        Self {
            records,
            grid,
            albedo,
            normal,
            physical,
        }
    }
}

/// Resolve a decal material on [`DEFAULT_DECAL_GRAPH`]; `None` for any other graph.
fn decal_material(
    vfs: &Vfs,
    banks: &ResourceBanks,
    id: &str,
    graphs: &mut HashMap<String, Option<ShaderGraph>>,
) -> Option<DecalMaterial> {
    let mat = banks.materials.get(id)?;
    let path = mat.source_file.as_deref()?;
    if path.rsplit('/').next() != Some(DEFAULT_DECAL_GRAPH) {
        return None;
    }
    let graph = graphs.entry(path.to_owned()).or_insert_with(|| {
        let res = bg3_lsf::parse(&vfs.read(path).ok()?).ok()?;
        ShaderGraph::from_resource(&res)
    });
    let m = resolve(path, graph.as_ref(), &mat.params);
    let tex = |s: Slot| m.texture(s).map(|t| t.id.clone());
    Some(DecalMaterial {
        basecolor: tex(Slot::BaseColor),
        normal: tex(Slot::Normal),
        physical: tex(Slot::Physical),
        reflectance: m.scalar("Reflectance").unwrap_or(0.5),
    })
}

/// The decal's GPU record (`Decal` in `shaders/decals.wesl`), game space: world → unit box,
/// the box axes (with the texture layer and reflectance in `w`), tiling/offset and
/// `_DecalDeferredProperties` (normal blending, fade distance, angle cutoff, opacity).
fn record(d: &Decal, layer: usize, reflectance: f32) -> [[f32; 4]; RECORD_VEC4S] {
    let m = d.local_from_world();
    let [x, y, z] = d.axes();
    [
        m[0],
        m[1],
        m[2],
        m[3],
        [x[0], x[1], x[2], layer as f32],
        [y[0], y[1], y[2], reflectance],
        [z[0], z[1], z[2], 0.0],
        [d.tiling[0], d.tiling[1], d.offset[0], d.offset[1]],
        [d.normal_blending, FADE_DISTANCE, d.angle_cutoff, d.opacity],
    ]
}

/// World-space (Bevy x/z) grid over the decals' bounds: a header (origin x, origin z, 1/cell
/// as f32 bits, cells along x and z, debug flag), then `(first, count)` per cell (row-major along x) into
/// the index list that follows. Each cell lists the decals overlapping it in draw order.
fn build_grid(decals: &[Decal], cell: f32) -> Vec<u32> {
    let mut header = vec![0u32; GRID_HEADER];
    if decals.is_empty() {
        return header;
    }
    // Bevy x = game x, Bevy z = −game z.
    let bounds: Vec<([f32; 2], [f32; 2])> = decals
        .iter()
        .map(|d| {
            let (min, max) = d.world_bounds();
            ([min[0], -max[2]], [max[0], -min[2]])
        })
        .collect();
    let lo = bounds
        .iter()
        .fold([f32::MAX; 2], |a, (b, _)| [a[0].min(b[0]), a[1].min(b[1])]);
    let hi = bounds
        .iter()
        .fold([f32::MIN; 2], |a, (_, b)| [a[0].max(b[0]), a[1].max(b[1])]);
    let cell = cell.max((hi[0] - lo[0]).max(hi[1] - lo[1]) / MAX_CELLS);
    let n = [0, 1].map(|k| (((hi[k] - lo[k]) / cell).floor() as u32 + 1).max(1));
    let mut lists: Vec<Vec<u32>> = vec![Vec::new(); (n[0] * n[1]) as usize];
    for (i, (min, max)) in bounds.iter().enumerate() {
        let c0 = [0, 1].map(|k| (((min[k] - lo[k]) / cell).floor() as u32).min(n[k] - 1));
        let c1 = [0, 1].map(|k| (((max[k] - lo[k]) / cell).floor() as u32).min(n[k] - 1));
        for z in c0[1]..=c1[1] {
            for x in c0[0]..=c1[0] {
                lists[(z * n[0] + x) as usize].push(i as u32);
            }
        }
    }
    header[0] = lo[0].to_bits();
    header[1] = lo[1].to_bits();
    header[2] = (1.0 / cell).to_bits();
    header[3] = n[0];
    header[4] = n[1];
    let mut out = header;
    let mut next = (GRID_HEADER + 2 * lists.len()) as u32;
    for l in &lists {
        out.push(next);
        out.push(l.len() as u32);
        next += l.len() as u32;
    }
    out.extend(lists.into_iter().flatten());
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MapKind {
    /// Base colour: stored sRGB, filtered in linear.
    Albedo,
    /// Normal and physical maps: stored as is (the shader reads the game's channels).
    Linear,
}

impl MapKind {
    /// Texel for a missing map: transparent base colour (the decal draws nothing), a flat
    /// normal (X in A, Y in G, Z in B), metal 0 / roughness ½ / occlusion 1.
    fn neutral(self) -> [u8; 4] {
        match self {
            MapKind::Albedo => [0, 0, 0, 0],
            MapKind::Linear => [0, 128, 255, 128],
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Resample a decoded texture (RGBA8, all mips) to `size`² and build its full mip chain,
/// starting from the smallest source mip at least `size` wide and high (bilinear, wrapping).
/// Colour is filtered in linear space; `srgb` says how the source is encoded.
fn layer_chain(
    tex: &bg3_material::convert::Texture,
    srgb: bool,
    kind: MapKind,
    size: u32,
) -> Vec<u8> {
    // Pick the source level.
    let mut offset = 0usize;
    let mut level = (tex.width, tex.height, 0usize);
    for l in 0..tex.mip_count {
        let (w, h) = ((tex.width >> l).max(1), (tex.height >> l).max(1));
        if w < size || h < size {
            break;
        }
        level = (w, h, offset);
        offset += (w * h * 4) as usize;
    }
    let (sw, sh, off) = level;
    let src = &tex.data[off..off + (sw * sh * 4) as usize];
    let decode_colour = |v: u8| {
        let c = v as f32 / 255.0;
        if srgb { srgb_to_linear(c) } else { c }
    };
    // An sRGB-flagged map is decoded as the GPU would; base colour is re-encoded at the end.
    let texel = |x: i64, y: i64| -> [f32; 4] {
        let x = x.rem_euclid(sw as i64) as usize;
        let y = y.rem_euclid(sh as i64) as usize;
        let p = &src[(y * sw as usize + x) * 4..][..4];
        [
            decode_colour(p[0]),
            decode_colour(p[1]),
            decode_colour(p[2]),
            p[3] as f32 / 255.0,
        ]
    };
    let mut top = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / size as f32 * sw as f32 - 0.5;
            let v = (y as f32 + 0.5) / size as f32 * sh as f32 - 0.5;
            let (x0, y0) = (u.floor(), v.floor());
            let (fx, fy) = (u - x0, v - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let a = texel(x0, y0);
            let b = texel(x0 + 1, y0);
            let c = texel(x0, y0 + 1);
            let d = texel(x0 + 1, y0 + 1);
            top.push(std::array::from_fn::<f32, 4, _>(|i| {
                let t = a[i] + (b[i] - a[i]) * fx;
                let s = c[i] + (d[i] - c[i]) * fx;
                t + (s - t) * fy
            }));
        }
    }
    mip_chain(top, size, kind)
}

/// Full mip chain (2×2 box filter in linear space) of a `size`² level, encoded as RGBA8.
fn mip_chain(mut level: Vec<[f32; 4]>, size: u32, kind: MapKind) -> Vec<u8> {
    let encode = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        let c = if kind == MapKind::Albedo {
            linear_to_srgb(c)
        } else {
            c
        };
        (c * 255.0 + 0.5) as u8
    };
    let mut out = Vec::new();
    let mut s = size as usize;
    loop {
        let alpha = |a: f32| (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        out.extend(
            level
                .iter()
                .flat_map(|p| [encode(p[0]), encode(p[1]), encode(p[2]), alpha(p[3])]),
        );
        if s == 1 {
            break;
        }
        let n = s / 2;
        let mut next = Vec::with_capacity(n * n);
        for y in 0..n {
            for x in 0..n {
                let at = |dx: usize, dy: usize| level[(2 * y + dy) * s + 2 * x + dx];
                let q = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
                next.push(std::array::from_fn(|i| {
                    0.25 * (q[0][i] + q[1][i] + q[2][i] + q[3][i])
                }));
            }
        }
        level = next;
        s = n;
    }
    out
}

/// A full mip chain of one colour.
fn flat_chain(texel: [u8; 4], size: u32) -> Vec<u8> {
    let texels: usize = (0..=size.ilog2())
        .map(|l| ((size >> l) as usize).pow(2))
        .sum();
    texel.repeat(texels)
}

fn array_image(layers: &[Vec<u8>], size: u32, kind: MapKind) -> Image {
    let format = match kind {
        MapKind::Albedo => TextureFormat::Rgba8UnormSrgb,
        MapKind::Linear => TextureFormat::Rgba8Unorm,
    };
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers.len() as u32,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Layer-major: every mip of layer 0, then layer 1, ….
    image.data = Some(layers.concat());
    image.texture_descriptor.mip_level_count = size.ilog2() + 1;
    image.texture_view_descriptor = Some(bevy::render::render_resource::TextureViewDescriptor {
        dimension: Some(bevy::render::render_resource::TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    image
}

/// Turn [`LevelDecals`] into GPU resources; runs before the level materials are built.
pub fn upload(
    mut commands: Commands,
    decals: Option<ResMut<LevelDecals>>,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let mut decals = decals
        .map(|mut d| std::mem::take(&mut *d))
        .unwrap_or_default();
    let active = !decals.records.is_empty();
    if decals.records.is_empty() {
        // A storage binding can't be empty.
        decals.records = vec![[0.0; 4]; RECORD_VEC4S];
    }
    if decals.grid.is_empty() {
        decals.grid = vec![0; GRID_HEADER];
    }
    let usage = RenderAssetUsages::RENDER_WORLD;
    commands.insert_resource(DecalBindings {
        active,
        decals: buffers.add(ShaderBuffer::new(decals.records, usage)),
        grid: buffers.add(ShaderBuffer::new(decals.grid, usage)),
        albedo: decals.albedo.map(|i| images.add(i)),
        normal: decals.normal.map(|i| images.add(i)),
        physical: decals.physical.map(|i| images.add(i)),
    });
    commands.remove_resource::<LevelDecals>();
}

/// Terrain tiles: Bevy's standard material with the level's decals blended in.
pub type TerrainDecalMaterial = ExtendedMaterial<StandardMaterial, DecalExtension>;

/// The decal bindings on their own (`shaders/decals.wesl`), for materials other than the level
/// material (which carries the same bindings in `LevelExtension`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct DecalExtension {
    #[storage(130, read_only)]
    pub decals: Handle<ShaderBuffer>,
    #[storage(131, read_only)]
    pub decal_grid: Handle<ShaderBuffer>,
    #[texture(132, dimension = "2d_array")]
    #[sampler(133)]
    pub decal_albedo: Option<Handle<Image>>,
    #[texture(134, dimension = "2d_array")]
    pub decal_normal: Option<Handle<Image>>,
    #[texture(135, dimension = "2d_array")]
    pub decal_physical: Option<Handle<Image>>,
}

impl DecalExtension {
    fn new(b: &DecalBindings) -> Self {
        Self {
            decals: b.decals.clone(),
            decal_grid: b.grid.clone(),
            decal_albedo: b.albedo.clone(),
            decal_normal: b.normal.clone(),
            decal_physical: b.physical.clone(),
        }
    }
}

impl MaterialExtension for DecalExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://bg3_viewer/shaders/terrain_decals.wesl".into()
    }
}

/// Decals on terrain (the level materials bind them themselves, see `level_materials.rs`).
pub struct DecalsPlugin;

impl Plugin for DecalsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/terrain_decals.wesl");
        app.add_plugins(MaterialPlugin::<TerrainDecalMaterial>::default())
            .add_systems(Last, swap_terrain_materials);
    }
}

type SpawnedTile<'a> = (Entity, &'a MeshMaterial3d<StandardMaterial>, &'a ChildOf);

/// Give spawned terrain tiles (children of the `terrain …` entities, `terrain.rs`) the decal
/// step on top of their standard material.
fn swap_terrain_materials(
    mut commands: Commands,
    bindings: Option<Res<DecalBindings>>,
    spawned: Query<SpawnedTile, Added<MeshMaterial3d<StandardMaterial>>>,
    names: Query<&Name>,
    standard: Res<Assets<StandardMaterial>>,
    mut materials: ResMut<Assets<TerrainDecalMaterial>>,
    mut swapped: Local<HashMap<AssetId<StandardMaterial>, Handle<TerrainDecalMaterial>>>,
) {
    let Some(bindings) = bindings.filter(|b| b.active) else {
        return;
    };
    for (entity, material, parent) in &spawned {
        let is_terrain = names
            .get(parent.parent())
            .is_ok_and(|n| n.as_str().starts_with("terrain "));
        if !is_terrain {
            continue;
        }
        let Some(base) = standard.get(&material.0) else {
            continue;
        };
        let handle = swapped
            .entry(material.id())
            .or_insert_with(|| {
                materials.add(TerrainDecalMaterial {
                    base: base.clone(),
                    extension: DecalExtension::new(&bindings),
                })
            })
            .clone();
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(handle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_world::Transform as GameTransform;

    fn decal(position: [f32; 3], dimensions: [f32; 3], layer: u32) -> Decal {
        Decal {
            id: format!("{position:?}"),
            name: "d".into(),
            transform: GameTransform {
                position,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
            material: Some("m".into()),
            dimensions,
            tiling: [1.0, 1.0],
            offset: [0.0, 0.0],
            normal_blending: 0.5,
            angle_cutoff: 0.0,
            layer,
            opacity: 1.0,
        }
    }

    /// The decal indices of the cell holding Bevy-space point (x, z).
    fn lookup(grid: &[u32], x: f32, z: f32) -> Vec<u32> {
        let f = |i: usize| f32::from_bits(grid[i]);
        let (n0, n1) = (grid[3], grid[4]);
        let cx = ((x - f(0)) * f(2)).floor();
        let cz = ((z - f(1)) * f(2)).floor();
        if cx < 0.0 || cz < 0.0 || cx >= n0 as f32 || cz >= n1 as f32 {
            return Vec::new();
        }
        let c = GRID_HEADER + 2 * (cz as usize * n0 as usize + cx as usize);
        let (first, count) = (grid[c] as usize, grid[c + 1] as usize);
        grid[first..first + count].to_vec()
    }

    #[test]
    fn grid_lists_overlapping_decals_in_order() {
        // Game z = 5 is Bevy z = −5.
        let decals = [
            decal([0.0, 0.0, 5.0], [8.0, 1.0, 8.0], 0),
            decal([2.0, 0.0, 5.0], [1.0, 1.0, 1.0], 0),
            decal([40.0, 0.0, 5.0], [2.0, 1.0, 2.0], 0),
        ];
        let grid = build_grid(&decals, 4.0);
        assert_eq!(lookup(&grid, 2.0, -5.0), [0, 1]);
        assert_eq!(lookup(&grid, -3.0, -8.0), [0]);
        assert_eq!(lookup(&grid, 40.0, -5.0), [2]);
        assert!(lookup(&grid, 20.0, -5.0).is_empty());
        assert!(lookup(&grid, 0.0, 5.0).is_empty());
    }

    #[test]
    fn empty_grid_has_no_cells() {
        let grid = build_grid(&[], 4.0);
        assert_eq!(grid.len(), GRID_HEADER);
        assert_eq!((grid[3], grid[4]), (0, 0));
    }

    #[test]
    fn record_packs_layer_reflectance_and_properties() {
        let r = record(&decal([1.0, 2.0, 3.0], [2.0, 2.0, 2.0], 0), 3, 0.25);
        assert_eq!(r[4][3], 3.0);
        assert_eq!(r[5][3], 0.25);
        assert_eq!(r[8], [0.5, FADE_DISTANCE, 0.0, 1.0]);
        // Translation column maps the decal's position to the box centre.
        assert_eq!(r[3], [-0.5, -1.0, -1.5, 1.0]);
    }

    #[test]
    fn resampled_chain_is_complete_and_filters_in_linear() {
        // 2×1 source: black and white columns, sRGB.
        let tex = bg3_material::convert::Texture {
            format: bg3_material::convert::TexelFormat::Rgba8,
            width: 2,
            height: 2,
            mip_count: 1,
            data: [[0, 0, 0, 255], [255, 255, 255, 255]].repeat(2).concat(),
        };
        let chain = layer_chain(&tex, true, MapKind::Albedo, 4);
        assert_eq!(chain.len(), (16 + 4 + 1) * 4);
        // The 1×1 mip averages black and white in linear: sRGB ≈ 188, not 128.
        let last = &chain[chain.len() - 4..];
        assert!((185..=191).contains(&last[0]), "{last:?}");
        assert_eq!(last[3], 255);
        assert_eq!(flat_chain([1, 2, 3, 4], 4).len(), chain.len());
    }
}
