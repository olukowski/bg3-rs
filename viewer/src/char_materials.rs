//! Character materials: BG3's skin, eye, hair, eyelash and clothing-palette graphs as a Bevy
//! `ExtendedMaterial` (`shaders/character.wesl`).
//!
//! Materials are resolved once per material ID ([`plan`] while textures load, then
//! [`CharacterTemplates`]), and instantiated per character at spawn with that character's
//! parameter overrides ([`CharacterTemplates::instantiate`]). The math lives in
//! `bg3_material::character`; the research is `docs/research/character-shaders.md`.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::embedded_asset;
use bevy::mesh::Indices;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bg3_material::character::sss::{DiffusionProfile, SssProfiles};
use bg3_material::character::{CharKind, ParamSet, SkinKind, UNIFORM_LEN, vertex_cut};
use bg3_material::{BlendMode, ResolvedMaterial};
use bg3_world::ResourceBanks;
use bg3_world::appearance::{ParamAssignment, assignments};
use bg3_world::characters::{EquippedItem, MaterialOverrides, ResolvedCharacter};

use crate::materials::{TextureKeys, Usage};

const SHADER: &str = "embedded://bg3_viewer/shaders/character.wesl";

/// `CharacterUniform::flags` bits above the shading model (bits 0–2), as in the shader.
const FLAG_SSS_MARK: u32 = 8;
const FLAG_BEVY_LIGHTING: u32 = 256;

pub type CharacterMaterial = ExtendedMaterial<StandardMaterial, CharacterExtension>;

#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct CharacterUniform {
    pub mode: u32,
    pub flags: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub v: [Vec4; UNIFORM_LEN],
}

/// The extension's bindings: a uniform block and up to eight textures whose meaning depends on
/// the mode (see the shader).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CharacterExtension {
    #[uniform(100)]
    pub uniform: CharacterUniform,
    #[texture(101)]
    #[sampler(102)]
    pub t0: Option<Handle<Image>>,
    #[texture(103)]
    pub t1: Option<Handle<Image>>,
    #[texture(104)]
    pub t2: Option<Handle<Image>>,
    #[texture(105)]
    pub t3: Option<Handle<Image>>,
    #[texture(106)]
    pub t4: Option<Handle<Image>>,
    #[texture(107)]
    pub t5: Option<Handle<Image>>,
    #[texture(108)]
    pub t6: Option<Handle<Image>>,
    #[texture(109)]
    pub t7: Option<Handle<Image>>,
}

impl MaterialExtension for CharacterExtension {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn deferred_fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

pub struct CharacterMaterialsPlugin;

impl Plugin for CharacterMaterialsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/character.wesl");
        app.add_plugins(MaterialPlugin::<CharacterMaterial>::default());
    }
}

/// Texture slots of a character material, as indices into the prepared image list.
#[derive(Debug, Clone)]
pub struct CharPlan {
    kind: CharKind,
    /// G-buffer shading model (0 lit, 3 subsurface, 5 hair).
    model: u32,
    /// The diffusion profile of a subsurface material (default profile when unresolved).
    profile: Option<DiffusionProfile>,
    params: ParamSet,
    textures: [Option<usize>; 8],
    normal: Option<usize>,
    blend: BlendMode,
    two_sided: bool,
}

