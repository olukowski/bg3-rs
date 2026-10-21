//! Loading [`GameData`] from the install (through the VFS).
//!
//! | Data | Where |
//! |------|-------|
//! | flags | `Public/<Module>/Flags/<uuid>.lsf`, region `Flags`: `UUID`, `Name`, `Usage`, `Description` |
//! | tags | `Public/<Module>/Tags/<uuid>.lsf`, region `Tags`: `UUID`, `Name`, `Categories/Category/Name` |
//! | statuses | stats `StatusData` entries (`StatusType`, `StatusGroups`) |
//! | templates | `Public/<Module>/RootTemplates/_merged.lsf`: `MapKey`, `Name`, `Type`, `ParentTemplateId`, `Tags/Tag/Object` |
//! | globals | `Mods/<Module>/Globals/<Level>/{Characters,Items,Triggers}/_merged.lsf` |
//! | level objects | `Mods/<Module>/Levels/<Level>/{Characters,Items,Triggers}/_merged.lsf` |
//! | characters' rule stats | stats `Character` entries ([`crate::character`]); `LevelOverride` of templates and objects |
//! | level maps | `Public/<Module>/Levelmaps/LevelMapValues.lsx` (`bg3_rules::levelmap`) |
//! | difficulty classes | `Public/<Module>/DifficultyClasses/DifficultyClasses.lsx`: `UUID`, `Difficulties` |
//!
//! Modules are read in `bg3_vfs::modules::BASE_LOAD_ORDER`; a later module replaces a
//! definition with the same UUID.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;
use rayon::prelude::*;

use crate::entity::EntityKind;
use crate::flags::{FlagDef, FlagDefs, FlagUsage};
use crate::guid::Guid;
use crate::sim::{GameData, ModuleLevels, PlacedObject, RaceInfo, TemplateInfo, WeightCategory};
use crate::status::StatusDefs;
use crate::tags::{TagDef, TagDefs};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Vfs(#[from] bg3_vfs::Error),
    #[error("{path}: {source}")]
    Lsf {
        path: String,
        #[source]
        source: bg3_lsf::Error,
    },
}

/// Object directories that hold story-relevant objects.
const OBJECT_DIRS: &[&str] = &["Characters", "Items", "Triggers"];

pub(crate) fn parse(vfs: &Vfs, path: &str) -> Result<bg3_lsf::Resource, LoadError> {
    let data = vfs.read(path)?;
    bg3_lsf::parse(&data).map_err(|source| LoadError::Lsf {
        path: path.to_owned(),
        source,
    })
}

pub(crate) fn s(node: &Node, name: &str) -> Option<String> {
    node.attr(name)
        .map(ToString::to_string)
        .filter(|s| !s.is_empty())
}

pub(crate) fn guid_attr(node: &Node, name: &str) -> Option<Guid> {
    s(node, name).and_then(|v| Guid::parse(&v))
}

/// `LevelOverride`, if set to a level (> 0).
fn level_override(node: &Node) -> Option<i32> {
    node.attr("LevelOverride")
        .and_then(|v| v.as_i64())
        .filter(|l| *l > 0)
        .map(|l| l as i32)
}

/// `Tags/Tag/Object` GUIDs of a template or level object.
fn tag_list(node: &Node) -> Vec<Guid> {
    node.child("Tags")
        .into_iter()
        .flat_map(|t| t.children_named("Tag"))
        .filter_map(|t| guid_attr(t, "Object"))
        .collect()
}

/// `StatusList/Status@Object` status names of a template or level object.
fn status_list(node: &Node) -> Vec<String> {
    node.child("StatusList")
        .into_iter()
        .flat_map(|t| t.children_named("Status"))
        .filter_map(|t| s(t, "Object"))
        .collect()
}

