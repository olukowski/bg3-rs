//! Level materials on the game's shading (`shaders/level.wesl`): Bevy's `StandardMaterial`
//! inputs plus the graph's reflectance, shading model, subsurface inputs, micro detail layer and
//! glow, lit by the game's light loop (`shaders/game_lighting.wesl`). See
//! `docs/research/materials.md` ("Level shading").
//!
//! Every level material is still built as a `StandardMaterial` (`materials.rs`); after spawning,
//! [`swap_level_materials`] replaces each such handle with its game-shaded counterpart, so the
//! spawning code is shared. `BG3_STANDARD_LEVEL=1` keeps the `StandardMaterial`s and
//! `BG3_LEVEL_BEVY_LIGHTING=1` lights the game's inputs with Bevy's PBR (A/B checks).

use std::collections::HashMap;

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bg3_material::ResolvedMaterial;
use bg3_material::character::sss::DiffusionProfile;
use bg3_material::level::{LEVEL_UNIFORM_LEN, LevelShading, SubsurfaceInput};
use bg3_world::ResourceBanks;

use crate::materials::{TextureKeys, Usage};

const SHADER: &str = "embedded://bg3_viewer/shaders/level.wesl";

/// `LevelUniform::flags` bits above the shading model, as in the shader.
const FLAG_SSS_MARK: u32 = 8;
const FLAG_DETAIL: u32 = 16;
/// `BG3_LEVEL_BEVY_LIGHTING=1`: the same inputs on Bevy's lighting (A/B of the light loop).
const FLAG_BEVY_LIGHTING: u32 = 256;

pub type LevelMaterial = ExtendedMaterial<StandardMaterial, LevelExtension>;

#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct LevelUniform {
    pub flags: u32,
    pub glow: u32,
    pub subsurface: u32,
    pub _pad: u32,
    pub v: [Vec4; LEVEL_UNIFORM_LEN],
}

/// The extension's bindings (see the shader).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct LevelExtension {
    #[uniform(100)]
    pub uniform: LevelUniform,
    #[texture(101)]
    #[sampler(102)]
    pub detail_map: Option<Handle<Image>>,
    #[texture(103)]
    #[sampler(104)]
    pub detail_normal_map: Option<Handle<Image>>,
    #[texture(105)]
    #[sampler(106)]
    pub glow_map: Option<Handle<Image>>,
    #[texture(107)]
    #[sampler(108)]
    pub sss_map: Option<Handle<Image>>,
}

impl MaterialExtension for LevelExtension {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn deferred_fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

/// Whether level materials get the game's shading (`BG3_STANDARD_LEVEL=1` keeps Bevy's).
pub fn enabled() -> bool {
    std::env::var_os("BG3_STANDARD_LEVEL").is_none()
}

pub struct LevelMaterialsPlugin;

/// Keeps the shared lighting module (imported by `character.wesl` and `level.wesl`) loaded.
#[derive(Resource)]
#[allow(dead_code)]
struct GameLightingLibrary(Handle<Shader>);

impl Plugin for LevelMaterialsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/game_lighting.wesl");
        embedded_asset!(app, "shaders/level.wesl");
        let library: Handle<Shader> = load_embedded_asset!(app, "shaders/game_lighting.wesl");
        app.insert_resource(GameLightingLibrary(library))
            .init_resource::<LevelGameMaterials>()
            .add_plugins(MaterialPlugin::<LevelMaterial>::default())
            .add_systems(Last, swap_level_materials);
    }
}

/// Textures and shading of one level material, as indices into the prepared image list.
#[derive(Debug, Clone)]
pub struct LevelPlan {
    shading: LevelShading,
    uniform: [[f32; 4]; LEVEL_UNIFORM_LEN],
    profile: Option<DiffusionProfile>,
    detail_map: Option<usize>,
    detail_normal_map: Option<usize>,
    sss_map: Option<usize>,
}

