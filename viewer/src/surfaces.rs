//! Surfaces (blood, …) drawn like the game's surface renderer (`docs/research/surfaces.md`):
//! `ecl::SurfaceDeferredRenderer` keeps a texture per AI subgrid with, per 0.5 m cell, two
//! (surface type, amount) pairs, and draws each surface type as a deferred decal box whose
//! material (the type's `DecalMaterial`) turns the cells into a shape: coverage bilinear
//! between cell centres, edges warped by a noise texture, faded where the receiver is off the
//! AI grid's height. We have no G-buffer, so the level material shader does that step per pixel
//! after the level decals (`shaders/surfaces.wesl`, called from `shaders/level.wesl`).
//!
//! This module prepares the data: one world-space cell texture over the level's AI grid
//! (`mask`: type and amount; `height`: the cells' AI height), the per-type material parameters
//! and a texture array per map. The cell textures keep their size, so updates from the sim
//! reuse the GPU textures (and the materials' bind groups). Only the
//! `VFX_Surface_AlphaBlend_Fluid` graph (blood, silver blood, electrified blood, mud) is
//! ported. `BG3_NO_SURFACES=1` turns surfaces off; `BG3_SURFACE_DEBUG=1` paints the shader's
//! inputs instead (red: coverage, green: height fade, blue: near a surface cell).

use std::collections::HashMap;

use bevy::asset::{RenderAssetUsages, embedded_asset, uuid_handle};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::storage::ShaderBuffer;
use bg3_material::{ShaderGraph, resolve};
use bg3_nav::NavGrid;
use bg3_sim::surface::{SURFACE_TYPES, type_index};
use bg3_vfs::Vfs;
use rayon::prelude::*;

use crate::decals::{MapKind, array_image, flat_chain, layer_chain};

/// The ported surface graph.
const FLUID_GRAPH: &str = "VFX_Surface_AlphaBlend_Fluid.lsf";

/// Texture array size (square, full mip chains).
const MAP_SIZE: u32 = 512;

/// Largest cell texture side (cells); bigger grids are cut around the focus point.
const MAX_CELLS: i32 = 2048;

/// `vec4`s per surface type in the parameter buffer, and of the header before them.
const TYPE_VEC4S: usize = 4;
const HEADER_VEC4S: usize = 2;

/// Distance fade of the deferred decal pass (`_DecalDeferredProperties.y`, see `decals.md`).
const FADE_DISTANCE: f32 = bg3_world::decals::FADE_DISTANCE;

pub const SURFACE_PARAMS: Handle<ShaderBuffer> =
    uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e01");
pub const SURFACE_MASK: Handle<Image> = uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e02");
pub const SURFACE_HEIGHT: Handle<Image> = uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e03");
pub const SURFACE_BASE: Handle<Image> = uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e04");
pub const SURFACE_NOISE: Handle<Image> = uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e05");
pub const SURFACE_NORMAL: Handle<Image> = uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e06");
pub const SURFACE_PHYSICAL: Handle<Image> = uuid_handle!("2b6f8a59-5f0e-4f57-9d64-7c2a8f3b1e07");

pub fn enabled() -> bool {
    std::env::var_os("BG3_NO_SURFACES").is_none()
}

/// One surface cell to draw: global AI grid column, its height and the surface type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceCell {
    pub column: (i32, i32),
    pub height: f32,
    pub ty: u8,
}

impl SurfaceCell {
    /// From a cell centre on its surface (`bg3_sim::Sim::surface_cells`).
    pub fn at(position: [f32; 3], ty: u8) -> Self {
        Self {
            column: bg3_sim::surface::column_of(position),
            height: position[1],
            ty,
        }
    }
}

/// The ground surfaces to show; replaced whenever they change (the sim's revision).
#[derive(Resource, Default)]
pub struct SurfaceCells {
    pub cells: Vec<SurfaceCell>,
    /// Set when `cells` changed and the textures need refilling.
    pub dirty: bool,
}

/// The level's surface rendering data, before upload.
#[derive(Resource)]
pub struct LevelSurfaces {
    params: Vec<[f32; 4]>,
    maps: Option<[Image; 4]>,
    /// First global column (x, z) and size of the cell textures.
    origin: (i32, i32),
    size: (u32, u32),
    grid: Option<std::sync::Arc<NavGrid>>,
    /// Types with a ported material, by type index.
    drawn: Vec<bool>,
}