/// Paths of one resource kind across modules, in load order.
fn module_files(vfs: &Vfs, kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in BASE_LOAD_ORDER {
        let prefix = format!("Public/{m}/{kind}/");
        let mut files: Vec<String> = vfs
            .files_under(&prefix)
            .map(|f| f.entry.name.clone())
            .filter(|n| n.to_ascii_lowercase().ends_with(".lsf"))
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

/// Every flag definition.
pub fn load_flags(vfs: &Vfs) -> Result<FlagDefs, LoadError> {
    let files = module_files(vfs, "Flags");
    let defs: Vec<Option<FlagDef>> = files
        .par_iter()
        .map(|p| -> Result<Option<FlagDef>, LoadError> {
            let res = parse(vfs, p)?;
            let Some(n) = res.region("Flags") else {
                return Ok(None);
            };
            Ok(guid_attr(n, "UUID").map(|id| FlagDef {
                id,
                name: s(n, "Name").unwrap_or_default(),
                usage: FlagUsage::from_raw(
                    n.attr("Usage").and_then(|v| v.as_i64()).unwrap_or(0) as u8
                ),
                description: s(n, "Description").unwrap_or_default(),
            }))
        })
        .collect::<Result<_, _>>()?;
    let mut out = FlagDefs::new();
    for d in defs.into_iter().flatten() {
        out.insert(d);
    }
    Ok(out)
}

/// Every tag definition.
pub fn load_tags(vfs: &Vfs) -> Result<TagDefs, LoadError> {
    let files = module_files(vfs, "Tags");
    let defs: Vec<Option<TagDef>> = files
        .par_iter()
        .map(|p| -> Result<Option<TagDef>, LoadError> {
            let res = parse(vfs, p)?;
            let Some(n) = res.region("Tags") else {
                return Ok(None);
            };
            Ok(guid_attr(n, "UUID").map(|id| TagDef {
                id,
                name: s(n, "Name").unwrap_or_default(),
                categories: n
                    .child("Categories")
                    .into_iter()
                    .flat_map(|c| c.children_named("Category"))
                    .filter_map(|c| s(c, "Name"))
                    .collect(),
            }))
        })
        .collect::<Result<_, _>>()?;
    let mut out = TagDefs::new();
    for d in defs.into_iter().flatten() {
        out.insert(d);
    }
    Ok(out)
}

pub(crate) fn game_objects(res: &bg3_lsf::Resource) -> impl Iterator<Item = &Node> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children_named("GameObjects"))
}

/// Root templates (name, type, parent, tags).
pub fn load_templates(vfs: &Vfs) -> Result<HashMap<Guid, TemplateInfo>, LoadError> {
    let mut out = HashMap::new();
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/RootTemplates/_merged.lsf");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        for n in game_objects(&res) {
            let Some(id) = guid_attr(n, "MapKey") else {
                continue;
            };
            out.insert(
                id,
                TemplateInfo {
                    id,
                    name: s(n, "Name").unwrap_or_default(),
                    kind: EntityKind::from_level_type(&s(n, "Type").unwrap_or_default()),
                    parent: guid_attr(n, "ParentTemplateId"),
                    tags: tag_list(n),
                    race: guid_attr(n, "Race"),
                    stats: s(n, "Stats"),
                    statuses: status_list(n),
                    faction: guid_attr(n, "Faction"),
                    level_override: level_override(n),
                },
            );
        }
    }
    Ok(out)
}

/// Weight categories (`Public/<Module>/WeightCategories/WeightCategories.lsx`:
/// `WeightCategory` nodes with `MaxWeight` (g) and `Tags@Object`), ascending. A later module
/// replaces the list.
pub fn load_weight_categories(vfs: &Vfs) -> Result<Vec<WeightCategory>, LoadError> {
    let mut out: Vec<WeightCategory> = Vec::new();
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/WeightCategories/WeightCategories.lsx");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        let Some(root) = res.region("WeightCategories") else {
            continue;
        };
        out = root
            .children_named("WeightCategory")
            .chain(
                root.children_named("root")
                    .flat_map(|r| r.children_named("WeightCategory")),
            )
            .map(|n| WeightCategory {
                max_weight: n.attr("MaxWeight").and_then(|v| v.as_i64()).unwrap_or(0),
                tags: n
                    .children_named("Tags")
                    .filter_map(|t| guid_attr(t, "Object"))
                    .collect(),
            })
            .collect();
    }
    out.sort_by_key(|c| c.max_weight);
    Ok(out)
}

/// `Weight` (kg) of every stats entry that has one.
pub fn stats_weights(stats: &bg3_stats::Stats) -> HashMap<String, f32> {
    stats
        .entries()
        .filter_map(|o| Some((o.name.clone(), o.get("Weight")?.trim().parse().ok()?)))
        .collect()
}

fn find_attr(n: &Node, name: &str) -> Option<String> {
    s(n, name).or_else(|| n.children.iter().find_map(|c| find_attr(c, name)))
}

