//! Resource banks: visual, material and texture resources indexed by ID.
//!
//! Banks live in `_merged.lsf` files under `Public/<Module>/Content/**` (regions `VisualBank`,
//! `MaterialBank`, `TextureBank`, …). Later modules override earlier IDs.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;
use rayon::prelude::*;

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

#[derive(Debug, Clone)]
pub struct VisualResource {
    pub id: String,
    pub name: String,
    /// GR2 path in the VFS.
    pub source_file: Option<String>,
    pub objects: Vec<MeshObject>,
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
}

#[derive(Debug, Clone)]
pub struct TextureResource {
    pub id: String,
    pub name: String,
    pub source_file: Option<String>,
}

#[derive(Debug, Default)]
pub struct ResourceBanks {
    pub visuals: HashMap<String, VisualResource>,
    pub materials: HashMap<String, MaterialResource>,
    pub textures: HashMap<String, TextureResource>,
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
}

impl ResourceBanks {
    /// Scan every `_merged.lsf` under `Public/<Module>/` for each module in load order.
    pub fn load(vfs: &Vfs) -> Result<Self> {
        Self::load_modules(vfs, BASE_LOAD_ORDER)
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
            }
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
    }
}