/// What the textures were filled from (for the update system).
#[derive(Resource)]
pub struct SurfaceRegion {
    origin: (i32, i32),
    size: (u32, u32),
    grid: Option<std::sync::Arc<NavGrid>>,
    /// Only these types go into the cell texture: the others (chasm, fire, …) would take the
    /// texels of drawn surfaces stacked over them.
    drawn: Vec<bool>,
}

/// One type's material: parameters and texture IDs (`None`: not drawn).
#[derive(Debug, Clone, Default)]
struct FluidLook {
    base: Option<String>,
    noise: Option<String>,
    normal: Option<String>,
    physical: Option<String>,
    tilling_warp: f32,
    intensity: f32,
    tilling: f32,
    edge_width: f32,
    edge_range: f32,
    min_opacity: f32,
    normal_intensity: f32,
    color_intensity: f32,
    reflectance: f32,
    edge_color: [f32; 3],
    normal_blending: f32,
}

impl LevelSurfaces {
    /// Resolve the surface types' materials and size the cell textures to the level's AI grid
    /// (cut to [`MAX_CELLS`] around `focus`).
    pub fn load(
        vfs: &Vfs,
        loaded: &crate::level::LoadedLevel,
        grid: Option<std::sync::Arc<NavGrid>>,
        focus: Option<[f32; 3]>,
    ) -> Self {
        let started = std::time::Instant::now();
        let mut params = vec![[0.0f32; 4]; HEADER_VEC4S + TYPE_VEC4S * SURFACE_TYPES.len()];
        for t in 0..SURFACE_TYPES.len() {
            params[HEADER_VEC4S + TYPE_VEC4S * t][0] = -1.0;
        }
        let (origin, size) = grid
            .as_deref()
            .filter(|_| enabled())
            .map_or(((0, 0), (1, 1)), |g| region(g, focus));
        params[0] = [
            origin.0 as f32,
            origin.1 as f32,
            size.0 as f32,
            size.1 as f32,
        ];
        if !enabled() || grid.is_none() {
            return Self {
                params,
                maps: None,
                origin,
                size,
                grid,
                drawn: vec![false; SURFACE_TYPES.len()],
            };
        }

        let templates = bg3_world::surfaces::surface_templates(&loaded.templates);
        let banks = &loaded.banks;
        let mut graphs: HashMap<String, Option<ShaderGraph>> = HashMap::new();
        let mut looks: Vec<(u8, FluidLook)> = Vec::new();
        let mut skipped = 0;
        for (name, props) in &templates {
            let Some(ty) = type_index(name) else {
                continue;
            };
            let look = props
                .decal_material
                .as_deref()
                .and_then(|id| fluid_look(vfs, banks, id, &mut graphs));
            match look {
                Some(mut l) => {
                    l.normal_blending = props.normal_blending.unwrap_or(0.0);
                    looks.push((ty, l));
                }
                None => skipped += 1,
            }
        }
        looks.sort_by_key(|(t, _)| *t);
        for (layer, (ty, l)) in looks.iter().enumerate() {
            let at = HEADER_VEC4S + TYPE_VEC4S * usize::from(*ty);
            params[at] = [layer as f32, l.tilling_warp, l.intensity, l.tilling];
            params[at + 1] = [
                l.edge_width,
                l.edge_range,
                l.min_opacity,
                l.normal_intensity,
            ];
            params[at + 2] = [l.color_intensity, l.reflectance, l.normal_blending, 1.0];
            params[at + 3] = [l.edge_color[0], l.edge_color[1], l.edge_color[2], 0.0];
        }
        let debug = std::env::var_os("BG3_SURFACE_DEBUG").is_some();
        params[1] = [1.0, FADE_DISTANCE, f32::from(u8::from(debug)), 0.0];

        let maps = (!looks.is_empty()).then(|| {
            let build = |pick: fn(&FluidLook) -> Option<&String>, kind: MapKind, neutral| {
                let layers: Vec<Vec<u8>> = looks
                    .par_iter()
                    .map(|(_, l)| {
                        pick(l)
                            .and_then(|id| banks.textures.get(id))
                            .and_then(|t| {
                                let path = t.source_file.as_deref()?;
                                let dds = vfs.read(path).ok()?;
                                let tex = bg3_material::convert::rgba(&dds).ok()?;
                                Some(layer_chain(&tex, t.srgb, kind, MAP_SIZE))
                            })
                            .unwrap_or_else(|| flat_chain(neutral, MAP_SIZE))
                    })
                    .collect();
                array_image(&layers, MAP_SIZE, kind)
            };
            [
                build(|l| l.base.as_ref(), MapKind::Albedo, [0, 0, 0, 0]),
                build(|l| l.noise.as_ref(), MapKind::Linear, [128, 128, 128, 255]),
                build(
                    |l| l.normal.as_ref(),
                    MapKind::Linear,
                    MapKind::Linear.neutral(),
                ),
                build(
                    |l| l.physical.as_ref(),
                    MapKind::Linear,
                    MapKind::Linear.neutral(),
                ),
            ]
        });
        eprintln!(
            "surfaces: {} types drawn ({} {FLUID_GRAPH}, {skipped} other graphs not ported); \
             cell texture {}×{} from column {:?}, in {:.2?}",
            looks.len(),
            looks.len(),
            size.0,
            size.1,
            origin,
            started.elapsed()
        );
        let mut drawn = vec![false; SURFACE_TYPES.len()];
        for (ty, _) in &looks {
            drawn[usize::from(*ty)] = true;
        }
        Self {
            params,
            maps,
            origin,
            size,
            grid,
            drawn,
        }
    }
}