/// Level names from `Mods/<Module>/meta.lsx` (`ModuleInfo`: `StartupLevelName`,
/// `CharacterCreationLevelName`, `MenuLevelName`, `LobbyLevelName`); a later module's
/// non-empty value wins.
pub fn load_module_levels(vfs: &Vfs) -> Result<ModuleLevels, LoadError> {
    let mut out = ModuleLevels::default();
    for m in BASE_LOAD_ORDER {
        let path = format!("Mods/{m}/meta.lsx");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        for r in &res.regions {
            for (field, attr) in [
                (&mut out.startup, "StartupLevelName"),
                (&mut out.character_creation, "CharacterCreationLevelName"),
                (&mut out.menu, "MenuLevelName"),
                (&mut out.lobby, "LobbyLevelName"),
            ] {
                if let Some(v) = find_attr(&r.node, attr) {
                    *field = v;
                }
            }
        }
    }
    Ok(out)
}

/// Faction names (`Public/<Module>/Factions/Factions.lsx`, region `FactionContainer`,
/// `Faction` nodes with `Faction` (name) and `UUID`).
pub fn load_factions(vfs: &Vfs) -> Result<HashMap<Guid, String>, LoadError> {
    let mut out = HashMap::new();
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/Factions/Factions.lsx");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        let Some(root) = res.region("FactionContainer") else {
            continue;
        };
        let list = root.children_named("Faction").chain(
            root.children_named("root")
                .flat_map(|r| r.children_named("Faction")),
        );
        for n in list {
            if let (Some(id), Some(name)) = (guid_attr(n, "UUID"), s(n, "Faction")) {
                out.insert(id, name);
            }
        }
    }
    Ok(out)
}

/// Races (`Public/<Module>/Races/Races.lsx`, `Race` nodes with `Name`, `UUID`, `ParentGuid`
/// and `Tags@Object` children). A later module's entry for the same UUID adds its tags.
pub fn load_races(vfs: &Vfs) -> Result<HashMap<Guid, RaceInfo>, LoadError> {
    let mut out: HashMap<Guid, RaceInfo> = HashMap::new();
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/Races/Races.lsx");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        let Some(root) = res.region("Races") else {
            continue;
        };
        let races = root.children_named("Race").chain(
            root.children_named("root")
                .flat_map(|r| r.children_named("Race")),
        );
        for n in races {
            let Some(id) = guid_attr(n, "UUID") else {
                continue;
            };
            let tags: Vec<Guid> = n
                .children_named("Tags")
                .filter_map(|t| guid_attr(t, "Object"))
                .collect();
            let e = out.entry(id).or_insert_with(|| RaceInfo {
                id,
                name: String::new(),
                parent: None,
                tags: Vec::new(),
            });
            if let Some(name) = s(n, "Name") {
                e.name = name;
            }
            if let Some(p) = guid_attr(n, "ParentGuid") {
                e.parent = Some(p);
            }
            for t in tags {
                if !e.tags.contains(&t) {
                    e.tags.push(t);
                }
            }
        }
    }
    Ok(out)
}

fn placed(n: &Node, level_dir: &str, global: bool) -> Option<PlacedObject> {
    let id = guid_attr(n, "MapKey")?;
    let position = n
        .child("Transform")
        .and_then(|t| t.attr("Position"))
        .and_then(|v| v.components())
        .filter(|c| c.len() == 3)
        .map_or([0.0; 3], |c| [c[0] as f32, c[1] as f32, c[2] as f32]);
    Some(PlacedObject {
        id,
        name: s(n, "Name").unwrap_or_default(),
        kind: EntityKind::from_level_type(&s(n, "Type").unwrap_or_default()),
        template: guid_attr(n, "TemplateName"),
        level: s(n, "LevelName").or_else(|| Some(level_dir.to_owned())),
        global,
        position,
        tags: tag_list(n),
        stats: s(n, "Stats"),
        statuses: status_list(n),
        race: guid_attr(n, "Race"),
        faction: guid_attr(n, "Faction"),
        level_override: level_override(n),
    })
}

/// Difficulty classes (`Public/<Module>/DifficultyClasses/DifficultyClasses.lsx`,
/// `DifficultyClass` nodes with `UUID` and `Difficulties`). `Difficulties` is a list; every
/// entry in the reference build has a single value, which is the DC. A later module replaces
/// an entry.
// bg3: eoc::DifficultyClassManager, eoc::DifficultyClass::Visit (4.1.1.7631656)
pub fn load_difficulty_classes(vfs: &Vfs) -> Result<HashMap<Guid, i32>, LoadError> {
    let mut out = HashMap::new();
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/DifficultyClasses/DifficultyClasses.lsx");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        let Some(root) = res.region("DifficultyClasses") else {
            continue;
        };
        let list = root.children_named("DifficultyClass").chain(
            root.children_named("root")
                .flat_map(|r| r.children_named("DifficultyClass")),
        );
        for n in list {
            let dc = s(n, "Difficulties")
                .and_then(|d| d.split(',').next().and_then(|v| v.trim().parse().ok()));
            if let (Some(id), Some(dc)) = (guid_attr(n, "UUID"), dc) {
                out.insert(id, dc);
            }
        }
    }
    Ok(out)
}