/// Decide whether a material gets character shading and queue its textures.
pub fn plan(
    graph_path: &str,
    m: &ResolvedMaterial,
    banks: &ResourceBanks,
    keys: &mut TextureKeys,
) -> Option<CharPlan> {
    let kind = CharKind::classify(graph_path, m)?;
    let by_param = |name: &str| {
        m.textures
            .iter()
            .find(|t| t.param == name)
            .map(|t| t.id.clone())
    };
    // Unnamed graph inputs, identified by their default texture's resource name.
    let by_bank_name = |prefix: &str| {
        m.textures
            .iter()
            .filter(|t| t.param.is_empty())
            .find(|t| {
                banks
                    .textures
                    .get(&t.id)
                    .is_some_and(|r| r.name.starts_with(prefix))
            })
            .map(|t| t.id.clone())
    };
    let mut color = |id: Option<String>| {
        id.and_then(|id| keys.texture(banks, &id, |s| Usage::Color { srgb: s }))
    };
    let mut textures: [Option<usize>; 8] = [None; 8];
    let vt = m
        .virtual_textures
        .first()
        .filter(|v| banks.virtual_textures.contains_key(*v));
    let normal_tex = |keys: &mut TextureKeys, name: &str| {
        by_param(name).and_then(|id| keys.texture(banks, &id, |_| Usage::Normal))
    };
    let normal = match kind {
        CharKind::Skin(SkinKind::Body) => {
            let vt = vt?;
            textures[2] = color(by_param("Thickness"));
            textures[3] = color(by_param("CancelMSK"));
            textures[4] = color(by_param("BaseColor_NonSkin"));
            textures[5] = color(by_param("TattooAtlas"));
            textures[0] = Some(keys.add(vt.clone(), Usage::VirtualLayerRaw(2)));
            textures[1] = Some(keys.add(vt.clone(), Usage::VirtualLayer(0)));
            Some(keys.add(vt.clone(), Usage::VirtualLayer(1)))
        }
        CharKind::Skin(SkinKind::Head) => {
            textures[0] = color(by_param("HMVY"));
            textures[1] = color(by_param("CLEA"));
            textures[2] = color(by_bank_name("SKIN_Shared_Shading_MSK"));
            textures[3] = color(by_param("CancelMSK"));
            textures[4] = color(by_param("BaseColor_NonSkin"));
            textures[5] = color(by_param("TattooAtlas"));
            textures[6] = color(by_param("MakeUpAtlas"));
            textures[7] = color(by_bank_name("SKIN_Shared_Dirt_Head_MSK"));
            normal_tex(keys, "normalmap")
        }
        CharKind::Eye { .. } => {
            textures[0] = color(by_param("RegionMask"));
            textures[1] = color(by_param("ColorsMask"));
            textures[2] = color(by_bank_name("EYES_Blindness"));
            textures[3] = color(by_bank_name("ATM_HDRi_EyesCatchlight"));
            textures[4] = color(by_bank_name("EYES_Sclera_NM"));
            normal_tex(keys, "IrisNormal")
        }
        CharKind::Lashes => {
            textures[0] = color(by_param("ID_Depth_Root_Alpha_MSKA"));
            None
        }
        CharKind::Hair => {
            textures[0] = color(by_param("ID_Depth_Root_Alpha_MSKA"));
            textures[1] = color(by_bank_name("HAIR_Splatter"));
            textures[2] = color(by_bank_name("HAIR_ColorVar_LUT"));
            None
        }
        CharKind::ClothPalette => {
            textures[0] = color(by_param("MSKColor"));
            None
        }
        CharKind::ClothMsk => {
            textures[0] = color(by_param("MSKcloth"));
            None
        }
        CharKind::Tearline => None,
    };
    // The graph's own shading model; the name-based guess only without a graph.
    let model = m
        .shading_model
        .map_or_else(|| kind.shading_model(graph_path), u32::from);
    let profile = (model == 3).then(|| {
        m.diffusion_profile
            .as_ref()
            .and_then(|id| banks.diffusion_profiles.get(id))
            .cloned()
            .unwrap_or_else(DiffusionProfile::engine_default)
    });
    Some(CharPlan {
        kind,
        model,
        profile,
        params: ParamSet::from_material(m),
        textures,
        normal,
        blend: m.blend,
        two_sided: m.two_sided,
    })
}

struct CharTemplate {
    kind: CharKind,
    model: u32,
    /// Lighting block (`v[20..24]`) of subsurface materials.
    profile_block: Option<[[f32; 4]; 4]>,
    params: ParamSet,
    base: StandardMaterial,
    textures: [Option<Handle<Image>>; 8],
}

/// Character material templates by material ID.
#[derive(Resource, Default)]
pub struct CharacterTemplates {
    by_id: HashMap<String, CharTemplate>,
    /// Kernel slots of the subsurface profiles the materials use.
    sss: SssProfiles,
}

/// The material a character mesh part is drawn with.
pub enum PartMaterial {
    Character(Handle<CharacterMaterial>),
    /// Drawn by the game but not here (see [`CharKind::Tearline`]).
    Hidden,
}