/// The cell texture's region: the grid's columns, cut to [`MAX_CELLS`] around `focus` (or
/// the grid's centre).
fn region(grid: &NavGrid, focus: Option<[f32; 3]>) -> ((i32, i32), (u32, u32)) {
    let mut lo = (i32::MAX, i32::MAX);
    let mut hi = (i32::MIN, i32::MIN);
    for s in &grid.file.subgrids {
        if s.width == 0 || s.height == 0 {
            continue;
        }
        let (x, z) = s.global_origin();
        lo = (lo.0.min(x), lo.1.min(z));
        hi = (
            hi.0.max(x + s.width as i32 - 1),
            hi.1.max(z + s.height as i32 - 1),
        );
    }
    if lo.0 > hi.0 {
        return ((0, 0), (1, 1));
    }
    let centre = focus.map_or(((lo.0 + hi.0) / 2, (lo.1 + hi.1) / 2), |p| {
        bg3_sim::surface::column_of(p)
    });
    let axis = |lo: i32, hi: i32, c: i32| {
        let n = hi - lo + 1;
        if n <= MAX_CELLS {
            (lo, n as u32)
        } else {
            let start = (c - MAX_CELLS / 2).clamp(lo, hi - MAX_CELLS + 1);
            (start, MAX_CELLS as u32)
        }
    };
    let (x0, w) = axis(lo.0, hi.0, centre.0);
    let (z0, h) = axis(lo.1, hi.1, centre.1);
    ((x0, z0), (w, h))
}

/// The look of a surface material on the fluid graph; `None` for other graphs.
fn fluid_look(
    vfs: &Vfs,
    banks: &bg3_world::ResourceBanks,
    id: &str,
    graphs: &mut HashMap<String, Option<ShaderGraph>>,
) -> Option<FluidLook> {
    let mat = banks.materials.get(id)?;
    let path = mat.source_file.as_deref()?;
    if path.rsplit('/').next() != Some(FLUID_GRAPH) {
        return None;
    }
    let graph = graphs.entry(path.to_owned()).or_insert_with(|| {
        let res = bg3_lsf::parse(&vfs.read(path).ok()?).ok()?;
        ShaderGraph::from_resource(&res)
    });
    let m = resolve(path, graph.as_ref(), &mat.params);
    let tex = |name: &str| {
        m.textures
            .iter()
            .find(|t| t.param == name)
            .map(|t| t.id.clone())
    };
    let f = |name: &str, default: f32| m.scalar(name).unwrap_or(default);
    let edge = m.vector("EdgeColor").unwrap_or(&[0.0, 0.0, 0.0]);
    Some(FluidLook {
        base: tex("BaseColor"),
        noise: tex("Deformation_Noise"),
        normal: tex("Normal"),
        physical: tex("Physical"),
        tilling_warp: f("Tilling_WarpNoise", 1.0),
        intensity: f("Intensity", 0.0),
        tilling: f("Tilling", 1.0),
        edge_width: f("Edge_Width", 1.0),
        edge_range: f("Edge_Range", 0.0),
        min_opacity: f("MinOpacity", 1.0),
        normal_intensity: f("Normal_Intensity", 1.0),
        color_intensity: f("Color_Intensity", 0.0),
        reflectance: f("Reflectance", 0.5),
        edge_color: [
            edge.first().copied().unwrap_or(0.0),
            edge.get(1).copied().unwrap_or(0.0),
            edge.get(2).copied().unwrap_or(0.0),
        ],
        normal_blending: 0.0,
    })
}

