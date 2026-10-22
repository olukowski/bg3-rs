//! What character creation reads from the install.
//!
//! | Data | Where |
//! |---|---|
//! | races | `Public/<Module>/Races/Races.lsx`: `UUID`, `Name`, `DisplayName`, `ParentGuid`, `ProgressionTableUUID`, `RaceEquipment`, `Tags`, `ExcludedGods`, `Visuals/Object` |
//! | classes, subclasses | `Public/<Module>/ClassDescriptions/ClassDescriptions.lsx`: `UUID`, `Name`, `DisplayName`, `ParentGuid`, `BaseHp`, `HpPerLevel`, `ProgressionTableUUID`, `PrimaryAbility`, `SpellCastingAbility`, `ClassEquipment`, `HasGod`, `MustPrepareSpells`, `Tags` |
//! | backgrounds | `Public/<Module>/Backgrounds/Backgrounds.lsx`: `UUID`, `DisplayName`, `Passives`, `Hidden`, `Tags` |
//! | deities | `Public/<Module>/Gods/Gods.lsx`: `UUID`, `Name`, `DisplayName`, `Tags` |
//! | progressions | `Public/<Module>/Progressions/Progressions.lsx`: `TableUUID`, `Level`, `ProgressionType`, `IsMulticlass`, `Boosts`, `PassivesAdded`, `PassivesRemoved`, `Selectors`, `SubClasses/SubClass@Object` |
//! | lists | `Public/<Module>/Lists/{Ability,Skill,Spell,Passive,Equipment}Lists.lsx` |
//! | body presets | `Public/<Module>/CharacterCreationPresets/CharacterCreationPresets.lsx`: `RaceUUID`, `SubRaceUUID`, `BodyType`, `BodyShape`, `RootTemplate` |
//! | class ability presets | `…/CharacterCreationPresets/AbilityDistributionPresets.lsx`: `ClassUUID`, six scores |
//! | selector defaults | `Public/<Module>/DefaultValues/{Abilities,Skills,Spells,Passives,Equipments}.lsx`: `TableUUID`, `Level`, `SelectorId`, `RaceUUID`, `OriginUUID`, `Add` |
//! | the custom origin | `Public/<Module>/Origins/Origins.lsx`, `Name` `Generic`: `Passives`, `AppearanceTags`, `ReallyTags` |
//! | equipment, items, passives | stats `Equipment.txt`, `Weapon`/`Armor` entries, `PassiveData` `Boosts` |
//!
//! Modules are read in `bg3_vfs::modules::BASE_LOAD_ORDER`; a later module's entry with the
//! same UUID replaces the earlier one as a whole (*assumption*, as for the other UUID-keyed
//! resources; e.g. `SharedDev` re-defines the level-1 class progressions with an added
//! `SelectAbilityBonus`).

use std::collections::HashMap;

use bg3_lsf::{Node, Value};
use bg3_rules::context::Ability;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use super::rules::{Selector, parse_selectors};
use crate::guid::Guid;
use crate::load::{LoadError, guid_attr, parse, s};

/// A race or subrace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcRace {
    /// Shared CC entries selected by Races/Visuals/Object.
    pub visuals: Vec<Guid>,
    pub id: Guid,
    pub name: String,
    /// Loca handle of `DisplayName`.
    pub display: Option<String>,
    pub parent: Option<Guid>,
    pub progression_table: Option<Guid>,
    pub tags: Vec<Guid>,
    pub excluded_gods: Vec<Guid>,
    /// `RaceEquipment`: an equipment set (underwear).
    pub equipment: Option<String>,
}

