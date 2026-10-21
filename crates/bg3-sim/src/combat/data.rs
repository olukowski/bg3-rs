//! Static data combat needs: factions and relations, action resource definitions, the combat
//! fields of `Character` stats entries, weapons, armour, equipment sets, spell sets, spells,
//! status boosts, and the per-object / per-template combat attributes (`Equipment`,
//! `SkillList`, `SpellSet`, `CombatGroupID`).
//!
//! | Data | Where |
//! |---|---|
//! | faction parents | `Public/<Module>/Factions/Factions.lsx`: `UUID`, `ParentGuid` |
//! | relations | `Public/<Module>/Factions/Relations.lsx`, region `FactionManager`: `Relation` `Source`, `Target`, `Value` (0 hostile, 50 neutral, 100 allied) |
//! | action resources | `Public/<Module>/ActionResourceDefinitions/ActionResourceDefinitions.lsx`: `Name`, `UUID`, `ReplenishType`, `MaxLevel` |
//! | rule constants | stats `Data.txt`: `InitiativeDie`, `JoinAllyInCombatRadius` |
//! | per character | stats `Character`: `Initiative`, `ActionResources`, `Proficiency Group`, `UnarmedAttackAbility`, `*Resistance` |
//! | weapons / armour | stats `Weapon` / `Armor` entries |
//! | equipment sets | stats `Equipment.txt` (`new equipment`) |
//! | spells | stats `SpellData` entries |
//! | per object | level / globals `Characters/_merged.lsf` and root templates: `Equipment`, `SpellSet`, `SkillList/Skill@Skill`, `CombatGroupID` |

use std::collections::HashMap;

use bg3_rules::context::{Ability, DamageType, Resistance};
use bg3_rules::dice::RollDefinition;
use bg3_rules::fields::{BoostEntry, parse_boosts};
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::guid::Guid;
use crate::load::{LoadError, game_objects, guid_attr, parse, s};

/// The faction tree and the relation table.
#[derive(Clone, Debug, Default)]
pub struct Factions {
    pub parent: HashMap<Guid, Guid>,
    /// `(source, target) → value`.
    pub relations: HashMap<(Guid, Guid), i32>,
}

/// Relation values (`eoc::relation`): 0 is hostile, 50 neutral (the default), 100 allied.
pub const RELATION_HOSTILE: i32 = 0;
pub const RELATION_NEUTRAL: i32 = 50;
pub const RELATION_ALLIED: i32 = 100;

impl Factions {
    /// A faction and its ancestors, the faction first.
    pub fn tree(&self, f: Guid) -> Vec<Guid> {
        let mut out = vec![f];
        let mut cur = f;
        while let Some(&p) = self.parent.get(&cur) {
            if p.is_null() || out.contains(&p) || out.len() > 32 {
                break;
            }
            out.push(p);
            cur = p;
        }
        out
    }

    /// The relation of faction `source` towards faction `target`: for the source and then
    /// each of its ancestors, the first relation found towards the target or one of the
    /// target's ancestors (nearest first); 50 (neutral) if none.
    // bg3: eoc::relation::GetFactionRelation → (anonymous)::GetFactionTree,
    // GetFactionRelationRecursive (4.1.1.7631656). The engine first consults the story's
    // runtime relations (SetRelation) and then the data; the sim keeps both in one table.
    pub fn relation(&self, source: Guid, target: Guid) -> i32 {
        let targets = self.tree(target);
        for s in self.tree(source) {
            for t in &targets {
                if let Some(&v) = self.relations.get(&(s, *t)) {
                    return v;
                }
            }
        }
        RELATION_NEUTRAL
    }
}

/// When an action resource refills (`ReplenishType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replenish {
    Turn,
    ShortRest,
    Rest,
    Never,
    Other,
}

