//! Level contents: placed objects from `Mods/<Module>/Levels/<Level>/<Kind>/_merged.lsf`.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::{Error, Result, parse, str_attr};

/// Object kinds stored as per-level `_merged.lsf` directories.
pub const OBJECT_DIRS: &[&str] = &["Scenery", "Items", "Characters"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    /// Quaternion, `x y z w`.
    pub rotation: [f32; 4],
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LevelObject {
    /// Instance GUID (`MapKey`).
    pub id: String,
    pub name: String,
    /// `scenery`, `item`, `character`, …
    pub kind: String,
    /// Root template GUID.
    pub template: Option<String>,
    /// Instance-level visual override, if any.
    pub visual_template: Option<String>,
    pub transform: Transform,
    /// Character fields set on the object itself (`character` objects only).
    pub character: Option<crate::characters::CharacterProps>,
    /// Module the (winning) definition came from.
    pub module: String,
}

#[derive(Debug)]
pub struct Level {
    pub name: String,
    pub objects: Vec<LevelObject>,
}

impl Level {
    pub fn load(vfs: &Vfs, name: &str) -> Result<Self> {
        Self::load_modules(vfs, name, BASE_LOAD_ORDER)
    }

    /// Merge the level's objects across modules; a later module replaces objects by `MapKey`.
    pub fn load_modules(vfs: &Vfs, name: &str, modules: &[&str]) -> Result<Self> {
        let mut by_id: HashMap<String, usize> = HashMap::new();
        let mut objects: Vec<LevelObject> = Vec::new();
        let mut found = false;
        for module in modules {
            for dir in OBJECT_DIRS {
                let path = format!("Mods/{module}/Levels/{name}/{dir}/_merged.lsf");
                if !vfs.exists(&path) {
                    continue;
                }
                found = true;
                let res = parse(vfs, &path)?;
                for obj in read_objects(&res, module) {
                    match by_id.get(&obj.id) {
                        Some(&i) => objects[i] = obj,
                        None => {
                            by_id.insert(obj.id.clone(), objects.len());
                            objects.push(obj);
                        }
                    }
                }
            }
        }
        if !found {
            return Err(Error::LevelNotFound(name.to_owned()));
        }
        Ok(Self {
            name: name.to_owned(),
            objects,
        })
    }
}

/// Characters defined in any level's `Globals` directory
/// (`Mods/<Module>/Globals/<Level>/Characters/_merged.lsf`): they exist in every level and
/// move between levels (origins, companions). A later module replaces a character by `MapKey`.
pub fn load_global_characters(vfs: &Vfs) -> Result<Vec<LevelObject>> {
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut objects: Vec<LevelObject> = Vec::new();
    for module in BASE_LOAD_ORDER {
        let prefix = format!("Mods/{module}/Globals/");
        let mut paths: Vec<String> = vfs
            .files_under(&prefix)
            .map(|f| f.entry.name.clone())
            .filter(|n| n.ends_with("/Characters/_merged.lsf"))
            .collect();
        paths.sort();
        for path in paths {
            let res = parse(vfs, &path)?;
            for obj in read_objects(&res, module) {
                match by_id.get(&obj.id) {
                    Some(&i) => objects[i] = obj,
                    None => {
                        by_id.insert(obj.id.clone(), objects.len());
                        objects.push(obj);
                    }
                }
            }
        }
    }
    Ok(objects)
}

fn read_objects<'a>(
    res: &'a bg3_lsf::Resource,
    module: &'a str,
) -> impl Iterator<Item = LevelObject> + 'a {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children.iter().filter(|c| c.name == "GameObjects"))
        .filter_map(move |n| {
            let kind = str_attr(n, "Type").unwrap_or_default();
            Some(LevelObject {
                id: str_attr(n, "MapKey")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                character: (kind == "character")
                    .then(|| crate::characters::CharacterProps::read(n)),
                kind,
                template: str_attr(n, "TemplateName"),
                visual_template: str_attr(n, "VisualTemplate"),
                transform: n.child("Transform").map(read_transform).unwrap_or_default(),
                module: module.to_owned(),
            })
        })
}

pub(crate) fn read_transform(node: &Node) -> Transform {
    let vec = |name: &str| -> Option<Vec<f32>> {
        Some(
            node.attr(name)?
                .components()?
                .into_iter()
                .map(|x| x as f32)
                .collect(),
        )
    };
    let mut t = Transform::default();
    if let Some(p) = vec("Position").filter(|v| v.len() == 3) {
        t.position = [p[0], p[1], p[2]];
    }
    if let Some(r) = vec("RotationQuat").filter(|v| v.len() == 4) {
        t.rotation = [r[0], r[1], r[2], r[3]];
    }
    if let Some(s) = node.attr("Scale").and_then(|v| v.as_f64()) {
        t.scale = s as f32;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceBanks, RootTemplates};

    #[test]
    #[ignore = "needs BG3 install"]
    fn nautiloid_scenery_resolves_to_meshes() {
        let install = bg3_install::Install::locate().unwrap();
        let vfs = Vfs::from_install(&install).unwrap();
        let level = Level::load(&vfs, "TUT_Avernus_C").unwrap();
        let templates = RootTemplates::load(&vfs).unwrap();
        let banks = ResourceBanks::load(&vfs).unwrap();
        assert!(
            banks.errors.is_empty(),
            "{:?}",
            &banks.errors[..banks.errors.len().min(5)]
        );

        let scenery: Vec<_> = level
            .objects
            .iter()
            .filter(|o| o.kind == "scenery")
            .collect();
        let resolved = scenery
            .iter()
            .filter_map(|o| {
                let visual = o
                    .visual_template
                    .as_deref()
                    .or_else(|| templates.visual_for(o.template.as_deref()?))?;
                banks.visuals.get(visual)?.source_file.as_ref()
            })
            .filter(|gr2| vfs.exists(gr2))
            .count();
        eprintln!(
            "{} objects, {} scenery, {resolved} scenery resolved to existing GR2; {} templates, {} visuals, {} materials, {} textures",
            level.objects.len(),
            scenery.len(),
            templates.templates.len(),
            banks.visuals.len(),
            banks.materials.len(),
            banks.textures.len()
        );
        let mut why: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for o in &scenery {
            let visual = || {
                o.visual_template
                    .as_deref()
                    .or_else(|| templates.visual_for(o.template.as_deref()?))
            };
            let reason = match o.template.as_deref().map(|t| templates.get(t)) {
                None => "no TemplateName",
                Some(None) => "template not found",
                Some(Some(_)) => match visual() {
                    None => "template has no visual",
                    Some(v) => match banks.visuals.get(v) {
                        None if banks.kind(v) == Some("EffectBank") => "effect (not a mesh)",
                        None => "visual not in banks",
                        Some(r) => match &r.source_file {
                            None => "visual has no SourceFile",
                            Some(f) if !vfs.exists(f) => "GR2 missing",
                            Some(_) => continue,
                        },
                    },
                },
            };
            why.entry(reason).or_default().push(&o.name);
        }
        for (reason, names) in &why {
            eprintln!(
                "{reason}: {} e.g. {:?}",
                names.len(),
                &names[..names.len().min(4)]
            );
        }
        let effects = why.get("effect (not a mesh)").map_or(0, Vec::len);
        assert_eq!(
            resolved + effects,
            scenery.len(),
            "every scenery object should resolve to a GR2 or an effect"
        );
    }
}