/// A class or subclass (`parent` set).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcClass {
    pub id: Guid,
    pub name: String,
    pub display: Option<String>,
    pub parent: Option<Guid>,
    pub base_hp: i32,
    pub hp_per_level: i32,
    pub progression_table: Option<Guid>,
    pub primary_ability: Option<Ability>,
    pub spellcasting_ability: Option<Ability>,
    pub tags: Vec<Guid>,
    /// `ClassEquipment`: the starting equipment set.
    pub equipment: Option<String>,
    pub has_god: bool,
    pub must_prepare_spells: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcBackground {
    pub id: Guid,
    pub display: Option<String>,
    pub passives: Vec<String>,
    pub tags: Vec<Guid>,
    /// Not offered in character creation (origin characters' backgrounds).
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcGod {
    pub id: Guid,
    pub name: String,
    pub display: Option<String>,
    pub tags: Vec<Guid>,
}

/// A body preset: the root template the created character is made from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcPreset {
    pub id: Guid,
    pub race: Guid,
    pub subrace: Option<Guid>,
    /// 0 masculine, 1 feminine.
    pub body_type: u8,
    /// 0 regular, 1 strong.
    pub body_shape: u8,
    pub root_template: Guid,
}

/// The origin a custom character gets (`Generic`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcOrigin {
    pub id: Guid,
    pub name: String,
    pub passives: Vec<String>,
    pub appearance_tags: Vec<Guid>,
    pub really_tags: Vec<Guid>,
}

/// One row of a progression table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progression {
    pub id: Guid,
    pub table: Guid,
    pub name: String,
    pub level: i32,
    /// `ProgressionType`: 0 class, 1 subclass, 2 race.
    pub kind: u8,
    /// The row a multiclassed character gets instead (not used at level 1 of a new character).
    pub multiclass: bool,
    pub boosts: String,
    pub passives_added: Vec<String>,
    pub passives_removed: Vec<String>,
    pub selectors: Vec<Selector>,
    /// Subclasses to choose from at this level.
    pub subclasses: Vec<Guid>,
}

/// Which `DefaultValues` file a default comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefaultKind {
    Abilities,
    Skills,
    Spells,
    Passives,
    Equipments,
}

/// A selector's default picks, in preference order.
#[derive(Clone, Debug, PartialEq)]
pub struct DefaultValue {
    pub kind: DefaultKind,
    pub table: Guid,
    pub level: i32,
    pub selector_id: Option<String>,
    pub race: Option<Guid>,
    pub origin: Option<Guid>,
    pub add: Vec<String>,
}

/// The fields of an item stats entry character creation needs (armour class, proficiency,
/// weapon actions).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcItem {
    pub name: String,
    /// `Weapon`, `Armor` or `Object`.
    pub kind: String,
    pub slot: String,
    pub armor_class: i32,
    pub ability: Option<Ability>,
    pub ability_cap: i32,
    pub shield: bool,
    /// `ArmorType` (`None`, `Cloth`, `Leather`, …).
    pub armor_type: String,
    pub proficiency: Vec<String>,
    /// `UnlockSpell(X)` boosts of `BoostsOnEquipMainHand`.
    pub unlocks: Vec<String>,
}

/// Everything character creation reads.
#[derive(Clone, Debug, Default)]
pub struct CcData {
    /// Every race and subrace, in load order.
    pub races: Vec<CcRace>,
    /// Every class and subclass, in load order.
    pub classes: Vec<CcClass>,
    pub backgrounds: Vec<CcBackground>,
    pub gods: Vec<CcGod>,
    pub presets: Vec<CcPreset>,
    pub appearance: super::appearance::AppearanceData,
    pub origins: Vec<CcOrigin>,
    /// By `TableUUID`, in load order.
    pub progressions: HashMap<Guid, Vec<Progression>>,
    pub ability_lists: HashMap<Guid, Vec<String>>,
    pub skill_lists: HashMap<Guid, Vec<String>>,
    pub spell_lists: HashMap<Guid, Vec<String>>,
    pub passive_lists: HashMap<Guid, Vec<String>>,
    pub equipment_lists: HashMap<Guid, Vec<String>>,
    /// `AbilityDistributionPresets` by class: Strength … Charisma.
    pub ability_presets: HashMap<Guid, [i32; 6]>,
    pub defaults: Vec<DefaultValue>,
    /// `Boosts` of `PassiveData` entries.
    pub passive_boosts: HashMap<String, String>,
    /// `BoostConditions` of `PassiveData` entries (the boosts apply while it holds).
    pub passive_conditions: HashMap<String, String>,
    /// Equipment sets: the first entry of each `add equipmentgroup`.
    pub equipment_sets: HashMap<String, Vec<String>>,
    /// Equipment sets' `initialweaponset`.
    pub initial_weapon_sets: HashMap<String, bg3_world::unsheath::WeaponSet>,
    pub items: HashMap<String, CcItem>,
    /// Spells that exist (`SpellData` names), to drop list entries the stats don't define.
    pub spells: std::collections::HashSet<String>,
    /// Loca handles of the `DisplayName` of spells, passives and items (stats entries).
    pub display_names: HashMap<String, String>,
}