fn cell_image(size: (u32, u32), format: TextureFormat, data: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        format,
        // Kept in the main world: the update system rewrites the data in place.
        RenderAssetUsages::all(),
    );
    image.sampler = bevy::image::ImageSampler::nearest();
    image
}

/// Upload [`LevelSurfaces`] to the fixed handles the level materials bind.
pub fn upload(
    mut commands: Commands,
    surfaces: Option<Res<LevelSurfaces>>,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let Some(s) = surfaces else {
        return;
    };
    let usage = RenderAssetUsages::RENDER_WORLD;
    let _ = buffers.insert(&SURFACE_PARAMS, ShaderBuffer::new(s.params.clone(), usage));
    let texels = (s.size.0 * s.size.1) as usize;
    let _ = images.insert(
        &SURFACE_MASK,
        cell_image(s.size, TextureFormat::Rgba8Unorm, vec![0; texels * 4]),
    );
    let _ = images.insert(
        &SURFACE_HEIGHT,
        cell_image(s.size, TextureFormat::R32Float, vec![0; texels * 4]),
    );
    let empty = |kind| array_image(&[flat_chain([0; 4], 8)], 8, kind);
    let [base, noise, normal, physical] = s.maps.clone().unwrap_or_else(|| {
        [
            empty(MapKind::Albedo),
            empty(MapKind::Linear),
            empty(MapKind::Linear),
            empty(MapKind::Linear),
        ]
    });
    for (h, i) in [
        (&SURFACE_BASE, base),
        (&SURFACE_NOISE, noise),
        (&SURFACE_NORMAL, normal),
        (&SURFACE_PHYSICAL, physical),
    ] {
        let _ = images.insert(h, i);
    }
    commands.insert_resource(SurfaceRegion {
        origin: s.origin,
        size: s.size,
        grid: s.grid.clone(),
        drawn: s.drawn.clone(),
    });
    commands.remove_resource::<LevelSurfaces>();
}

/// Rewrite the cell textures from [`SurfaceCells`] when they change.
pub fn update_textures(
    mut cells: ResMut<SurfaceCells>,
    region: Option<Res<SurfaceRegion>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(region) = region else {
        return;
    };
    if !cells.dirty {
        return;
    }
    cells.dirty = false;
    let (mask, height) = fill(&cells.cells, &region);
    if std::env::var_os("BG3_SURFACE_DEBUG").is_some() {
        let drawn = mask.chunks(4).filter(|t| t[0] != 0).count();
        eprintln!(
            "surfaces: {} cells, {drawn} in the cell texture",
            cells.cells.len()
        );
    }
    if let Some(mut img) = images.get_mut(&SURFACE_MASK) {
        img.data = Some(mask);
    }
    if let Some(mut img) = images.get_mut(&SURFACE_HEIGHT) {
        img.data = Some(height);
    }
}