/// Read the object directories under `base` (`Mods/<M>/Globals/<L>` or
/// `Mods/<M>/Levels/<L>`), merging into `out` by id (a later module replaces).
fn read_objects(
    vfs: &Vfs,
    base: &str,
    level_dir: &str,
    global: bool,
    out: &mut Vec<PlacedObject>,
    index: &mut HashMap<Guid, usize>,
) -> Result<(), LoadError> {
    for dir in OBJECT_DIRS {
        let path = format!("{base}/{dir}/_merged.lsf");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        for o in game_objects(&res).filter_map(|n| placed(n, level_dir, global)) {
            match index.get(&o.id) {
                Some(&i) => out[i] = o,
                None => {
                    index.insert(o.id, out.len());
                    out.push(o);
                }
            }
        }
    }
    Ok(())
}

/// Objects of every level's `Globals` directory.
pub fn load_globals(vfs: &Vfs) -> Result<Vec<PlacedObject>, LoadError> {
    let mut out = Vec::new();
    let mut index = HashMap::new();
    for m in BASE_LOAD_ORDER {
        let prefix = format!("Mods/{m}/Globals/");
        let mut levels: Vec<String> = vfs
            .files_under(&prefix)
            .filter_map(|f| {
                let rest = f.entry.name.get(prefix.len()..)?;
                Some(rest.split('/').next()?.to_owned())
            })
            .collect();
        levels.sort();
        levels.dedup();
        for l in levels {
            read_objects(
                vfs,
                &format!("Mods/{m}/Globals/{l}"),
                &l,
                true,
                &mut out,
                &mut index,
            )?;
        }
    }
    Ok(out)
}

/// A level's own (non-global) objects.
pub fn load_level_objects(vfs: &Vfs, level: &str) -> Result<Vec<PlacedObject>, LoadError> {
    let mut out = Vec::new();
    let mut index = HashMap::new();
    for m in BASE_LOAD_ORDER {
        read_objects(
            vfs,
            &format!("Mods/{m}/Levels/{level}"),
            level,
            false,
            &mut out,
            &mut index,
        )?;
    }
    Ok(out)
}

impl GameData {
    /// Load everything the sim needs, with the objects of the given levels (none given: the
    /// module's character-creation and startup levels, where a new game begins).
    pub fn load(vfs: &Vfs, stats: &bg3_stats::Stats, levels: &[&str]) -> Result<Self, LoadError> {
        let mut data = Self {
            flags: load_flags(vfs)?,
            tags: load_tags(vfs)?,
            statuses: StatusDefs::from_stats(stats),
            templates: load_templates(vfs)?,
            races: load_races(vfs)?,
            factions: load_factions(vfs)?,
            module_levels: load_module_levels(vfs)?,
            weight_categories: load_weight_categories(vfs)?,
            stats_weight: stats_weights(stats),
            stats_type: stats
                .entries()
                .filter_map(|o| Some((o.name.clone(), o.type_name.clone()?)))
                .collect(),
            globals: load_globals(vfs)?,
            levels: HashMap::new(),
            characters: crate::character::character_defs(stats),
            level_maps: bg3_rules::levelmap::LevelMaps::load(vfs, BASE_LOAD_ORDER),
            difficulty_classes: load_difficulty_classes(vfs)?,
            combat: crate::combat::CombatData::new(),
            story: crate::story_data::StoryData::default(),
        };
        let mut names: Vec<String> = levels.iter().map(|l| (*l).to_owned()).collect();
        if names.is_empty() {
            let m = &data.module_levels;
            names = [&m.character_creation, &m.startup]
                .into_iter()
                .filter(|l| !l.is_empty())
                .cloned()
                .collect();
        }
        data.story = crate::story_data::StoryData::load(vfs, &names)?;
        data.combat = crate::combat::CombatData::load(vfs, stats, &names)?;
        for l in names {
            let objs = load_level_objects(vfs, &l)?;
            data.levels.insert(l, objs);
        }
        Ok(data)
    }
}