fn list(v: Option<String>) -> Vec<String> {
    v.unwrap_or_default()
        .split([';', ','])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(super) fn handle(n: &Node, key: &str) -> Option<String> {
    match n.attr(key) {
        Some(Value::TranslatedString(t)) if !t.handle.is_empty() => Some(t.handle.clone()),
        _ => None,
    }
}

pub(super) fn int(n: &Node, key: &str) -> Option<i64> {
    n.attr(key).and_then(|v| v.as_i64())
}

fn flag(n: &Node, key: &str) -> bool {
    matches!(n.attr(key), Some(Value::Bool(true)))
}

fn objects(n: &Node, child: &str) -> Vec<Guid> {
    n.children_named(child)
        .filter_map(|t| guid_attr(t, "Object"))
        .collect()
}

/// `EAbility` as stored in `ClassDescriptions` (`PrimaryAbility`, `SpellCastingAbility`):
/// 0 none, 1 Strength … 6 Charisma (*inferred*: Cleric 5 = Wisdom, Wizard 4 = Intelligence,
/// Fighter's spellcasting 0).
pub fn ability_from_index(v: i64) -> Option<Ability> {
    usize::try_from(v - 1)
        .ok()
        .and_then(|i| Ability::ALL.get(i))
        .copied()
}

/// Insert or replace by id, keeping the first position.
pub(super) fn upsert<T>(v: &mut Vec<T>, item: T, id: impl Fn(&T) -> Guid) {
    let key = id(&item);
    match v.iter_mut().find(|x| id(x) == key) {
        Some(x) => *x = item,
        None => v.push(item),
    }
}

/// The nodes named `name` of a resource region, directly or under `root`.
pub(super) fn nodes<'a>(res: &'a bg3_lsf::Resource, region: &str, name: &'a str) -> Vec<&'a Node> {
    let Some(root) = res.region(region) else {
        return Vec::new();
    };
    root.children_named(name)
        .chain(
            root.children_named("root")
                .flat_map(move |r| r.children_named(name)),
        )
        .collect()
}

/// Each module's copy of a resource, in load order.
pub(super) fn each_module(
    vfs: &Vfs,
    rel: &str,
    mut f: impl FnMut(&bg3_lsf::Resource),
) -> Result<(), LoadError> {
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/{rel}");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            f(&res);
        }
    }
    Ok(())
}

