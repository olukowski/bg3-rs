//! Resource banks: visual, material and texture resources indexed by ID.
//!
//! Banks live in `_merged.lsf` files under `Public/<Module>/Content/**` (regions `VisualBank`,
//! `MaterialBank`, `TextureBank`, …). Later modules override earlier IDs.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;
use rayon::prelude::*;

use crate::characters::{
    AnimationResource, AnimationSetResource, CharacterVisualResource, SkeletonResource,
};
use crate::{Result, parse, str_attr};

/// One mesh of a visual: `ObjectID` is `<model>.<mesh>.<n>`.
#[derive(Debug, Clone)]
pub struct MeshObject {
    pub object_id: String,
    pub material_id: Option<String>,
    pub lod: u8,
}

impl MeshObject {
    /// The GR2 mesh name (middle component of `ObjectID`).
    pub fn mesh_name(&self) -> &str {
        let mut parts = self.object_id.split('.');
        parts.next();
        parts.next().unwrap_or("")
    }
}

#[derive(Debug, Clone, Default)]
pub struct VisualResource {
    pub id: String,
    pub name: String,
    /// GR2 path in the VFS.
    pub source_file: Option<String>,
    pub objects: Vec<MeshObject>,
    /// `SkeletonResource` → `SkeletonBank` (set on character base visuals).
    pub skeleton_resource: Option<String>,
    /// `AnimationWaterfall` → `AnimationSetBank` IDs, in file order.
    pub animation_waterfall: Vec<String>,
    /// `Slot` (`NakedBody`, `Head`, `Unassigned`, …).
    pub slot: Option<String>,
    /// `AttachBone`: bone a rigid visual attaches to.
    pub attach_bone: Option<String>,
    /// Named body regions this visual asks to cut from underlying meshes.
    pub vertex_color_mask_slots: Vec<String>,
    pub supports_vertex_color_mask: bool,
}

#[derive(Debug, Clone)]
pub struct MaterialResource {
    pub id: String,
    pub name: String,
    /// Shader-graph resource path.
    pub source_file: Option<String>,
    /// `(parameter name, texture ID)` for enabled 2D texture parameters.
    pub textures: Vec<(String, String)>,
    /// `(parameter name, value)` for scalar parameters.
    pub scalars: Vec<(String, f64)>,
    /// Every parameter with its `Enabled` (overrides-the-graph) flag.
    pub params: Vec<bg3_material::Param>,
}

#[derive(Debug, Clone)]
pub struct TextureResource {
    pub id: String,
    pub name: String,
    pub source_file: Option<String>,
    /// The `SRGB` flag: colour data to be decoded from sRGB.
    pub srgb: bool,
    /// `Type`: 1 = 2D, 2 = cube map (observed), 3 = 3D.
    pub kind: i32,
}

/// A Granite virtual texture (`VirtualTextureBank`); see `bg3-vt`.
pub use bg3_vt::VirtualTextureResource;

#[derive(Debug, Default)]
pub struct ResourceBanks {
    pub visuals: HashMap<String, VisualResource>,
    pub materials: HashMap<String, MaterialResource>,
    pub textures: HashMap<String, TextureResource>,
    pub virtual_textures: HashMap<String, VirtualTextureResource>,
    /// Character visuals, skeletons, animations and animation sets.
    pub characters: crate::characters::CharacterBanks,
    pub equipment: crate::characters::EquipmentSettings,
    /// Equipment types and weapon animation sets (drawn/sheathed weapons, `unsheath.rs`).
    pub weapons: crate::unsheath::WeaponData,
    /// `MaterialPresetBank` resources (their `Presets` node): colour/skin/hair/eye presets
    /// that character visuals and items reference (see `appearance.rs`).
    pub material_presets: HashMap<String, crate::characters::MaterialOverrides>,
    /// Bank region (`EffectBank`, `SkeletonBank`, …) of every resource ID, including the typed ones.
    pub kinds: HashMap<String, String>,
    /// Bank files that failed to parse (path, error).
    pub errors: Vec<(String, String)>,
}

struct BankFile {
    kinds: Vec<(String, String)>,
    visuals: Vec<VisualResource>,
    materials: Vec<MaterialResource>,
    textures: Vec<TextureResource>,
    virtual_textures: Vec<VirtualTextureResource>,
    character_visuals: Vec<CharacterVisualResource>,
    skeletons: Vec<SkeletonResource>,
    animations: Vec<AnimationResource>,
    animation_sets: Vec<AnimationSetResource>,
    material_presets: Vec<(String, crate::characters::MaterialOverrides)>,
}

impl ResourceBanks {
    /// Scan every `_merged.lsf` under `Public/<Module>/` for each module in load order.
    ///
    /// The engine's own banks (`Public/Engine/`, e.g. default materials that level meshes
    /// reference) come first.
    pub fn load(vfs: &Vfs) -> Result<Self> {
        let modules: Vec<&str> = std::iter::once("Engine")
            .chain(BASE_LOAD_ORDER.iter().copied())
            .collect();
        Self::load_modules(vfs, &modules)
    }

    pub fn load_modules(vfs: &Vfs, modules: &[&str]) -> Result<Self> {
        let mut banks = Self::default();
        for module in modules {
            let prefix = format!("Public/{module}/");
            let mut paths: Vec<String> = vfs
                .files_under(&prefix)
                .map(|f| f.entry.name.clone())
                .filter(|n| n.ends_with("_merged.lsf") && !n.contains("/RootTemplates/"))
                .collect();
            // Deterministic override order within a module.
            paths.sort_by_key(|p| p.to_ascii_lowercase());
            let parsed: Vec<(String, std::result::Result<BankFile, String>)> = paths
                .into_par_iter()
                .map(|path| {
                    let res = parse(vfs, &path)
                        .map(|r| read_bank_file(&r))
                        .map_err(|e| e.to_string());
                    (path, res)
                })
                .collect();
            for (path, file) in parsed {
                match file {
                    Ok(file) => banks.insert(file),
                    Err(e) => banks.errors.push((path, e)),
                }
            }
        }
        banks.equipment = crate::characters::EquipmentSettings::load(vfs, modules)?;
        banks.weapons = crate::unsheath::WeaponData::load(vfs, modules)?;
        Ok(banks)
    }