impl CharacterTemplates {
    /// Register material `id`. `standard` is the generic material built for it, the base for
    /// clothing; skin/eye/lash/hair surfaces replace its maps.
    pub fn add(
        &mut self,
        id: &str,
        plan: &CharPlan,
        standard: &StandardMaterial,
        images: &[Option<Handle<Image>>],
    ) {
        let get = |i: Option<usize>| i.and_then(|i| images[i].clone());
        let mut base = standard.clone();
        match plan.kind {
            CharKind::ClothPalette | CharKind::ClothMsk => {}
            _ => {
                base = StandardMaterial {
                    base_color: Color::WHITE,
                    normal_map_texture: get(plan.normal),
                    flip_normal_map_y: false,
                    double_sided: plan.two_sided,
                    cull_mode: standard.cull_mode,
                    alpha_mode: match (plan.kind, plan.blend) {
                        (CharKind::Lashes, _) | (_, BlendMode::Blend) => AlphaMode::Blend,
                        // The game's depth prepass keeps hair where coverage ≥ 1/3 (no TAA).
                        (CharKind::Hair, _) => AlphaMode::Mask(1.0 / 3.0),
                        (_, BlendMode::AlphaTest) => AlphaMode::Mask(0.5),
                        _ => AlphaMode::Opaque,
                    },
                    ..default()
                };
            }
        }
        // Transmission is on for every character subsurface graph surveyed (their G-buffer
        // target 3 alpha is a constant 1).
        let profile_block = plan
            .profile
            .as_ref()
            .map(|p| p.uniform(self.sss.slot(p), true));
        self.by_id.insert(
            id.to_owned(),
            CharTemplate {
                kind: plan.kind,
                model: plan.model,
                profile_block,
                params: plan.params.clone(),
                base,
                textures: plan.textures.map(get),
            },
        );
    }

    /// The material for `id` with `overrides` applied in order (each to the parameters the
    /// material declares), or `None` if `id` isn't a character material.
    pub fn instantiate(
        &self,
        id: &str,
        overrides: &[ParamAssignment],
        materials: &mut Assets<CharacterMaterial>,
    ) -> Option<PartMaterial> {
        let t = self.by_id.get(id)?;
        if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
            eprintln!(
                "character material {id}: {:?}, {} overrides",
                t.kind,
                overrides.len()
            );
        }
        if t.kind == CharKind::Tearline {
            return Some(PartMaterial::Hidden);
        }
        let mut params = t.params.clone();
        for o in overrides {
            params.set_declared(&o.parameter, &o.value);
        }
        let mut v = t.kind.uniform(&params);
        if let Some(block) = &t.profile_block {
            v[UNIFORM_LEN - 4..].copy_from_slice(block);
        }
        if matches!(t.kind, CharKind::Eye { .. }) {
            // The sclera detail normal applies only when its texture is bound.
            v[11][3] = if t.textures[4].is_some() { 1.0 } else { 0.0 };
        }
        let v = v.map(Vec4::from_array);
        let mut flags = t.model;
        if t.model == 3 && crate::sss::enabled() {
            flags |= FLAG_SSS_MARK;
        }
        if std::env::var_os("BG3_CHAR_BEVY_LIGHTING").is_some() {
            flags |= FLAG_BEVY_LIGHTING;
        }
        let [t0, t1, t2, t3, t4, t5, t6, t7] = t.textures.clone();
        Some(PartMaterial::Character(materials.add(CharacterMaterial {
            base: t.base.clone(),
            extension: CharacterExtension {
                uniform: CharacterUniform {
                    mode: t.kind.mode(),
                    flags,
                    _pad0: 0,
                    _pad1: 0,
                    v,
                },
                t0,
                t1,
                t2,
                t3,
                t4,
                t5,
                t6,
                t7,
            },
        })))
    }

    /// The kernel slot of a subsurface profile (level materials share the character slots).
    pub fn sss_slot(&mut self, profile: &DiffusionProfile) -> usize {
        self.sss.slot(profile)
    }

    /// The subsurface profiles in kernel-slot order.
    pub fn sss_profiles(&self) -> &SssProfiles {
        &self.sss
    }

    /// Whether material `id` reads vertex colours in its fragment shader (eyes: left/right;
    /// hair: graying seed, highlight, root occlusion).
    pub fn needs_vertex_colors(&self, id: &str) -> bool {
        self.by_id
            .get(id)
            .is_some_and(|t| matches!(t.kind, CharKind::Eye { .. } | CharKind::Hair))
    }
}

/// What a character mesh part needs besides its mesh and material ID.
#[derive(Debug, Clone, Default)]
pub struct PartLook {
    /// Parameter overrides in application order (item's, then the character's).
    pub overrides: Arc<Vec<ParamAssignment>>,
    /// First vertex-colour set (cut mask in green, eye side in red).
    pub vertex_colors: Option<Vec<[f32; 4]>>,
    /// The visual supports body cut masks (`SupportsVertexColorMask`).
    pub cuttable: bool,
}

/// The override layers of one character: its visual's, and each worn item's.
pub struct LookOverrides<'a> {
    character: Arc<Vec<ParamAssignment>>,
    items: Vec<(&'a EquippedItem, Vec<ParamAssignment>)>,
}