/// The cell textures' texels: mask (R type, G second type, B amount, A second amount, as the
/// game's `_SurfaceMask`) and height (f32 metres). Each covered cell's neighbours get the
/// height of their tile nearest the cell, so the bilinear height between covered and
/// uncovered cells stays on the surface.
fn fill(cells: &[SurfaceCell], region: &SurfaceRegion) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (region.size.0 as i32, region.size.1 as i32);
    let n = (w * h) as usize;
    let mut mask = vec![0u8; n * 4];
    let mut height = vec![f32::NAN; n];
    let index = |c: (i32, i32)| {
        let (x, z) = (c.0 - region.origin.0, c.1 - region.origin.1);
        (x >= 0 && z >= 0 && x < w && z < h).then_some((z * w + x) as usize)
    };
    let drawn = |c: &&SurfaceCell| {
        region
            .drawn
            .get(usize::from(c.ty))
            .copied()
            .unwrap_or(false)
    };
    for c in cells.iter().filter(drawn) {
        let Some(i) = index(c.column) else {
            continue;
        };
        mask[i * 4] = c.ty;
        mask[i * 4 + 2] = 255;
        height[i] = c.height;
    }
    for c in cells.iter().filter(drawn) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let col = (c.column.0 + dx, c.column.1 + dz);
                let Some(i) = index(col) else {
                    continue;
                };
                if !height[i].is_nan() {
                    continue;
                }
                height[i] = region
                    .grid
                    .as_deref()
                    .and_then(|g| g.closest_in_column(col, c.height).map(|t| g.height(t)))
                    .filter(|y| (y - c.height).abs() < 1.5)
                    .unwrap_or(c.height);
            }
        }
    }
    let height = height
        .into_iter()
        .flat_map(|v| if v.is_nan() { 0.0f32 } else { v }.to_le_bytes())
        .collect();
    (mask, height)
}

/// Painted surfaces of the level's grid, for the viewer without `--play` (the sim's
/// level-load surfaces).
pub fn painted_cells(grid: &std::sync::Arc<NavGrid>) -> Vec<SurfaceCell> {
    let mut sim = bg3_sim::Sim::default();
    sim.set_surface_grid(Some(std::sync::Arc::new(bg3_sim_nav::GridSurfaces::new(
        grid.clone(),
    ))));
    sim.surface_cells(bg3_sim::surface::SurfaceLayer::Ground)
        .into_iter()
        .map(|(p, ty)| SurfaceCell::at(p, ty))
        .collect()
}

pub struct SurfacesPlugin;

/// Keeps `shaders/surfaces.wesl` (imported by `level.wesl`) loaded.
#[derive(Resource)]
#[allow(dead_code)]
struct SurfaceShaderLibrary(Handle<Shader>);

impl Plugin for SurfacesPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/surfaces.wesl");
        let library: Handle<Shader> =
            bevy::asset::load_embedded_asset!(app, "shaders/surfaces.wesl");
        app.insert_resource(SurfaceShaderLibrary(library))
            .init_resource::<SurfaceCells>()
            .add_systems(Update, update_textures);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_writes_cells_and_neighbour_heights() {
        let region = SurfaceRegion {
            origin: (10, 20),
            size: (4, 4),
            grid: None,
            drawn: vec![true; SURFACE_TYPES.len()],
        };
        let cells = [SurfaceCell {
            column: (11, 21),
            height: 3.5,
            ty: 4,
        }];
        let (mask, height) = fill(&cells, &region);
        let at = |x: usize, z: usize| (z * 4 + x) * 4;
        assert_eq!(&mask[at(1, 1)..at(1, 1) + 4], &[4, 0, 255, 0]);
        assert_eq!(mask[at(2, 2)], 0);
        let h = |x: usize, z: usize| {
            f32::from_le_bytes(height[at(x, z)..at(x, z) + 4].try_into().unwrap())
        };
        assert_eq!(h(1, 1), 3.5);
        assert_eq!(h(0, 0), 3.5);
        assert_eq!(h(2, 2), 3.5);
        assert_eq!(h(3, 3), 0.0);
        // Out-of-region cells are ignored.
        let (mask, _) = fill(
            &[SurfaceCell {
                column: (0, 0),
                height: 0.0,
                ty: 4,
            }],
            &region,
        );
        assert!(mask.iter().all(|&b| b == 0));
    }

    #[test]
    fn cell_from_position() {
        let c = SurfaceCell::at([-0.25, 2.0, 1.25], 4);
        assert_eq!(c.column, (-1, 2));
    }
}