impl CcData {
    /// Load from the install (resources through the VFS, plus the merged stats).
    pub fn load(vfs: &Vfs, stats: &bg3_stats::Stats) -> Result<Self, LoadError> {
        let mut d = Self::default();
        each_module(vfs, "Races/Races.lsx", |res| {
            for n in nodes(res, "Races", "Race") {
                let Some(id) = guid_attr(n, "UUID") else {
                    continue;
                };
                let r = CcRace {
                    visuals: objects(n, "Visuals"),
                    id,
                    name: s(n, "Name").unwrap_or_default(),
                    display: handle(n, "DisplayName"),
                    parent: guid_attr(n, "ParentGuid"),
                    progression_table: guid_attr(n, "ProgressionTableUUID"),
                    tags: objects(n, "Tags"),
                    excluded_gods: objects(n, "ExcludedGods"),
                    equipment: s(n, "RaceEquipment"),
                };
                upsert(&mut d.races, r, |r| r.id);
            }
        })?;
        each_module(vfs, "ClassDescriptions/ClassDescriptions.lsx", |res| {
            for n in nodes(res, "ClassDescriptions", "ClassDescription") {
                let Some(id) = guid_attr(n, "UUID") else {
                    continue;
                };
                let c = CcClass {
                    id,
                    name: s(n, "Name").unwrap_or_default(),
                    display: handle(n, "DisplayName"),
                    parent: guid_attr(n, "ParentGuid"),
                    base_hp: int(n, "BaseHp").unwrap_or(0) as i32,
                    hp_per_level: int(n, "HpPerLevel").unwrap_or(0) as i32,
                    progression_table: guid_attr(n, "ProgressionTableUUID"),
                    primary_ability: int(n, "PrimaryAbility").and_then(ability_from_index),
                    spellcasting_ability: int(n, "SpellCastingAbility")
                        .and_then(ability_from_index),
                    tags: objects(n, "Tags"),
                    equipment: s(n, "ClassEquipment"),
                    has_god: flag(n, "HasGod"),
                    must_prepare_spells: flag(n, "MustPrepareSpells"),
                };
                upsert(&mut d.classes, c, |c| c.id);
            }
        })?;
        each_module(vfs, "Backgrounds/Backgrounds.lsx", |res| {
            for n in nodes(res, "Backgrounds", "Background") {
                let Some(id) = guid_attr(n, "UUID") else {
                    continue;
                };
                let b = CcBackground {
                    id,
                    display: handle(n, "DisplayName"),
                    passives: list(s(n, "Passives")),
                    tags: objects(n, "Tags"),
                    hidden: flag(n, "Hidden"),
                };
                upsert(&mut d.backgrounds, b, |b| b.id);
            }
        })?;
        each_module(vfs, "Gods/Gods.lsx", |res| {
            for n in nodes(res, "Gods", "God") {
                let Some(id) = guid_attr(n, "UUID") else {
                    continue;
                };
                let g = CcGod {
                    id,
                    name: s(n, "Name").unwrap_or_default(),
                    display: handle(n, "DisplayName"),
                    tags: objects(n, "Tags"),
                };
                upsert(&mut d.gods, g, |g| g.id);
            }
        })?;
        each_module(
            vfs,
            "CharacterCreationPresets/CharacterCreationPresets.lsx",
            |res| {
                for n in nodes(res, "CharacterCreationPresets", "CharacterCreationPreset") {
                    let (Some(id), Some(race), Some(t)) = (
                        guid_attr(n, "UUID"),
                        guid_attr(n, "RaceUUID"),
                        guid_attr(n, "RootTemplate"),
                    ) else {
                        continue;
                    };
                    let p = CcPreset {
                        id,
                        race,
                        subrace: guid_attr(n, "SubRaceUUID").filter(|g| !g.is_null()),
                        body_type: int(n, "BodyType").unwrap_or(0) as u8,
                        body_shape: int(n, "BodyShape").unwrap_or(0) as u8,
                        root_template: t,
                    };
                    upsert(&mut d.presets, p, |p| p.id);
                }
            },
        )?;
        each_module(
            vfs,
            "CharacterCreationPresets/AbilityDistributionPresets.lsx",
            |res| {
                for n in nodes(
                    res,
                    "AbilityDistributionPresets",
                    "AbilityDistributionPreset",
                ) {
                    let Some(class) = guid_attr(n, "ClassUUID") else {
                        continue;
                    };
                    let mut a = [10; 6];
                    for (i, ab) in Ability::ALL.iter().enumerate() {
                        a[i] = int(n, ab.name()).unwrap_or(10) as i32;
                    }
                    d.ability_presets.insert(class, a);
                }
            },
        )?;
        each_module(vfs, "Origins/Origins.lsx", |res| {
            for n in nodes(res, "Origins", "Origin") {
                let Some(id) = guid_attr(n, "UUID") else {
                    continue;
                };
                let o = CcOrigin {
                    id,
                    name: s(n, "Name").unwrap_or_default(),
                    passives: list(s(n, "Passives")),
                    appearance_tags: objects(n, "AppearanceTags"),
                    really_tags: objects(n, "ReallyTags"),
                };
                upsert(&mut d.origins, o, |o| o.id);
            }
        })?;
        let mut rows: Vec<Progression> = Vec::new();
        each_module(vfs, "Progressions/Progressions.lsx", |res| {
            for n in nodes(res, "Progressions", "Progression") {
                let (Some(id), Some(table)) = (guid_attr(n, "UUID"), guid_attr(n, "TableUUID"))
                else {
                    continue;
                };
                let p = Progression {
                    id,
                    table,
                    name: s(n, "Name").unwrap_or_default(),
                    level: int(n, "Level").unwrap_or(0) as i32,
                    kind: int(n, "ProgressionType").unwrap_or(0) as u8,
                    multiclass: flag(n, "IsMulticlass"),
                    boosts: s(n, "Boosts").unwrap_or_default(),
                    passives_added: list(s(n, "PassivesAdded")),
                    passives_removed: list(s(n, "PassivesRemoved")),
                    selectors: parse_selectors(&s(n, "Selectors").unwrap_or_default()),
                    subclasses: n
                        .children_named("SubClasses")
                        .flat_map(|c| c.children_named("SubClass"))
                        .filter_map(|c| guid_attr(c, "Object"))
                        .collect(),
                };
                upsert(&mut rows, p, |p| p.id);
            }
        })?;
        for p in rows {
            d.progressions.entry(p.table).or_default().push(p);
        }
        for (file, key, out) in [
            ("AbilityLists", "Abilities", &mut d.ability_lists),
            ("SkillLists", "Skills", &mut d.skill_lists),
            ("SpellLists", "Spells", &mut d.spell_lists),
            ("PassiveLists", "Passives", &mut d.passive_lists),
            ("EquipmentLists", "Items", &mut d.equipment_lists),
        ] {
            let node = &file[..file.len() - 1];
            each_module(vfs, &format!("Lists/{file}.lsx"), |res| {
                for n in nodes(res, file, node) {
                    if let Some(id) = guid_attr(n, "UUID") {
                        out.insert(id, list(s(n, key)));
                    }
                }
            })?;
        }
        for (file, kind) in [
            ("Abilities", DefaultKind::Abilities),
            ("Skills", DefaultKind::Skills),
            ("Spells", DefaultKind::Spells),
            ("Passives", DefaultKind::Passives),
            ("Equipments", DefaultKind::Equipments),
        ] {
            let mut rows: Vec<(Guid, DefaultValue)> = Vec::new();
            each_module(vfs, &format!("DefaultValues/{file}.lsx"), |res| {
                for n in nodes(res, "DefaultValues", "DefaultValue") {
                    let (Some(id), Some(table)) = (guid_attr(n, "UUID"), guid_attr(n, "TableUUID"))
                    else {
                        continue;
                    };
                    let v = DefaultValue {
                        kind,
                        table,
                        level: int(n, "Level").unwrap_or(0) as i32,
                        selector_id: s(n, "SelectorId"),
                        race: guid_attr(n, "RaceUUID"),
                        origin: guid_attr(n, "OriginUUID"),
                        add: list(s(n, "Add")),
                    };
                    upsert(&mut rows, (id, v), |r| r.0);
                }
            })?;
            d.defaults.extend(rows.into_iter().map(|(_, v)| v));
        }
        d.appearance = super::appearance::AppearanceData::load(vfs)?;
        d.add_stats(stats);
        Ok(d)
    }