impl Replenish {
    pub fn parse(s: &str) -> Self {
        match s {
            "Turn" => Self::Turn,
            "ShortRest" => Self::ShortRest,
            "Rest" | "FullRest" => Self::Rest,
            "Never" => Self::Never,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionResourceDef {
    pub name: String,
    pub id: Guid,
    pub replenish: Replenish,
    pub max_level: u32,
}

/// The combat fields of a `Character` stats entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharacterCombatDef {
    /// `Initiative`.
    pub initiative: i32,
    /// `ActionResources`: `Name:amount` (`Name:amount:level` for leveled resources, level
    /// ignored).
    pub action_resources: Vec<(String, f32)>,
    /// `Proficiency Group`.
    pub proficiency: Vec<String>,
    pub unarmed_ability: Option<Ability>,
    /// `<Type>Resistance` fields.
    pub resistances: Vec<(DamageType, Resistance)>,
    /// `DefaultBoosts` `AC(n)` boosts.
    pub ac_boosts: i32,
}

/// Equipment slot of a weapon or armour (`Slot`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    MeleeMain,
    MeleeOff,
    RangedMain,
    RangedOff,
    Breast,
    Other,
}

impl Slot {
    pub fn parse(s: &str) -> Self {
        match s {
            "Melee Main Weapon" => Self::MeleeMain,
            "Melee Offhand Weapon" => Self::MeleeOff,
            "Ranged Main Weapon" => Self::RangedMain,
            "Ranged Offhand Weapon" => Self::RangedOff,
            "Breast" => Self::Breast,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WeaponDef {
    pub name: String,
    pub damage: RollDefinition,
    pub damage_type: DamageType,
    /// `WeaponRange`, metres (stats: centimetres).
    pub range: f32,
    pub properties: Vec<String>,
    pub proficiency: Vec<String>,
    pub slot: Slot,
}

impl WeaponDef {
    pub fn has(&self, property: &str) -> bool {
        self.properties.iter().any(|p| p == property)
    }

    pub fn ranged(&self) -> bool {
        matches!(self.slot, Slot::RangedMain | Slot::RangedOff) || self.has("Ammunition")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArmorDef {
    pub name: String,
    pub armor_class: i32,
    /// `Armor Class Ability`.
    pub ability: Option<Ability>,
    /// `Ability Modifier Cap`; 0 or less = no cap.
    pub ability_cap: i32,
    pub shield: bool,
    pub slot: Slot,
    pub proficiency: Vec<String>,
}

/// The fields of a `SpellData` entry the sim uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellDef {
    pub name: String,
    /// `SpellType`: `Target`, `Projectile`, `Shout`, `Zone`, …
    pub spell_type: String,
    /// `TargetRadius`: a number (metres) or a weapon-range keyword.
    pub target_radius: String,
    /// `UseCosts`: `Name:amount` (`Name:amount:level`, level ignored).
    pub use_costs: Vec<(String, f32)>,
    pub flags: Vec<String>,
    /// Raw expression fields for `bg3_rules::exec::Spell::from_fields`.
    pub fields: Vec<(String, String)>,
}

impl SpellDef {
    pub fn field(&self, k: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }

    /// The spell for the rules engine's cast pipeline.
    pub fn to_exec(&self) -> Result<bg3_rules::exec::Spell, bg3_rules::exec::SpellError> {
        bg3_rules::exec::Spell::from_fields(&self.name, |k| self.field(k))
    }
}

/// Combat attributes of a template or placed object; empty fields inherit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectCombat {
    pub equipment: Option<String>,
    pub spell_set: Option<String>,
    /// `SkillList/Skill@Skill`.
    pub skills: Vec<String>,
    pub combat_group: Option<String>,
}

/// Everything combat reads from the install.
#[derive(Clone, Debug)]
pub struct CombatData {
    pub factions: Factions,
    pub resources: Vec<ActionResourceDef>,
    pub characters: HashMap<String, CharacterCombatDef>,
    pub weapons: HashMap<String, WeaponDef>,
    pub armors: HashMap<String, ArmorDef>,
    /// Equipment set → groups of item stats names.
    pub equipment_sets: HashMap<String, Vec<Vec<String>>>,
    pub spell_sets: HashMap<String, Vec<String>>,
    pub spells: HashMap<String, SpellDef>,
    /// `Boosts` of `StatusData` entries.
    pub status_boosts: HashMap<String, String>,
    /// Templates by id.
    pub templates: HashMap<Guid, ObjectCombat>,
    /// Placed objects (levels and globals) by id.
    pub objects: HashMap<Guid, ObjectCombat>,
    /// `InitiativeDie` from `Data.txt` (faces; 4 in the reference build).
    pub initiative_die: u32,
    /// `JoinAllyInCombatRadius` from `Data.txt` (metres; 7 in the reference build).
    pub join_ally_radius: f32,
    /// `BaseSpellDifficultyCheck` from `Data.txt`.
    pub base_spell_dc: i32,
    /// Condition helper scripts (`Mods/<Module>/Scripts/thoth/helpers/*.khn`): `(path, source)`.
    pub helper_scripts: Vec<(String, String)>,
    /// Classes and subclasses (`ClassDescriptions.lsx`) by `UUID`.
    pub classes: HashMap<Guid, ClassDef>,
    /// Origins (`Origins.lsx`) by the origin character's global object (`GlobalTemplate`).
    pub origins: HashMap<Guid, OriginDef>,
    /// Races (`Races.lsx`): `UUID → ProgressionTableUUID`.
    pub race_tables: HashMap<Guid, Guid>,
    /// Progressions (`Progressions.lsx`) by `TableUUID`.
    pub progressions: HashMap<Guid, Vec<ProgressionDef>>,
    /// Spell lists (`Lists/SpellLists.lsx`) by `UUID`.
    pub spell_lists: HashMap<Guid, Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassDef {
    pub base_hp: i32,
    pub hp_per_level: i32,
    pub progression_table: Option<Guid>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OriginDef {
    pub class: Option<Guid>,
    pub subclass: Option<Guid>,
    pub race: Option<Guid>,
    pub subrace: Option<Guid>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgressionDef {
    pub id: Guid,
    pub level: i32,
    pub boosts: String,
    pub selectors: String,
}

impl Default for CombatData {
    fn default() -> Self {
        Self::new()
    }
}

fn split_list(v: Option<&str>) -> Vec<String> {
    v.unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `Name:amount[:level]` lists (`ActionResources`, `UseCosts`).
pub fn parse_costs(v: &str) -> Vec<(String, f32)> {
    v.split(';')
        .filter_map(|e| {
            let mut it = e.trim().split(':');
            let name = it.next()?.trim();
            let amount: f32 = it.next()?.trim().parse().ok()?;
            (!name.is_empty()).then(|| (name.to_owned(), amount))
        })
        .collect()
}

fn resistance(v: &str) -> Option<Resistance> {
    match v.trim() {
        "Vulnerable" => Some(Resistance::Vulnerable),
        "Resistant" | "ResistantToMagical" | "ResistantToNonMagical" => Some(Resistance::Resistant),
        "Immune" | "ImmuneToMagical" | "ImmuneToNonMagical" => Some(Resistance::Immune),
        _ => None,
    }
}

impl CombatData {
    pub fn new() -> Self {
        Self {
            factions: Factions::default(),
            resources: Vec::new(),
            characters: HashMap::new(),
            weapons: HashMap::new(),
            armors: HashMap::new(),
            equipment_sets: HashMap::new(),
            spell_sets: HashMap::new(),
            spells: HashMap::new(),
            status_boosts: HashMap::new(),
            templates: HashMap::new(),
            objects: HashMap::new(),
            initiative_die: 4,
            join_ally_radius: 7.0,
            base_spell_dc: bg3_rules::rules::DEFAULT_BASE_SPELL_DC,
            helper_scripts: Vec::new(),
            classes: HashMap::new(),
            origins: HashMap::new(),
            race_tables: HashMap::new(),
            progressions: HashMap::new(),
            spell_lists: HashMap::new(),
        }
    }

    /// Maximum hit points of a character of `class` at `level` with Constitution modifier
    /// `con`: `BaseHp + con` at level 1, `HpPerLevel + con` for every further level.
    // *Verified by trace* (combat-20261021-183321): Lae'zel (Fighter, BaseHp 10, Con 14)
    // has 12 HP at level 1; the trace's cleric avatar (BaseHp 8, Con 12) 9 HP.
    pub fn class_hp(&self, class: Guid, level: i32, con: i32) -> Option<i32> {
        let c = self.classes.get(&class)?;
        Some(c.base_hp + con + (level.max(1) - 1) * (c.hp_per_level + con))
    }

    /// The progression tables of an origin character (race, subrace, class, subclass).
    pub fn origin_tables(&self, origin: &OriginDef) -> Vec<Guid> {
        let race = [origin.race, origin.subrace]
            .into_iter()
            .flatten()
            .filter_map(|r| self.race_tables.get(&r).copied());
        let class = [origin.class, origin.subclass]
            .into_iter()
            .flatten()
            .filter_map(|c| self.classes.get(&c).and_then(|c| c.progression_table));
        race.chain(class).collect()
    }

    /// Everything that comes from stats (no VFS needed).
    pub fn add_stats(&mut self, stats: &bg3_stats::Stats) {
        if let Some(v) = stats
            .key("InitiativeDie")
            .and_then(|v| v.trim().parse().ok())
        {
            self.initiative_die = v;
        }
        if let Some(v) = stats
            .key("JoinAllyInCombatRadius")
            .and_then(|v| v.trim().parse().ok())
        {
            self.join_ally_radius = v;
        }
        if let Some(v) = stats
            .key("BaseSpellDifficultyCheck")
            .and_then(|v| v.trim().parse().ok())
        {
            self.base_spell_dc = v;
        }
        for o in stats.entries() {
            match o.type_name.as_deref() {
                Some("Character") => {
                    let mut d = CharacterCombatDef {
                        initiative: o
                            .get("Initiative")
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0),
                        action_resources: parse_costs(o.get("ActionResources").unwrap_or("")),
                        proficiency: split_list(o.get("Proficiency Group")),
                        unarmed_ability: o
                            .get("UnarmedAttackAbility")
                            .and_then(|v| Ability::parse(v.trim())),
                        ..Default::default()
                    };
                    for t in DamageType::ALL {
                        if let Some(r) = o
                            .get(&format!("{}Resistance", t.name()))
                            .and_then(resistance)
                        {
                            d.resistances.push((*t, r));
                        }
                    }
                    if let Ok(entries) = parse_boosts(o.get("DefaultBoosts").unwrap_or("")) {
                        for e in entries {
                            if let BoostEntry::Boost(b) = e
                                && b.condition.is_none()
                                && b.name == "AC"
                                && let Some(n) =
                                    b.args.first().and_then(|a| a.trim().parse::<i32>().ok())
                            {
                                d.ac_boosts += n;
                            }
                        }
                    }
                    self.characters.insert(o.name.clone(), d);
                }
                Some("Weapon") => {
                    let damage = o.get("Damage").and_then(RollDefinition::parse);
                    let dt = o
                        .get("Damage Type")
                        .and_then(|v| DamageType::parse(v.trim()));
                    if let (Some(damage), Some(damage_type)) = (damage, dt) {
                        self.weapons.insert(
                            o.name.clone(),
                            WeaponDef {
                                name: o.name.clone(),
                                damage,
                                damage_type,
                                range: o
                                    .get("WeaponRange")
                                    .and_then(|v| v.trim().parse::<f32>().ok())
                                    .unwrap_or(150.0)
                                    / 100.0,
                                properties: split_list(o.get("Weapon Properties")),
                                proficiency: split_list(o.get("Proficiency Group")),
                                slot: Slot::parse(o.get("Slot").unwrap_or("").trim()),
                            },
                        );
                    }
                }
                Some("Armor") => {
                    self.armors.insert(
                        o.name.clone(),
                        ArmorDef {
                            name: o.name.clone(),
                            armor_class: o
                                .get("ArmorClass")
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0),
                            ability: o
                                .get("Armor Class Ability")
                                .and_then(|v| Ability::parse(v.trim())),
                            ability_cap: o
                                .get("Ability Modifier Cap")
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0),
                            shield: o.get("Shield").is_some_and(|v| v.trim() == "Yes"),
                            slot: Slot::parse(o.get("Slot").unwrap_or("").trim()),
                            proficiency: split_list(o.get("Proficiency Group")),
                        },
                    );
                }
                Some("SpellData") => {
                    const KEYS: &[&str] = &[
                        "TargetConditions",
                        "SpellRoll",
                        "SpellSuccess",
                        "SpellFail",
                        "SpellProperties",
                        "DamageType",
                        "SpellSchool",
                    ];
                    self.spells.insert(
                        o.name.clone(),
                        SpellDef {
                            name: o.name.clone(),
                            spell_type: o.get("SpellType").unwrap_or("").trim().to_owned(),
                            target_radius: o.get("TargetRadius").unwrap_or("").trim().to_owned(),
                            use_costs: parse_costs(o.get("UseCosts").unwrap_or("")),
                            flags: split_list(o.get("SpellFlags")),
                            fields: KEYS
                                .iter()
                                .filter_map(|k| Some(((*k).to_owned(), o.get(k)?.to_owned())))
                                .collect(),
                        },
                    );
                }
                Some("StatusData") => {
                    if let Some(b) = o.get("Boosts").filter(|b| !b.trim().is_empty()) {
                        self.status_boosts.insert(o.name.clone(), b.to_owned());
                    }
                }
                _ => {}
            }
        }
        for e in stats.equipment.iter() {
            self.equipment_sets.insert(e.name.clone(), e.groups.clone());
        }
        for s in stats.spell_sets.iter() {
            self.spell_sets.insert(s.name.clone(), s.spells.clone());
        }
    }

    /// Load from the install: stats plus the VFS resources, with the combat attributes of the
    /// objects of the given levels (and every level's globals).
    pub fn load(vfs: &Vfs, stats: &bg3_stats::Stats, levels: &[String]) -> Result<Self, LoadError> {
        let mut d = Self::new();
        d.add_stats(stats);
        load_factions(vfs, &mut d.factions)?;
        d.resources = load_resources(vfs)?;
        d.helper_scripts = bg3_rules::install::helper_scripts(vfs, BASE_LOAD_ORDER);
        load_classes(vfs, &mut d)?;
        for m in BASE_LOAD_ORDER {
            let path = format!("Public/{m}/RootTemplates/_merged.lsf");
            if vfs.exists(&path) {
                read_combat_objects(vfs, &path, &mut d.templates)?;
            }
        }
        for m in BASE_LOAD_ORDER {
            let prefix = format!("Mods/{m}/Globals/");
            let mut files: Vec<String> = vfs
                .files_under(&prefix)
                .map(|f| f.entry.name.clone())
                .filter(|n| n.ends_with("/Characters/_merged.lsf"))
                .collect();
            files.sort();
            for f in files {
                read_combat_objects(vfs, &f, &mut d.objects)?;
            }
            for l in levels {
                let path = format!("Mods/{m}/Levels/{l}/Characters/_merged.lsf");
                if vfs.exists(&path) {
                    read_combat_objects(vfs, &path, &mut d.objects)?;
                }
            }
        }
        Ok(d)
    }

    pub fn resource(&self, name: &str) -> Option<&ActionResourceDef> {
        self.resources.iter().find(|r| r.name == name)
    }

    /// `Initiative(n)` boosts of a status, summed.
    pub fn status_initiative(&self, status: &str) -> i32 {
        boost_sum(
            self.status_boosts.get(status).map(String::as_str),
            "Initiative",
        )
    }

    /// `AC(n)` boosts of a status, summed.
    pub fn status_ac(&self, status: &str) -> i32 {
        boost_sum(self.status_boosts.get(status).map(String::as_str), "AC")
    }
}

/// Sum of the first (integer) argument of unconditional boosts named `name`.
pub fn boost_sum(boosts: Option<&str>, name: &str) -> i32 {
    let Some(Ok(entries)) = boosts.map(parse_boosts) else {
        return 0;
    };
    entries
        .into_iter()
        .filter_map(|e| match e {
            BoostEntry::Boost(b) if b.condition.is_none() && b.name == name => {
                b.args.first().and_then(|a| a.trim().parse::<i32>().ok())
            }
            _ => None,
        })
        .sum()
}

fn load_factions(vfs: &Vfs, out: &mut Factions) -> Result<(), LoadError> {
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/Factions/Factions.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            if let Some(root) = res.region("FactionContainer") {
                let list = root.children_named("Faction").chain(
                    root.children_named("root")
                        .flat_map(|r| r.children_named("Faction")),
                );
                for n in list {
                    if let (Some(id), Some(p)) = (guid_attr(n, "UUID"), guid_attr(n, "ParentGuid"))
                        && !p.is_null()
                    {
                        out.parent.insert(id, p);
                    }
                }
            }
        }
        let path = format!("Public/{m}/Factions/Relations.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            if let Some(root) = res.region("FactionManager") {
                let rels = root
                    .children
                    .iter()
                    .flat_map(|r| {
                        std::iter::once(r)
                            .chain(r.children.iter())
                            .chain(r.children.iter().flat_map(|c| c.children.iter()))
                    })
                    .filter(|n| n.name == "Relation");
                for n in rels {
                    let v = n.attr("Value").and_then(|v| v.as_i64());
                    if let (Some(a), Some(b), Some(v)) =
                        (guid_attr(n, "Source"), guid_attr(n, "Target"), v)
                    {
                        out.relations.insert((a, b), v as i32);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Children named `name` of a region's node, directly or under `root`.
fn region_nodes<'a>(
    res: &'a bg3_lsf::Resource,
    region: &str,
    name: &'a str,
) -> Vec<&'a bg3_lsf::Node> {
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

fn load_classes(vfs: &Vfs, d: &mut CombatData) -> Result<(), LoadError> {
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/ClassDescriptions/ClassDescriptions.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            for n in region_nodes(&res, "ClassDescriptions", "ClassDescription") {
                let int = |k: &str| n.attr(k).and_then(|v| v.as_i64()).map(|v| v as i32);
                if let Some(id) = guid_attr(n, "UUID") {
                    d.classes.insert(
                        id,
                        ClassDef {
                            base_hp: int("BaseHp").unwrap_or(0),
                            hp_per_level: int("HpPerLevel").unwrap_or(0),
                            progression_table: guid_attr(n, "ProgressionTableUUID"),
                        },
                    );
                }
            }
        }
        let path = format!("Public/{m}/Origins/Origins.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            for n in region_nodes(&res, "Origins", "Origin") {
                if let Some(obj) = guid_attr(n, "GlobalTemplate") {
                    d.origins.insert(
                        obj,
                        OriginDef {
                            class: guid_attr(n, "ClassUUID"),
                            subclass: guid_attr(n, "SubClassUUID"),
                            race: guid_attr(n, "RaceUUID"),
                            subrace: guid_attr(n, "SubRaceUUID"),
                        },
                    );
                }
            }
        }
        let path = format!("Public/{m}/Races/Races.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            for n in region_nodes(&res, "Races", "Race") {
                if let (Some(id), Some(t)) =
                    (guid_attr(n, "UUID"), guid_attr(n, "ProgressionTableUUID"))
                {
                    d.race_tables.insert(id, t);
                }
            }
        }
        let path = format!("Public/{m}/Progressions/Progressions.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            for n in region_nodes(&res, "Progressions", "Progression") {
                if let Some(t) = guid_attr(n, "TableUUID") {
                    let p = ProgressionDef {
                        id: guid_attr(n, "UUID").unwrap_or(Guid::NULL),
                        level: n.attr("Level").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                        boosts: s(n, "Boosts").unwrap_or_default(),
                        selectors: s(n, "Selectors").unwrap_or_default(),
                    };
                    // A later module's progression with the same UUID replaces the earlier.
                    let list = d.progressions.entry(t).or_default();
                    match list.iter_mut().find(|x| x.id == p.id && !p.id.is_null()) {
                        Some(x) => *x = p,
                        None => list.push(p),
                    }
                }
            }
        }
        let path = format!("Public/{m}/Lists/SpellLists.lsx");
        if vfs.exists(&path) {
            let res = parse(vfs, &path)?;
            for n in region_nodes(&res, "SpellLists", "SpellList") {
                if let Some(id) = guid_attr(n, "UUID") {
                    d.spell_lists
                        .insert(id, split_list(s(n, "Spells").as_deref()));
                }
            }
        }
    }
    Ok(())
}

fn load_resources(vfs: &Vfs) -> Result<Vec<ActionResourceDef>, LoadError> {
    let mut out: Vec<ActionResourceDef> = Vec::new();
    for m in BASE_LOAD_ORDER {
        let path = format!("Public/{m}/ActionResourceDefinitions/ActionResourceDefinitions.lsx");
        if !vfs.exists(&path) {
            continue;
        }
        let res = parse(vfs, &path)?;
        let Some(root) = res.region("ActionResourceDefinitions") else {
            continue;
        };
        let list = root.children_named("ActionResourceDefinition").chain(
            root.children_named("root")
                .flat_map(|r| r.children_named("ActionResourceDefinition")),
        );
        for n in list {
            let (Some(name), Some(id)) = (s(n, "Name"), guid_attr(n, "UUID")) else {
                continue;
            };
            let def = ActionResourceDef {
                name,
                id,
                replenish: Replenish::parse(&s(n, "ReplenishType").unwrap_or_default()),
                max_level: n.attr("MaxLevel").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
            };
            match out.iter_mut().find(|d| d.id == def.id) {
                Some(d) => *d = def,
                None => out.push(def),
            }
        }
    }
    Ok(out)
}

fn read_combat_objects(
    vfs: &Vfs,
    path: &str,
    out: &mut HashMap<Guid, ObjectCombat>,
) -> Result<(), LoadError> {
    let res = parse(vfs, path)?;
    for n in game_objects(&res) {
        let Some(id) = guid_attr(n, "MapKey") else {
            continue;
        };
        if s(n, "Type").as_deref() != Some("character") {
            continue;
        }
        let skills: Vec<String> = n
            .child("SkillList")
            .into_iter()
            .flat_map(|l| l.children_named("Skill"))
            .filter_map(|k| s(k, "Skill"))
            .collect();
        let c = ObjectCombat {
            equipment: s(n, "Equipment"),
            spell_set: s(n, "SpellSet"),
            skills,
            combat_group: s(n, "CombatGroupID"),
        };
        if c != ObjectCombat::default() {
            out.insert(id, c);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(n: u128) -> Guid {
        Guid(n)
    }

    #[test]
    fn relation_walks_both_trees() {
        let mut f = Factions::default();
        // 1 ← 2 (2's parent is 1); 10 ← 11.
        f.parent.insert(g(2), g(1));
        f.parent.insert(g(11), g(10));
        f.relations.insert((g(1), g(10)), 0);
        assert_eq!(f.relation(g(2), g(11)), 0, "inherited on both sides");
        f.relations.insert((g(2), g(10)), 100);
        assert_eq!(f.relation(g(2), g(11)), 100, "the source's own entry first");
        assert_eq!(
            f.relation(g(10), g(1)),
            RELATION_NEUTRAL,
            "relations are directed"
        );
    }

    #[test]
    fn costs_and_boosts() {
        assert_eq!(
            parse_costs("ActionPoint:1;Movement:9; SpellSlot:1:1"),
            [
                ("ActionPoint".to_owned(), 1.0),
                ("Movement".to_owned(), 9.0),
                ("SpellSlot".to_owned(), 1.0)
            ]
        );
        assert_eq!(boost_sum(Some("Initiative(-20);AC(2)"), "Initiative"), -20);
        assert_eq!(
            boost_sum(Some("IF(Combat()):Initiative(5)"), "Initiative"),
            0
        );
    }
}