    /// Which bank region a resource ID belongs to.
    pub fn kind(&self, id: &str) -> Option<&str> {
        self.kinds.get(id).map(String::as_str)
    }

    fn insert(&mut self, file: BankFile) {
        self.kinds.extend(file.kinds);
        for v in file.visuals {
            self.visuals.insert(v.id.clone(), v);
        }
        for m in file.materials {
            self.materials.insert(m.id.clone(), m);
        }
        for t in file.textures {
            self.textures.insert(t.id.clone(), t);
        }
        for t in file.virtual_textures {
            self.virtual_textures.insert(t.id.clone(), t);
        }
        let c = &mut self.characters;
        for r in file.character_visuals {
            c.character_visuals.insert(r.id.clone(), r);
        }
        for r in file.skeletons {
            c.skeletons.insert(r.id.clone(), r);
        }
        for r in file.animations {
            c.animations.insert(r.id.clone(), r);
        }
        for r in file.animation_sets {
            c.animation_sets.insert(r.id.clone(), r);
        }
        self.material_presets.extend(file.material_presets);
    }
}

fn resources<'a>(res: &'a bg3_lsf::Resource, region: &str) -> impl Iterator<Item = &'a Node> {
    res.region(region)
        .into_iter()
        .flat_map(|n| n.children.iter().filter(|c| c.name == "Resource"))
}

fn read_bank_file(res: &bg3_lsf::Resource) -> BankFile {
    let id_name = |n: &Node| {
        (
            str_attr(n, "ID").unwrap_or_default(),
            str_attr(n, "Name").unwrap_or_default(),
        )
    };
    let visuals = resources(res, "VisualBank")
        .map(|n| {
            let (id, name) = id_name(n);
            let objects = n
                .children
                .iter()
                .filter(|c| c.name == "Objects")
                .map(|o| MeshObject {
                    object_id: str_attr(o, "ObjectID").unwrap_or_default(),
                    material_id: str_attr(o, "MaterialID"),
                    lod: o.attr("LOD").and_then(|v| v.as_i64()).unwrap_or(0) as u8,
                })
                .collect();
            VisualResource {
                id,
                name,
                source_file: str_attr(n, "SourceFile"),
                objects,
                skeleton_resource: str_attr(n, "SkeletonResource"),
                animation_waterfall: n
                    .children_named("AnimationWaterfall")
                    .filter_map(|w| str_attr(w, "Object"))
                    .collect(),
                slot: str_attr(n, "Slot"),
                attach_bone: str_attr(n, "AttachBone"),
                vertex_color_mask_slots: n
                    .children_named("VertexColorMaskSlots")
                    .filter_map(|c| str_attr(c, "Object"))
                    .collect(),
                supports_vertex_color_mask: n
                    .attr("SupportsVertexColorMask")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            }
        })
        .collect();
    let materials = resources(res, "MaterialBank")
        .map(|n| {
            let (id, name) = id_name(n);
            let enabled = |c: &Node| c.attr("Enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let textures = n
                .children
                .iter()
                .filter(|c| c.name == "Texture2DParameters" && enabled(c))
                .filter_map(|c| Some((str_attr(c, "ParameterName")?, str_attr(c, "ID")?)))
                .collect();
            let scalars = n
                .children
                .iter()
                .filter(|c| c.name == "ScalarParameters")
                .filter_map(|c| Some((str_attr(c, "ParameterName")?, c.attr("Value")?.as_f64()?)))
                .collect();
            MaterialResource {
                id,
                name,
                source_file: str_attr(n, "SourceFile"),
                textures,
                scalars,
                params: bg3_material::read_params(n),
            }
        })
        .collect();
    let textures = resources(res, "TextureBank")
        .map(|n| {
            let (id, name) = id_name(n);
            TextureResource {
                id,
                name,
                source_file: str_attr(n, "SourceFile"),
                srgb: n.attr("SRGB").and_then(|v| v.as_bool()).unwrap_or(false),
                kind: n.attr("Type").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
            }
        })
        .collect();
    let virtual_textures = VirtualTextureResource::read_bank(res);
    let character_visuals = resources(res, "CharacterVisualBank")
        .map(CharacterVisualResource::read)
        .collect();
    let skeletons = resources(res, "SkeletonBank")
        .map(SkeletonResource::read)
        .collect();
    let animations = resources(res, "AnimationBank")
        .map(AnimationResource::read)
        .collect();
    let animation_sets = resources(res, "AnimationSetBank")
        .map(AnimationSetResource::read)
        .collect();
    let material_presets = resources(res, "MaterialPresetBank")
        .filter_map(|n| {
            let presets = n.children.iter().find(|c| c.name == "Presets")?;
            Some((
                str_attr(n, "ID")?,
                crate::characters::MaterialOverrides::read(presets),
            ))
        })
        .collect();
    let kinds = res
        .regions
        .iter()
        .filter(|r| r.name.ends_with("Bank"))
        .flat_map(|r| {
            resources(res, &r.name).filter_map(|n| Some((str_attr(n, "ID")?, r.name.clone())))
        })
        .collect();
    BankFile {
        kinds,
        visuals,
        materials,
        textures,
        virtual_textures,
        character_visuals,
        skeletons,
        animations,
        animation_sets,
        material_presets,
    }
}