    /// The stats part: equipment sets, item entries, passive boosts, spell names.
    pub fn add_stats(&mut self, stats: &bg3_stats::Stats) {
        for e in stats.equipment.iter() {
            let firsts = e.groups.iter().filter_map(|g| g.first().cloned()).collect();
            self.equipment_sets.insert(e.name.clone(), firsts);
            self.initial_weapon_sets.insert(
                e.name.clone(),
                bg3_world::unsheath::WeaponSet::from_initial(e.initial_weapon_set.as_deref()),
            );
        }
        for o in stats.entries() {
            let ty = o.type_name.as_deref().unwrap_or("");
            if matches!(
                ty,
                "PassiveData" | "SpellData" | "Weapon" | "Armor" | "Object"
            ) && let Some(h) = o.get("DisplayName")
            {
                let h = h.split(';').next().unwrap_or(h).trim();
                if !h.is_empty() {
                    self.display_names.insert(o.name.clone(), h.to_owned());
                }
            }
            match ty {
                "PassiveData" => {
                    if let Some(b) = o.get("Boosts").filter(|b| !b.trim().is_empty()) {
                        self.passive_boosts.insert(o.name.clone(), b.to_owned());
                    }
                    if let Some(c) = o.get("BoostConditions").filter(|c| !c.trim().is_empty()) {
                        self.passive_conditions
                            .insert(o.name.clone(), c.trim().to_owned());
                    }
                }
                "SpellData" => {
                    self.spells.insert(o.name.clone());
                }
                "Weapon" | "Armor" | "Object" => {
                    let num = |k: &str| o.get(k).and_then(|v| v.trim().parse().ok());
                    let unlocks = o
                        .get("BoostsOnEquipMainHand")
                        .and_then(|b| bg3_rules::fields::parse_boosts(b).ok())
                        .into_iter()
                        .flatten()
                        .filter_map(|e| match e {
                            bg3_rules::fields::BoostEntry::Boost(b)
                                if b.condition.is_none() && b.name == "UnlockSpell" =>
                            {
                                b.args.first().map(|a| a.trim().to_owned())
                            }
                            _ => None,
                        })
                        .collect();
                    self.items.insert(
                        o.name.clone(),
                        CcItem {
                            name: o.name.clone(),
                            kind: ty.to_owned(),
                            slot: o.get("Slot").unwrap_or("").trim().to_owned(),
                            armor_class: num("ArmorClass").unwrap_or(0),
                            ability: o
                                .get("Armor Class Ability")
                                .and_then(|v| Ability::parse(v.trim())),
                            ability_cap: num("Ability Modifier Cap").unwrap_or(0),
                            shield: o.get("Shield").is_some_and(|v| v.trim() == "Yes"),
                            armor_type: o.get("ArmorType").unwrap_or("").trim().to_owned(),
                            proficiency: list(o.get("Proficiency Group").map(str::to_owned)),
                            unlocks,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    pub fn race(&self, id: Guid) -> Option<&CcRace> {
        self.races.iter().find(|r| r.id == id)
    }

    pub fn class(&self, id: Guid) -> Option<&CcClass> {
        self.classes.iter().find(|c| c.id == id)
    }

    pub fn background(&self, id: Guid) -> Option<&CcBackground> {
        self.backgrounds.iter().find(|b| b.id == id)
    }

    pub fn god(&self, id: Guid) -> Option<&CcGod> {
        self.gods.iter().find(|g| g.id == id)
    }

    /// The origin of a custom character (`Generic`).
    pub fn generic_origin(&self) -> Option<&CcOrigin> {
        self.origins.iter().find(|o| o.name == "Generic")
    }

    /// Playable races (those with a body preset), in load order.
    pub fn playable_races(&self) -> Vec<&CcRace> {
        self.races
            .iter()
            .filter(|r| self.presets.iter().any(|p| p.race == r.id))
            .collect()
    }

    /// Playable subraces of a race (those with a body preset).
    pub fn subraces(&self, race: Guid) -> Vec<&CcRace> {
        self.races
            .iter()
            .filter(|r| r.parent == Some(race))
            .filter(|r| {
                self.presets
                    .iter()
                    .any(|p| p.race == race && p.subrace == Some(r.id))
            })
            .collect()
    }

    /// Classes (not subclasses) with a level-1 progression.
    pub fn playable_classes(&self) -> Vec<&CcClass> {
        self.classes
            .iter()
            .filter(|c| c.parent.is_none())
            .filter(|c| self.class_row(c.id, 1).is_some())
            .collect()
    }

    /// Backgrounds offered in character creation.
    pub fn playable_backgrounds(&self) -> Vec<&CcBackground> {
        self.backgrounds.iter().filter(|b| !b.hidden).collect()
    }

    /// The subclasses a class chooses from at level 1 (empty if it chooses later).
    pub fn level1_subclasses(&self, class: Guid) -> Vec<Guid> {
        self.class_row(class, 1)
            .map(|p| p.subclasses.clone())
            .unwrap_or_default()
    }

    /// Deities a character of this race may follow (not excluded by the race or its parent).
    pub fn allowed_gods(&self, race: Guid, subrace: Option<Guid>) -> Vec<&CcGod> {
        let excluded: Vec<Guid> = [Some(race), subrace]
            .into_iter()
            .flatten()
            .filter_map(|r| self.race(r))
            .flat_map(|r| r.excluded_gods.iter().copied())
            .collect();
        self.gods
            .iter()
            .filter(|g| !excluded.contains(&g.id))
            .collect()
    }

    /// The progression row of a table at a level (not the multiclass variant).
    pub fn table_row(&self, table: Guid, level: i32) -> Option<&Progression> {
        self.progressions
            .get(&table)?
            .iter()
            .find(|p| p.level == level && !p.multiclass)
    }

    fn class_row(&self, class: Guid, level: i32) -> Option<&Progression> {
        self.table_row(self.class(class)?.progression_table?, level)
    }

    /// The body preset for a race, subrace and body.
    pub fn preset(
        &self,
        race: Guid,
        subrace: Option<Guid>,
        body_type: u8,
        body_shape: u8,
    ) -> Option<&CcPreset> {
        self.presets.iter().find(|p| {
            p.race == race
                && p.subrace == subrace
                && p.body_type == body_type
                && p.body_shape == body_shape
        })
    }

    /// The default picks of a selector: the `DefaultValues` row for its table, level and
    /// `SelectorId`, preferring one for the race (or the race's parent).
    pub fn default_picks(
        &self,
        kind: DefaultKind,
        table: Guid,
        level: i32,
        selector_id: Option<&str>,
        races: &[Guid],
    ) -> Vec<String> {
        let rows: Vec<&DefaultValue> = self
            .defaults
            .iter()
            .filter(|d| {
                d.kind == kind
                    && d.table == table
                    && d.level == level
                    && d.origin.is_none()
                    && (d.selector_id.is_none() || d.selector_id.as_deref() == selector_id)
            })
            .collect();
        // A row for the selector's own `SelectorId` before a generic one, a race's before
        // one for any race (*assumption*: the fighter's `FightingStyle` selector takes the
        // table's generic `FightingStyle_Defense` row).
        let exact = |d: &&&DefaultValue| d.selector_id.as_deref() == selector_id;
        let race = |d: &&&DefaultValue| d.race.is_some_and(|r| races.contains(&r));
        // Rows are concatenated in that order: a table can have several selectors without
        // an id (the wizard's cantrips and spells), and the caller keeps the options its
        // selector has.
        let mut out: Vec<String> = Vec::new();
        let order: [&dyn Fn(&&&DefaultValue) -> bool; 4] = [
            &|d| exact(d) && race(d),
            &|d| exact(d) && d.race.is_none(),
            &|d| race(d),
            &|d| d.race.is_none(),
        ];
        for f in order {
            for d in rows.iter().filter(|d| f(d)) {
                for x in &d.add {
                    if !out.contains(x) {
                        out.push(x.clone());
                    }
                }
            }
        }
        out
    }
}