/// Classify a level material and queue the textures its shading needs beyond the standard ones.
pub fn plan(
    graph_path: &str,
    m: &ResolvedMaterial,
    banks: &ResourceBanks,
    keys: &mut TextureKeys,
) -> LevelPlan {
    let shading = LevelShading::classify(graph_path, m);
    let by_param = |name: &str| {
        m.textures
            .iter()
            .find(|t| t.param == name)
            .map(|t| t.id.clone())
    };
    let color = |keys: &mut TextureKeys, name: &str| {
        by_param(name).and_then(|id| keys.texture(banks, &id, |s| Usage::Color { srgb: s }))
    };
    let (detail_map, detail_normal_map) = if shading.detail {
        let d = color(keys, "MicroDetailMap");
        let n = by_param("MicroDetailNormalMap")
            .and_then(|id| keys.texture(banks, &id, |_| Usage::Normal));
        (d, n)
    } else {
        (None, None)
    };
    let sss_map = shading.subsurface_texture.and_then(|p| color(keys, p));
    let profile = matches!(shading.model, 3 | 4).then(|| {
        m.diffusion_profile
            .as_ref()
            .and_then(|id| banks.diffusion_profiles.get(id))
            .cloned()
            .unwrap_or_else(DiffusionProfile::engine_default)
    });
    LevelPlan {
        uniform: shading.uniform(m),
        shading,
        profile,
        detail_map,
        detail_normal_map,
        sss_map,
    }
}

/// The game-shaded material per level `StandardMaterial`.
#[derive(Resource, Default)]
pub struct LevelGameMaterials {
    by_standard: HashMap<AssetId<StandardMaterial>, Handle<LevelMaterial>>,
}

impl LevelGameMaterials {
    /// Build the game-shaded counterpart of `standard` (the material built for the plan's
    /// material). `glow` and `physical` are the standard plan's glow and physical maps.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        standard_handle: &Handle<StandardMaterial>,
        standard: &StandardMaterial,
        plan: &LevelPlan,
        glow: Option<Handle<Image>>,
        physical: Option<Handle<Image>>,
        images: &[Option<Handle<Image>>],
        sss_slot: impl FnOnce(&DiffusionProfile) -> usize,
        materials: &mut Assets<LevelMaterial>,
    ) {
        let get = |i: Option<usize>| i.and_then(|i| images[i].clone());
        let s = &plan.shading;
        let mut v = plan.uniform.map(Vec4::from_array);
        if let Some(p) = &plan.profile {
            // Transmission is on for every level subsurface graph (target 3's alpha is 1) and
            // forced on for translucent ones (model 4).
            let block = p.uniform(sss_slot(p), true);
            for (i, b) in block.into_iter().enumerate() {
                v[6 + i] = Vec4::from_array(b);
            }
        }
        let detail_map = get(plan.detail_map);
        let detail_normal_map = get(plan.detail_normal_map);
        let mut flags = s.model;
        if s.model == 3 && crate::sss::enabled() {
            flags |= FLAG_SSS_MARK;
        }
        if detail_map.is_some() && detail_normal_map.is_some() {
            flags |= FLAG_DETAIL;
        }
        if std::env::var_os("BG3_LEVEL_BEVY_LIGHTING").is_some() {
            flags |= FLAG_BEVY_LIGHTING;
        }
        let sss_map = match s.subsurface {
            SubsurfaceInput::PhysicalAlpha => physical,
            _ => get(plan.sss_map),
        };
        let base = StandardMaterial {
            emissive: LinearRgba::BLACK,
            emissive_texture: None,
            ..standard.clone()
        };
        let handle = materials.add(LevelMaterial {
            base,
            extension: LevelExtension {
                uniform: LevelUniform {
                    flags,
                    glow: s.glow.index(),
                    subsurface: s.subsurface.index(),
                    _pad: 0,
                    v,
                },
                detail_map,
                detail_normal_map,
                glow_map: glow,
                sss_map,
            },
        });
        self.by_standard.insert(standard_handle.id(), handle);
    }
}

/// Put the game-shaded material on every entity spawned with a level `StandardMaterial`.
fn swap_level_materials(
    mut commands: Commands,
    game: Res<LevelGameMaterials>,
    spawned: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        Added<MeshMaterial3d<StandardMaterial>>,
    >,
) {
    if game.by_standard.is_empty() || !enabled() {
        return;
    }
    let mut swapped = 0;
    for (entity, material) in &spawned {
        if let Some(h) = game.by_standard.get(&material.id()) {
            commands
                .entity(entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(h.clone()));
            swapped += 1;
        }
    }
    if swapped > 0 && std::env::var_os("BG3_LEVEL_DEBUG").is_some() {
        eprintln!("level materials: {swapped} meshes on the game's shading");
    }
}
