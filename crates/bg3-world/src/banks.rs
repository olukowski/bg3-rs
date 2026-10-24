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
    /// The low-resolution cloth simulation mesh of a `ClothProxyMapping` entry (a `MapKey`).
    /// It drives the mapped high-resolution meshes and is not drawn.
    pub cloth_proxy: bool,
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
    pub needs_skeleton_remap: bool,
    /// `SkeletonSlot`: visuals naming the same slot share one skeleton. Heads register
    /// slot `1` with their face rig; beards name slot `1` to pose against it.
    // bg3: ls::Visual::GetOrCreateSkeletonSlot (4.1.1.7631656)
    pub skeleton_slot: Option<String>,
}

impl VisualResource {
    /// The LOD0 meshes that are drawn: every LOD0 object except cloth simulation proxies.
    ///
    /// A visual's `ClothProxyMapping` maps each cloth proxy object (`MapKey`, a coarse mesh
    /// with the engine's default grey material `Engine_Default_Mesh_Lit_Opaque`) to the
    /// high-resolution meshes it drives (`ClosestVertices`). The GR2 marks the same meshes
    /// with a cloth proxy type (`UserMeshProperties.Flags[2]` bits 8–9, as read by
    /// `ls.fs.Gr2.GetClothProxyType`, 4.1.1.7631656). **Assumption** (no runtime trace): the
    /// engine never draws the proxy; the reference captures show no proxy surface.
    // bg3: ls::VisualResource::ClothInfo::Visit (4.1.1.7631656)
    pub fn drawn_lod0_objects(&self) -> impl Iterator<Item = &MeshObject> {
        self.objects.iter().filter(|o| o.lod == 0 && !o.cloth_proxy)
    }

    /// The drawn meshes of every LOD (every object except cloth simulation proxies, see
    /// [`Self::drawn_lod0_objects`]).
    pub fn drawn_objects(&self) -> impl Iterator<Item = &MeshObject> {
        self.objects.iter().filter(|o| !o.cloth_proxy)
    }
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
    /// `DiffusionProfileUUID` when set (replaces the graph's).
    pub diffusion_profile: Option<String>,
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
    /// `DiffusionProfileBank` resources by ID (subsurface scattering and transmission).
    pub diffusion_profiles: HashMap<String, bg3_material::character::sss::DiffusionProfile>,
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
    diffusion_profiles: Vec<bg3_material::character::sss::DiffusionProfile>,
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
        for p in file.diffusion_profiles {
            self.diffusion_profiles.insert(p.id.clone(), p);
        }
    }
}

fn resources<'a>(res: &'a bg3_lsf::Resource, region: &str) -> impl Iterator<Item = &'a Node> {
    res.region(region)
        .into_iter()
        .flat_map(|n| n.children.iter().filter(|c| c.name == "Resource"))
}

/// One `VisualBank` resource.
fn read_visual(n: &Node) -> VisualResource {
    // `ClothProxyMapping/Object/@MapKey`: the cloth proxy object IDs.
    let proxies: Vec<String> = n
        .children_named("ClothProxyMapping")
        .flat_map(|m| m.children_named("Object"))
        .filter_map(|o| str_attr(o, "MapKey"))
        .collect();
    let objects = n
        .children_named("Objects")
        .map(|o| {
            let object_id = str_attr(o, "ObjectID").unwrap_or_default();
            MeshObject {
                cloth_proxy: proxies.contains(&object_id),
                object_id,
                material_id: str_attr(o, "MaterialID"),
                lod: o.attr("LOD").and_then(|v| v.as_i64()).unwrap_or(0) as u8,
            }
        })
        .collect();
    VisualResource {
        id: str_attr(n, "ID").unwrap_or_default(),
        name: str_attr(n, "Name").unwrap_or_default(),
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
        needs_skeleton_remap: n
            .attr("NeedsSkeletonRemap")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        skeleton_slot: str_attr(n, "SkeletonSlot"),
        supports_vertex_color_mask: n
            .attr("SupportsVertexColorMask")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

fn read_bank_file(res: &bg3_lsf::Resource) -> BankFile {
    let id_name = |n: &Node| {
        (
            str_attr(n, "ID").unwrap_or_default(),
            str_attr(n, "Name").unwrap_or_default(),
        )
    };
    let visuals = resources(res, "VisualBank").map(read_visual).collect();
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
                diffusion_profile: str_attr(n, "DiffusionProfileUUID").filter(|s| !s.is_empty()),
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
    let diffusion_profiles = resources(res, "DiffusionProfileBank")
        .filter_map(bg3_material::character::sss::DiffusionProfile::read)
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
        diffusion_profiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_lsf::Value;

    fn fs(s: &str) -> Value {
        Value::FixedString(s.to_owned())
    }

    fn object(id: &str, lod: u8) -> Node {
        Node::new("Objects")
            .with_attr("ObjectID", fs(id))
            .with_attr("MaterialID", fs("mat"))
            .with_attr("LOD", Value::Byte(lod))
    }

    #[test]
    fn cloth_proxies_are_not_drawn() {
        let mapping = |proxy: &str, high: &str| {
            Node::new("Object")
                .with_attr("MapKey", fs(proxy))
                .with_child(
                    Node::new("MapValue")
                        .with_child(Node::new("Object").with_attr("Name", fs(high))),
                )
        };
        let n = Node::new("Resource")
            .with_attr("ID", fs("v"))
            .with_child(
                Node::new("ClothProxyMapping")
                    .with_child(mapping("M.Skirt_Cloth_Mesh.1", "M.Skirt_Mesh.0"))
                    .with_child(mapping("M.Skirt_Cloth_Mesh_LOD1.3", "M.Skirt_Mesh_LOD1.2")),
            )
            .with_child(object("M.Skirt_Mesh.0", 0))
            .with_child(object("M.Skirt_Cloth_Mesh.1", 0))
            .with_child(object("M.Skirt_Mesh_LOD1.2", 1))
            .with_child(object("M.Skirt_Cloth_Mesh_LOD1.3", 1));
        let v = read_visual(&n);
        assert_eq!(v.objects.len(), 4);
        let proxies: Vec<_> = v
            .objects
            .iter()
            .filter(|o| o.cloth_proxy)
            .map(|o| o.mesh_name())
            .collect();
        assert_eq!(proxies, ["Skirt_Cloth_Mesh", "Skirt_Cloth_Mesh_LOD1"]);
        let drawn: Vec<_> = v.drawn_lod0_objects().map(|o| o.mesh_name()).collect();
        assert_eq!(drawn, ["Skirt_Mesh"]);
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn character_diffusion_profiles_load() {
        let Ok(install) = bg3_install::Install::locate() else {
            eprintln!("skipping: no BG3 install found");
            return;
        };
        let vfs = bg3_vfs::Vfs::from_install(&install).unwrap();
        let banks = ResourceBanks::load(&vfs).unwrap();
        let profiles = &banks.diffusion_profiles;
        let default = &profiles[bg3_material::character::sss::ENGINE_DEFAULT_PROFILE];
        assert_eq!(default.scatter_distance, 0.0);
        // CHAR_SHARED_Skin_SS, the skin graphs' profile.
        let skin = &profiles["3428c9e0-de61-cc06-2476-4c56b84d473d"];
        assert_eq!(skin.name, "CHAR_SHARED_Skin_SS");
        assert_eq!(skin.scatter_distance, 8.0);
        assert!(skin.thick_object);
        assert_eq!(skin.thickness_remap, [0.0, 30.0]);
        assert_eq!(skin.scatter_color[0], 1.0);
        assert_eq!(skin.scatter_color[2], 0.0);
    }
}