impl<'a> LookOverrides<'a> {
    pub fn new(
        c: &ResolvedCharacter,
        items: &'a [EquippedItem],
        presets: &HashMap<String, MaterialOverrides>,
    ) -> Self {
        Self {
            character: Arc::new(assignments(&c.character_visual.material_overrides, presets)),
            items: items
                .iter()
                .map(|i| (i, assignments(&i.material_overrides, presets)))
                .collect(),
        }
    }

    /// Overrides for a part of `visual`: the worn item's own (if the visual belongs to one),
    /// then the character's, which win (**assumption**: Lae'zel's visual carries the colour
    /// preset of her armour).
    pub fn part(&self, visual: &bg3_world::VisualResource, mesh: &bg3_gr2::Mesh) -> PartLook {
        let item = self
            .items
            .iter()
            .find(|(i, _)| i.worn.iter().any(|w| w.visual.id == visual.id));
        let overrides = match item {
            Some((_, own)) if !own.is_empty() => {
                Arc::new(own.iter().chain(self.character.iter()).cloned().collect())
            }
            _ => self.character.clone(),
        };
        PartLook {
            overrides,
            vertex_colors: mesh.colors.first().cloned(),
            cuttable: visual.supports_vertex_color_mask,
        }
    }
}

/// A part's material, of either kind.
pub enum SpawnMaterial {
    Standard(Handle<StandardMaterial>),
    Character(Handle<CharacterMaterial>),
}

impl SpawnMaterial {
    pub fn insert(self, e: &mut EntityCommands) {
        match self {
            Self::Standard(h) => e.insert(MeshMaterial3d(h)),
            Self::Character(h) => e.insert(MeshMaterial3d(h)),
        };
    }
}

/// Choose the material of a character mesh part and prepare its mesh for it: apply the
/// character's body cuts and keep vertex colours only where the shader reads them. `None`
/// for parts the game doesn't draw.
pub fn part_material(
    part: &mut crate::characters::PreparedPart,
    cut_bytes: &[u8],
    templates: Option<&CharacterTemplates>,
    char_materials: &mut Assets<CharacterMaterial>,
    level: &crate::materials::LevelMaterials,
    fallback: &Handle<StandardMaterial>,
) -> Option<SpawnMaterial> {
    let look = std::mem::take(&mut part.look);
    if look.cuttable
        && !cut_bytes.is_empty()
        && let Some(colors) = &look.vertex_colors
    {
        apply_cut(&mut part.mesh, colors, cut_bytes);
    }
    let id = part.material_id.as_deref();
    // `BG3_STANDARD_CHARACTERS=1`: the generic StandardMaterial path, for A/B comparisons.
    let templates = templates.filter(|_| std::env::var_os("BG3_STANDARD_CHARACTERS").is_none());
    if let (Some(t), Some(id)) = (templates, id) {
        if t.needs_vertex_colors(id)
            && let Some(colors) = &look.vertex_colors
        {
            part.mesh
                .insert_attribute(Mesh::ATTRIBUTE_COLOR, colors.clone());
        }
        match t.instantiate(id, &look.overrides, char_materials) {
            Some(PartMaterial::Character(h)) => return Some(SpawnMaterial::Character(h)),
            Some(PartMaterial::Hidden) => return None,
            None => {}
        }
    }
    match id.map(|m| level.by_id.get(m)) {
        Some(Some(Some(m))) => Some(SpawnMaterial::Standard(m.clone())),
        Some(Some(None)) => None,
        _ => Some(SpawnMaterial::Standard(fallback.clone())),
    }
}

/// Drop the triangles the game collapses for `cut_bytes` (see
/// `bg3_material::character::vertex_cut`).
fn apply_cut(mesh: &mut Mesh, colors: &[[f32; 4]], cut_bytes: &[u8]) {
    let mask = vertex_cut::mask_texture(cut_bytes.iter().copied());
    let cut: Vec<bool> = colors
        .iter()
        .map(|c| vertex_cut::is_cut(&mask, c[1]))
        .collect();
    if !cut.contains(&true) {
        return;
    }
    let Some(indices) = mesh.indices_mut() else {
        return;
    };
    let mut idx: Vec<u32> = indices.iter().map(|i| i as u32).collect();
    let (removed, partial) = vertex_cut::cut_triangles(&mut idx, &cut);
    if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
        eprintln!("vertex cut: {removed} triangles removed, {partial} partially cut kept");
    }
    *indices = Indices::U32(idx);
}
