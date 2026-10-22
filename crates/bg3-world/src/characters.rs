//! Characters: level character → root template chain → `CharacterVisualBank` resource → visuals,
//! skeleton and animation sets. See `docs/research/characters.md`.
//!
//! Everything here is pure data. The resolution rules that are inferred rather than confirmed
//! against the original are marked "assumption" and listed in the research note.

use std::collections::HashMap;

use bg3_lsf::Node;

use crate::banks::{ResourceBanks, VisualResource};
use crate::level::LevelObject;
use crate::str_attr;
use crate::templates::RootTemplates;

/// Character fields stored on a level object or a root template. `None` = not set here
/// (inherit from the template / parent template).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharacterProps {
    /// `CharacterVisualResourceID` → `CharacterVisualBank`.
    pub character_visual: Option<String>,
    /// `AnimationSetResourceID` → `AnimationSetBank`: an extra set consulted before the
    /// visual's own waterfall.
    pub animation_set: Option<String>,
    /// `Equipment`: a stats `new equipment` entry (`Equipment.txt`).
    pub equipment: Option<String>,
    /// `Stats`: the character's stats entry.
    pub stats: Option<String>,
    /// `EquipmentRace`: GUID selecting race-specific equipment visuals.
    pub equipment_race: Option<String>,
    /// `DefaultState` (observed 11 on dead thralls; meaning not yet traced).
    pub default_state: Option<i64>,
    /// `MovementSpeedRun` in m/s (default [`DEFAULT_MOVEMENT_SPEED_RUN`]).
    pub movement_speed_run: Option<f32>,
    /// `MovementSpeedWalk` in m/s (default [`DEFAULT_MOVEMENT_SPEED_WALK`]).
    pub movement_speed_walk: Option<f32>,
    /// `MovementStepUpHeight` in metres (default [`DEFAULT_MOVEMENT_STEP_UP_HEIGHT`]).
    pub movement_step_up_height: Option<f32>,
}

// bg3: eoc::CharacterTemplate::Visit (4.1.1.7631656) — defaults of the movement fields when a
// template chain doesn't set them (Stroll 1.0, Walk 2.0, Run 3.75, Sprint 6.0, Dash 6.0,
// StepUpHeight 0.6).
pub const DEFAULT_MOVEMENT_SPEED_WALK: f32 = 2.0;
pub const DEFAULT_MOVEMENT_SPEED_RUN: f32 = 3.75;
pub const DEFAULT_MOVEMENT_STEP_UP_HEIGHT: f32 = 0.6;

impl CharacterProps {
    pub fn read(n: &Node) -> Self {
        Self {
            character_visual: str_attr(n, "CharacterVisualResourceID"),
            animation_set: str_attr(n, "AnimationSetResourceID"),
            equipment: str_attr(n, "Equipment"),
            stats: str_attr(n, "Stats"),
            equipment_race: str_attr(n, "EquipmentRace").filter(|s| !is_null_guid(s)),
            default_state: n.attr("DefaultState").and_then(|v| v.as_i64()),
            movement_speed_run: f32_attr(n, "MovementSpeedRun"),
            movement_speed_walk: f32_attr(n, "MovementSpeedWalk"),
            movement_step_up_height: f32_attr(n, "MovementStepUpHeight"),
        }
    }
}

fn f32_attr(n: &Node, name: &str) -> Option<f32> {
    n.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32)
}

pub(crate) fn is_null_guid(s: &str) -> bool {
    s.is_empty() || s == "00000000-0000-0000-0000-000000000000"
}

/// A material parameter override on a character visual (`MaterialOverrides`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParamOverride {
    pub parameter: String,
    /// Scalar → 1 value, `Vector3Parameters` → 3, `VectorParameters` → 4.
    pub value: Vec<f32>,
    /// Whether the override is active (`Enabled`); disabled entries are editor leftovers.
    pub enabled: bool,
}

/// A `MaterialPresets` entry: material group → `MaterialPresetBank` resource.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialPresetRef {
    pub group: String,
    pub preset: String,
    pub force: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialOverrides {
    pub material_resource: Option<String>,
    pub presets: Vec<MaterialPresetRef>,
    pub color_preset: Option<String>,
    pub params: Vec<ParamOverride>,
}

impl MaterialOverrides {
    /// Read a `MaterialOverrides` node (also the `Presets` node of a `MaterialPresetBank`).
    pub(crate) fn read(n: &Node) -> Self {
        let mut out = Self {
            material_resource: str_attr(n, "MaterialResource"),
            ..Default::default()
        };
        for c in &n.children {
            match c.name.as_str() {
                "MaterialPresets" => {
                    for o in c.children_named("Object") {
                        if let Some(preset) = str_attr(o, "MaterialPresetResource") {
                            out.presets.push(MaterialPresetRef {
                                group: str_attr(o, "GroupName")
                                    .or_else(|| str_attr(o, "MapKey"))
                                    .unwrap_or_default(),
                                preset,
                                force: bool_attr(o, "ForcePresetValues"),
                            });
                        }
                    }
                }
                "ColorPreset" => out.color_preset = str_attr(c, "MaterialPresetResource"),
                "ScalarParameters" | "Vector3Parameters" | "VectorParameters" => {
                    let Some(parameter) = str_attr(c, "Parameter") else {
                        continue;
                    };
                    let value = match c.attr("Value") {
                        Some(v) => v
                            .components()
                            .map(|v| v.into_iter().map(|x| x as f32).collect())
                            .or_else(|| v.as_f64().map(|x| vec![x as f32]))
                            .unwrap_or_default(),
                        None => Vec::new(),
                    };
                    out.params.push(ParamOverride {
                        parameter,
                        value,
                        enabled: bool_attr(c, "Enabled"),
                    });
                }
                _ => {}
            }
        }
        out
    }
}

fn bool_attr(n: &Node, name: &str) -> bool {
    n.attr(name).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// One `Slots` entry of a character visual.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualSlot {
    /// `Head`, `Body`, `Hair`, `Beard`, `Private Parts`, … (free-form names).
    pub slot: String,
    /// `VisualBank` resource.
    pub visual: String,
    /// Bone to attach a rigid visual to (empty for skinned visuals).
    pub bone: Option<String>,
}

/// A `CharacterVisualBank` resource: how to assemble one character's look.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharacterVisualResource {
    pub id: String,
    pub name: String,
    /// `VisualBank` resource holding the skeleton (`SkeletonResource`) and the animation
    /// waterfall; it usually has no meshes (`Template = EMPTY_VISUAL_TEMPLATE`).
    pub base_visual: Option<String>,
    /// `VisualBank` resource of the body (`Slot = NakedBody` for creatures).
    pub body_set_visual: Option<String>,
    pub slots: Vec<VisualSlot>,
    pub material_overrides: MaterialOverrides,
    /// Whether equipment visuals are drawn on top (false on most NPC presets).
    pub show_equipment_visuals: bool,
}

impl CharacterVisualResource {
    pub(crate) fn read(n: &Node) -> Self {
        let mut out = Self {
            id: str_attr(n, "ID").unwrap_or_default(),
            name: str_attr(n, "Name").unwrap_or_default(),
            base_visual: str_attr(n, "BaseVisual"),
            body_set_visual: str_attr(n, "BodySetVisual"),
            show_equipment_visuals: bool_attr(n, "ShowEquipmentVisuals"),
            ..Default::default()
        };
        for c in &n.children {
            match c.name.as_str() {
                "Slots" => {
                    if let Some(visual) = str_attr(c, "VisualResource") {
                        out.slots.push(VisualSlot {
                            slot: str_attr(c, "Slot").unwrap_or_default(),
                            visual,
                            bone: str_attr(c, "Bone"),
                        });
                    }
                }
                "MaterialOverrides" => out.material_overrides = MaterialOverrides::read(c),
                _ => {}
            }
        }
        out
    }
}

/// A `SkeletonBank` resource.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkeletonResource {
    pub id: String,
    pub name: String,
    /// GR2 holding the skeleton.
    pub source_file: Option<String>,
    /// `<gr2 path>.<root bone>.0`.
    pub template: Option<String>,
}

impl SkeletonResource {
    pub(crate) fn read(n: &Node) -> Self {
        Self {
            id: str_attr(n, "ID").unwrap_or_default(),
            name: str_attr(n, "Name").unwrap_or_default(),
            source_file: str_attr(n, "SourceFile"),
            template: str_attr(n, "Template"),
        }
    }
}

/// An `AnimationBank` resource: one animation clip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimationResource {
    pub id: String,
    pub name: String,
    /// GR2 holding the animation.
    pub source_file: Option<String>,
    /// `<gr2 path>.<animation name>.<n>`.
    pub template: Option<String>,
    pub skeleton: Option<String>,
    pub duration: f32,
    pub looping: bool,
}

impl AnimationResource {
    pub(crate) fn read(n: &Node) -> Self {
        Self {
            id: str_attr(n, "ID").unwrap_or_default(),
            name: str_attr(n, "Name").unwrap_or_default(),
            source_file: str_attr(n, "SourceFile"),
            template: str_attr(n, "Template"),
            skeleton: str_attr(n, "SkeletonResource"),
            duration: n
                .attr("Duration")
                .and_then(|v| v.as_f64())
                .unwrap_or_default() as f32,
            looping: bool_attr(n, "Looping"),
        }
    }
}

/// One animation subset of an animation set: a table animation-type GUID → `AnimationBank` ID.
///
/// Subsets are keyed by a GUID (the empty key is the default subset); which one is active
/// depends on the character's state (observed: per-weapon-type subsets such as one-handed or
/// crossbow next to the default `DFLT` animations).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimationSubset {
    /// Subset GUID; `""` for the default subset.
    pub key: String,
    pub fallback: Option<String>,
    /// Animation type ("simple name") GUID → `AnimationBank` resource ID.
    pub animations: HashMap<String, String>,
}

/// An `AnimationSetBank` resource.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimationSetResource {
    pub id: String,
    pub name: String,
    pub subsets: Vec<AnimationSubset>,
}

impl AnimationSetResource {
    pub(crate) fn read(n: &Node) -> Self {
        let mut out = Self {
            id: str_attr(n, "ID").unwrap_or_default(),
            name: str_attr(n, "Name").unwrap_or_default(),
            subsets: Vec::new(),
        };
        let subsets = n
            .children_named("AnimationSet")
            .flat_map(|s| s.children_named("AnimationBank"))
            .flat_map(|b| b.children_named("AnimationSubSets"))
            .flat_map(|s| s.children_named("Object"));
        for s in subsets {
            let animations = s
                .children_named("Animation")
                .flat_map(|a| a.children_named("Object"))
                .filter_map(|o| Some((str_attr(o, "MapKey")?, str_attr(o, "ID")?)))
                .collect();
            out.subsets.push(AnimationSubset {
                key: str_attr(s, "MapKey").unwrap_or_default(),
                fallback: str_attr(s, "FallBackSubSet"),
                animations,
            });
        }
        out
    }

    pub fn subset(&self, key: &str) -> Option<&AnimationSubset> {
        self.subsets.iter().find(|s| s.key == key)
    }

    /// Look up an animation type in subset `key`, following `FallBackSubSet` links and finally
    /// the default subset (assumption: the default subset is the last resort).
    pub fn find(&self, key: &str, simple_name: &str) -> Option<&str> {
        let mut current = Some(key);
        for _ in 0..8 {
            let Some(k) = current else { break };
            let Some(sub) = self.subset(k) else { break };
            if let Some(id) = sub.animations.get(simple_name) {
                return Some(id);
            }
            current = sub.fallback.as_deref();
        }
        self.subset("")?
            .animations
            .get(simple_name)
            .map(String::as_str)
    }
}

/// Character-related banks, indexed by resource ID (filled by [`ResourceBanks`]).
#[derive(Debug, Default)]
pub struct CharacterBanks {
    pub character_visuals: HashMap<String, CharacterVisualResource>,
    pub skeletons: HashMap<String, SkeletonResource>,
    pub animations: HashMap<String, AnimationResource>,
    pub animation_sets: HashMap<String, AnimationSetResource>,
}

impl CharacterBanks {
    /// The character override precedes the visual's shared waterfall. Within each set use
    /// the requested subset, its fallback links, then the default subset, just as for idle.
    // bg3: ls::AnimationSetSystem::Update (4.1.1.7631656)
    /// Order is inferred from resources; not verified against the engine implementation.
    pub fn animation_in_sets(
        &self,
        sets: &[String],
        subset: &str,
        simple_name: &str,
    ) -> Option<&AnimationResource> {
        sets.iter()
            .filter_map(|id| self.animation_sets.get(id))
            .find_map(|set| set.find(subset, simple_name))
            .and_then(|id| self.animations.get(id))
    }
}

/// The animation-type ("simple name") GUID the character blueprint asks for when standing
/// still out of combat: the default of the `IDLE_DesiredStillShortName` variable in
/// `Public/Shared/Assets/Blueprints/Characters_Common.lsbs`. Read at runtime with
/// [`blueprint_variable_default`]; this constant is only the reference build's value, used as a
/// fallback and in tests.
pub const IDLE_STILL_PEACE: &str = "07e81afc-95c4-452b-ac68-5cbb74710cca";

/// The common character animation blueprint (compiled "Genome" resource).
pub const CHARACTER_BLUEPRINT: &str = "Public/Shared/Assets/Blueprints/Characters_Common.lsbs";

/// Animation type of the forward run loop out of combat (reference build value; every animation
/// set maps it to a `*_DFLT_LOCO_RUN_F_PEACE_01` clip).
pub const RUN_PEACE: &str = "821aed88-e17b-4b85-b37a-533f90fbba84";

/// Animation type of the forward walk loop out of combat (maps to `*_DFLT_LOCO_WALK_F_PEACE_01`).
pub const WALK_PEACE: &str = "0532ad73-988e-4b70-bb96-434bb2532bad";

/// Blueprint variable holding the idle animation type.
pub const IDLE_BLUEPRINT_VARIABLE: &str = "IDLE_DesiredStillShortName";

/// Named combat clips used by the viewer. Selection is a presentation approximation, not
/// the original animation blueprint's weapon/spell/direction state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CombatAnimation {
    Melee,
    Ranged,
    Cast,
    Hit,
    Death,
    Dead,
    Downed,
}

impl CombatAnimation {
    pub const ALL: [Self; 7] = [
        Self::Melee,
        Self::Ranged,
        Self::Cast,
        Self::Hit,
        Self::Death,
        Self::Dead,
        Self::Downed,
    ];

    /// Identifiers observed in Shared/Animation/ShortNames.lsx and the HUM_M/IMP banks.
    pub fn simple_name(self) -> &'static str {
        match self {
            Self::Melee => "CMBT_Melee_RHand_01_Attack",
            Self::Ranged => "CMBT_Range_RHand_01_Attack",
            Self::Cast => "SPL_Somatic_Target_01_Cast",
            Self::Hit => "REAC_Hit_F_Combat_01",
            Self::Death => "DIE_Physical_Stationary_Front_01",
            Self::Dead => "DIE_Generic_Deathpose_01",
            Self::Downed => "STAT_Downed_Peace_01_Loop",
        }
    }
}

/// Read the install's simple-name → GUID table, without embedding extracted tables.
pub fn animation_simple_names(vfs: &bg3_vfs::Vfs) -> HashMap<String, String> {
    crate::parse(vfs, "Public/Shared/Animation/ShortNames.lsx")
        .map(|r| read_simple_names(&r))
        .unwrap_or_default()
}

fn read_simple_names(res: &bg3_lsf::Resource) -> HashMap<String, String> {
    res.region("ShortNames")
        .into_iter()
        .flat_map(|root| root.children_named("ShortName"))
        .filter_map(|n| Some((str_attr(n, "Name")?, str_attr(n, "UUID")?)))
        .collect()
}

/// The idle animation type from the character blueprint, or [`IDLE_STILL_PEACE`].
pub fn idle_simple_name(vfs: &bg3_vfs::Vfs) -> String {
    crate::parse(vfs, CHARACTER_BLUEPRINT)
        .ok()
        .and_then(|r| blueprint_variable_default(&r, IDLE_BLUEPRINT_VARIABLE))
        .unwrap_or_else(|| IDLE_STILL_PEACE.to_owned())
}

/// The default value of a blueprint (`.lsbs`/`.lsbc` "Genome") variable, by name.
pub fn blueprint_variable_default(res: &bg3_lsf::Resource, name: &str) -> Option<String> {
    res.region("Genome")?
        .children_named("Variables")
        .find(|v| v.attr_str("Name") == Some(name))?
        .child("Value")
        .and_then(|v| str_attr(v, "Value"))
}

/// A visual to draw for a character, with the slot it fills.
#[derive(Debug, Clone)]
pub struct CharacterPart {
    /// `BodySet` for the body set visual, otherwise the slot name.
    pub slot: String,
    pub visual: VisualResource,
    /// Bone to attach to for rigid parts.
    pub attach_bone: Option<String>,
}

/// Item template's `Equipment` child. Empty maps are sparse template overrides.
#[derive(Debug, Clone, Default)]
pub struct EquipmentVisuals {
    pub slots: Vec<String>,
    pub visuals: HashMap<String, Vec<String>>,
    /// A null GUID explicitly stops fallback; an absent key uses `DefaultParent`.
    pub parent_race: HashMap<String, String>,
    pub material_overrides: MaterialOverrides,
}

impl EquipmentVisuals {
    // bg3: eoc::EquipmentData::Visit (4.1.1.7631656)
    pub(crate) fn read(n: &Node) -> Self {
        Self {
            slots: n
                .children_named("Slot")
                .filter_map(|s| str_attr(s, "Object"))
                .collect(),
            visuals: n
                .children_named("Visuals")
                .flat_map(|v| v.children_named("Object"))
                .filter_map(|o| {
                    Some((
                        str_attr(o, "MapKey")?,
                        o.children_named("MapValue")
                            .filter_map(|v| str_attr(v, "Object"))
                            .collect(),
                    ))
                })
                .collect(),
            parent_race: n
                .children_named("ParentRace")
                .flat_map(|v| v.children_named("Object"))
                .filter_map(|o| Some((str_attr(o, "MapKey")?, str_attr(o, "MapValue")?)))
                .collect(),
            material_overrides: n
                .child("VisualSet")
                .and_then(|v| v.child("MaterialOverrides"))
                .map(MaterialOverrides::read)
                .unwrap_or_default(),
        }
    }
}

/// Install-owned equipment mappings. No race tables are embedded in the executable.
#[derive(Debug, Default)]
pub struct EquipmentSettings {
    pub race_parents: HashMap<String, String>,
    pub slot_names: HashMap<String, String>,
    /// Named `NakedBodySystem` regions → vertex-colour byte values.
    pub body_mask_values: HashMap<String, u8>,
    pub attachments: HashMap<String, Vec<String>>,
}

impl EquipmentSettings {
    fn read(&mut self, res: &bg3_lsf::Resource) {
        if let Some(root) = res.region("EquipmentRaces") {
            for race in root.children_named("EquipmentRace") {
                if let Some(id) = str_attr(race, "Guid") {
                    // Later module definitions can remove an earlier parent too.
                    self.race_parents.remove(&id);
                    if let Some(parent) = str_attr(race, "DefaultParent") {
                        self.race_parents.insert(id, parent);
                    }
                }
            }
        }
        for region in &res.regions {
            for child in &region.node.children {
                match child.name.as_str() {
                    "EquipmentSlots" => {
                        self.slot_names
                            .extend(child.children_named("Object").filter_map(|n| {
                                Some((str_attr(n, "MapKey")?, str_attr(n, "MapValue")?))
                            }))
                    }
                    "NakedBodySystem" => {
                        self.body_mask_values
                            .extend(child.children_named("Object").filter_map(|n| {
                                Some((str_attr(n, "MapKey")?, n.attr("MapValue")?.as_i64()? as u8))
                            }))
                    }
                    "AttachmentsMappings" => {
                        self.attachments
                            .extend(child.children_named("Object").filter_map(|n| {
                                Some((
                                    str_attr(n, "MapKey")?,
                                    n.children_named("MapValue")
                                        .filter_map(|v| str_attr(v, "Object"))
                                        .collect(),
                                ))
                            }))
                    }
                    _ => {}
                }
            }
        }
    }

    pub(crate) fn load(vfs: &bg3_vfs::Vfs, modules: &[&str]) -> crate::Result<Self> {
        let mut out = Self::default();
        for module in modules {
            for file in [
                "EquipmentRaces.lsx",
                "EquipmentSlots.lsx",
                "VisualSlots.lsx",
                "VisualAttachmentMappings.lsf",
            ] {
                let path = format!("Mods/{module}/EquipmentSettings/{file}");
                if vfs.exists(&path) {
                    out.read(&crate::parse(vfs, &path)?);
                }
            }
        }
        Ok(out)
    }
}

/// A mask request, retaining names even when an install has no matching byte mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyMask {
    pub slot: String,
    pub value: Option<u8>,
}

/// A level character resolved to the data needed to draw and animate it.
#[derive(Debug, Clone)]
pub struct ResolvedCharacter {
    /// Level object `MapKey`.
    pub id: String,
    pub name: String,
    pub template: Option<String>,
    pub props: CharacterProps,
    pub character_visual: CharacterVisualResource,
    pub skeleton: Option<SkeletonResource>,
    pub parts: Vec<CharacterPart>,
    /// Union of active visuals' `VertexColorMaskSlots`; consumed by character shaders.
    pub body_masks: Vec<BodyMask>,
    /// Animation sets in lookup order.
    pub animation_sets: Vec<String>,
    /// The idle animation, if found.
    pub idle: Option<AnimationResource>,
}

/// An item from a character's starting equipment, with the visual its template uses.
#[derive(Debug, Clone)]
pub struct EquippedItem {
    /// Stats entry (e.g. `WPN_Imp_Handaxe`).
    pub stats: String,
    pub is_weapon: bool,
    /// Stats `Slot` (`Melee Main Weapon`, `Melee Offhand Weapon`, `Ranged Main Weapon`, `Breast`, …).
    pub slot: String,
    /// Item root template (stats `RootTemplate`).
    pub template: String,
    pub visual: Option<VisualResource>,
    /// Worn visuals for the requested equipment race (never the dropped mesh).
    pub worn: Vec<CharacterPart>,
    pub hides_slots: Vec<String>,
    pub body_masks: Vec<BodyMask>,
    pub material_overrides: MaterialOverrides,
}

/// Each equipment group contributes its first entry. This selection remains an assumption:
/// the data supplies alternative names but no conditions/weights (see the research note).
pub fn resolve_equipment(
    equipment: &str,
    race: Option<&str>,
    stats: &bg3_stats::Stats,
    templates: &RootTemplates,
    banks: &ResourceBanks,
) -> Vec<EquippedItem> {
    let Some(eq) = stats.equipment.get(equipment) else {
        return Vec::new();
    };
    resolve_equipment_items(
        eq.groups
            .iter()
            .filter_map(|g| g.first())
            .map(String::as_str),
        race,
        stats,
        templates,
        banks,
    )
}

/// Resolve explicit item stats names, including the sim's created character equipment list.
pub fn resolve_equipment_items<'a>(
    items: impl IntoIterator<Item = &'a str>,
    race: Option<&str>,
    stats: &bg3_stats::Stats,
    templates: &RootTemplates,
    banks: &ResourceBanks,
) -> Vec<EquippedItem> {
    items
        .into_iter()
        .filter_map(|entry| {
            let obj = stats.get(entry)?;
            let slot = obj.get("Slot").unwrap_or_default();
            // Starting sets contain inventory supplies and camp outfits too. The viewer currently
            // displays the adventuring set; it has no armour-set toggle.
            if slot.is_empty() || slot.starts_with("Vanity") {
                return None;
            }
            let template = obj.get("RootTemplate")?.to_owned();
            let equipment = templates.equipment_for(&template);
            let worn = race
                .and_then(|r| equipment.visuals_for(r, &banks.equipment))
                .into_iter()
                .flatten()
                .filter_map(|id| banks.visuals.get(id))
                .map(|v| CharacterPart {
                    slot: v.slot.clone().unwrap_or_default(),
                    attach_bone: v.attach_bone.clone(),
                    visual: v.clone(),
                })
                .collect::<Vec<_>>();
            let mut hides_slots = equipment.slots.clone();
            if hides_slots.is_empty()
                && !worn.is_empty()
                && let Some(name) = banks.equipment.slot_names.get(slot)
            {
                hides_slots.push(name.clone());
            }
            for p in &worn {
                if !p.slot.is_empty() && !hides_slots.contains(&p.slot) {
                    hides_slots.push(p.slot.clone());
                }
            }
            // Do not remove a preset part when no wearable visual could be resolved.
            if worn.is_empty() {
                hides_slots.clear();
            }
            Some(EquippedItem {
                stats: entry.to_owned(),
                is_weapon: obj.type_name.as_deref() == Some("Weapon"),
                slot: slot.to_owned(),
                visual: templates
                    .visual_for(&template)
                    .and_then(|id| banks.visuals.get(id))
                    .cloned(),
                template,
                body_masks: body_masks(&worn, &banks.equipment),
                worn,
                hides_slots,
                material_overrides: equipment.material_overrides,
            })
        })
        .collect()
}

impl EquipmentVisuals {
    // bg3: eoc::EquipmentData::ResolveAppliedRace (4.1.1.7631656)
    // bg3: eoc::EquipmentData::GetVisualResources (4.1.1.7631656)
    /// Direct visual first, then per-item ParentRace, then settings DefaultParent.
    /// Ordering/null semantics are data-inferred; no engine runtime trace was captured.
    fn visuals_for<'a>(
        &'a self,
        race: &'a str,
        settings: &'a EquipmentSettings,
    ) -> Option<&'a [String]> {
        let mut current = race;
        let mut visited = std::collections::HashSet::new();
        while !is_null_guid(current) && visited.insert(current) {
            if let Some(visuals) = self.visuals.get(current) {
                return Some(visuals);
            }
            current = self
                .parent_race
                .get(current)
                .or_else(|| settings.race_parents.get(current))?;
        }
        None
    }
}

impl RootTemplates {
    /// Merge sparse equipment fields from parent to child; explicit race entries (including
    /// empty lists/null parents) override inherited entries. Data-inferred assumption.
    pub fn equipment_for(&self, id: &str) -> EquipmentVisuals {
        let mut chain = Vec::new();
        let mut current = self.get(id);
        for _ in 0..32 {
            let Some(t) = current else { break };
            if let Some(eq) = &t.equipment {
                chain.push(eq);
            }
            current = t.parent.as_deref().and_then(|p| self.get(p));
        }
        let mut out = EquipmentVisuals::default();
        for eq in chain.into_iter().rev() {
            if !eq.slots.is_empty() {
                out.slots.clone_from(&eq.slots);
            }
            out.visuals.extend(eq.visuals.clone());
            out.parent_race.extend(eq.parent_race.clone());
            if eq.material_overrides != MaterialOverrides::default() {
                out.material_overrides = eq.material_overrides.clone();
            }
        }
        out
    }
}

impl ResolvedCharacter {
    /// Replace matching preset slots and add every part of each wearable, preserving the
    /// naked body for partial vertex-colour cuts. Call once on a freshly resolved character.
    // bg3: eoc::EquipmentData::GetVisualResources (4.1.1.7631656)
    pub fn apply_equipment(&mut self, items: &[EquippedItem], settings: &EquipmentSettings) {
        if self.character_visual.show_equipment_visuals {
            self.parts
                .retain(|p| !items.iter().any(|i| i.hides_slots.contains(&p.slot)));
            self.parts
                .extend(items.iter().flat_map(|i| i.worn.iter().cloned()));
        }
        self.body_masks = body_masks(&self.parts, settings);
    }
}

fn body_masks(parts: &[CharacterPart], settings: &EquipmentSettings) -> Vec<BodyMask> {
    let mut names: Vec<_> = parts
        .iter()
        .flat_map(|p| p.visual.vertex_color_mask_slots.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|slot| BodyMask {
            value: settings.body_mask_values.get(&slot).copied(),
            slot,
        })
        .collect()
}

/// Why a character could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharacterError {
    NotACharacter,
    NoCharacterVisual,
    CharacterVisualNotFound(String),
}

impl std::fmt::Display for CharacterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACharacter => write!(f, "not a character"),
            Self::NoCharacterVisual => {
                write!(f, "no CharacterVisualResourceID on object or templates")
            }
            Self::CharacterVisualNotFound(id) => write!(f, "character visual {id} not in banks"),
        }
    }
}

impl RootTemplates {
    /// Walk a template and its parents and return the first character field `f` finds.
    pub fn character_field<'a>(
        &'a self,
        id: &str,
        f: impl Fn(&'a CharacterProps) -> Option<&'a String>,
    ) -> Option<&'a str> {
        let mut current = self.get(id)?;
        for _ in 0..32 {
            if let Some(v) = current.character.as_ref().and_then(&f) {
                return Some(v);
            }
            current = self.get(current.parent.as_deref()?)?;
        }
        None
    }
}

/// Resolve every character field of a level object: the object first, then its template chain.
pub fn effective_props(obj: &LevelObject, templates: &RootTemplates) -> CharacterProps {
    let own = obj.character.clone().unwrap_or_default();
    let Some(t) = obj.template.as_deref() else {
        return own;
    };
    let pick = |mine: Option<String>, f: fn(&CharacterProps) -> Option<&String>| {
        mine.or_else(|| templates.character_field(t, f).map(str::to_owned))
    };
    let default_state = own.default_state.or_else(|| {
        let mut cur = templates.get(t);
        for _ in 0..32 {
            let c = cur?;
            if let Some(s) = c.character.as_ref().and_then(|p| p.default_state) {
                return Some(s);
            }
            cur = templates.get(c.parent.as_deref()?);
        }
        None
    });
    let pick_f32 = |mine: Option<f32>, f: fn(&CharacterProps) -> Option<f32>| {
        mine.or_else(|| {
            let mut cur = templates.get(t);
            for _ in 0..32 {
                let c = cur?;
                if let Some(v) = c.character.as_ref().and_then(f) {
                    return Some(v);
                }
                cur = templates.get(c.parent.as_deref()?);
            }
            None
        })
    };
    CharacterProps {
        character_visual: pick(own.character_visual, |p| p.character_visual.as_ref()),
        animation_set: pick(own.animation_set, |p| p.animation_set.as_ref()),
        equipment: pick(own.equipment, |p| p.equipment.as_ref()),
        stats: pick(own.stats, |p| p.stats.as_ref()),
        equipment_race: pick(own.equipment_race, |p| p.equipment_race.as_ref()),
        default_state,
        movement_speed_run: pick_f32(own.movement_speed_run, |p| p.movement_speed_run),
        movement_speed_walk: pick_f32(own.movement_speed_walk, |p| p.movement_speed_walk),
        movement_step_up_height: pick_f32(own.movement_step_up_height, |p| {
            p.movement_step_up_height
        }),
    }
}

impl ResolvedCharacter {
    /// Look up an animation type in the character's animation sets (default subset), in the
    /// same order as the idle.
    pub fn animation(&self, banks: &ResourceBanks, simple_name: &str) -> Option<AnimationResource> {
        banks
            .characters
            .animation_in_sets(&self.animation_sets, "", simple_name)
            .cloned()
    }
}

/// Resolve a level character to its visuals, skeleton and idle animation.
///
/// `idle_simple_name` is the animation type to look up (see [`IDLE_STILL_PEACE`]).
pub fn resolve_character(
    obj: &LevelObject,
    templates: &RootTemplates,
    banks: &ResourceBanks,
    idle_simple_name: &str,
) -> Result<ResolvedCharacter, CharacterError> {
    if obj.kind != "character" {
        return Err(CharacterError::NotACharacter);
    }
    let props = effective_props(obj, templates);
    let cv_id = props
        .character_visual
        .as_deref()
        .ok_or(CharacterError::NoCharacterVisual)?;
    let cv = banks
        .characters
        .character_visuals
        .get(cv_id)
        .ok_or_else(|| CharacterError::CharacterVisualNotFound(cv_id.to_owned()))?
        .clone();

    let base = cv.base_visual.as_deref().and_then(|v| banks.visuals.get(v));
    let skeleton = base
        .and_then(|b| b.skeleton_resource.as_deref())
        .and_then(|s| banks.characters.skeletons.get(s))
        .cloned();

    let mut parts = Vec::new();
    let mut push = |slot: &str, id: &str, bone: Option<&String>| {
        if let Some(v) = banks.visuals.get(id) {
            parts.push(CharacterPart {
                slot: slot.to_owned(),
                visual: v.clone(),
                attach_bone: bone.cloned().or_else(|| v.attach_bone.clone()),
            });
        }
    };
    if let Some(b) = cv.body_set_visual.as_deref() {
        push("BodySet", b, None);
    }
    for s in &cv.slots {
        push(&s.slot, &s.visual, s.bone.as_ref());
    }

    // Assumption: the template's own animation set overrides the visual's waterfall, which is
    // consulted in file order.
    let mut animation_sets: Vec<String> = props.animation_set.iter().cloned().collect();
    if let Some(b) = base {
        animation_sets.extend(b.animation_waterfall.iter().cloned());
    }
    let idle = banks
        .characters
        .animation_in_sets(&animation_sets, "", idle_simple_name)
        .cloned();

    Ok(ResolvedCharacter {
        id: obj.id.clone(),
        name: obj.name.clone(),
        template: obj.template.clone(),
        props,
        character_visual: cv,
        skeleton,
        body_masks: body_masks(&parts, &banks.equipment),
        parts,
        animation_sets,
        idle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_lsf::Value;

    fn fs(s: &str) -> Value {
        Value::FixedString(s.to_owned())
    }

    fn race_visual(race: &str, ids: &[&str]) -> Node {
        let mut n = Node::new("Object").with_attr("MapKey", fs(race));
        for id in ids {
            n.children
                .push(Node::new("MapValue").with_attr("Object", fs(id)));
        }
        n
    }

    #[test]
    fn wearable_race_fallback_preserves_parts_and_explicit_empty_or_null() {
        let mut eq = EquipmentVisuals::read(
            &Node::new("Equipment")
                .with_child(Node::new("Slot").with_attr("Object", fs("Body")))
                .with_child(
                    Node::new("Visuals").with_child(race_visual("parent", &["shirt", "pants"])),
                ),
        );
        let mut settings = EquipmentSettings::default();
        settings
            .race_parents
            .insert("child".into(), "parent".into());
        assert_eq!(eq.slots, ["Body"]);
        assert_eq!(
            eq.visuals_for("child", &settings).unwrap(),
            ["shirt", "pants"]
        );
        eq.parent_race.insert(
            "child".into(),
            "00000000-0000-0000-0000-000000000000".into(),
        );
        assert!(eq.visuals_for("child", &settings).is_none());
        eq.visuals.insert("child".into(), vec![]);
        assert_eq!(eq.visuals_for("child", &settings), Some([].as_slice()));
        eq.visuals.remove("child");
        eq.parent_race.insert("child".into(), "child".into());
        assert!(eq.visuals_for("child", &settings).is_none());
        eq.parent_race.insert("child".into(), "missing".into());
        assert!(eq.visuals_for("child", &settings).is_none());
    }

    fn synthetic_templates(nodes: Vec<Node>) -> RootTemplates {
        let mut root = Node::new("root");
        root.children = nodes;
        let res = bg3_lsf::Resource {
            metadata: Default::default(),
            regions: vec![bg3_lsf::Region {
                name: "Templates".into(),
                node: root,
            }],
        };
        RootTemplates {
            templates: crate::templates::game_objects(&res)
                .map(|t| (t.id.clone(), t))
                .collect(),
        }
    }

    #[test]
    fn explicit_cc_items_and_groups_resolve_without_a_dropped_visual() {
        let templates = synthetic_templates(vec![
            Node::new("GameObjects")
                .with_attr("MapKey", fs("item"))
                .with_child(Node::new("Equipment").with_child(
                    Node::new("Visuals").with_child(race_visual("race", &["armor", "pants"])),
                )),
        ]);
        let mut stats = bg3_stats::Stats::new();
        stats
            .load_str(
                "test",
                "test",
                r#"
new entry "shirt"
type "Armor"
data "Slot" "Breast"
data "RootTemplate" "item"
new entry "camp"
type "Armor"
data "Slot" "VanityBody"
data "RootTemplate" "item"
new equipment "set"
add equipmentgroup
add equipment entry "shirt"
add equipment entry "camp"
"#,
            )
            .unwrap();
        stats.finish();
        let mut banks = ResourceBanks::default();
        for id in ["armor", "pants"] {
            banks.visuals.insert(
                id.into(),
                VisualResource {
                    id: id.into(),
                    slot: Some("Body".into()),
                    ..Default::default()
                },
            );
        }
        let items = resolve_equipment("set", Some("race"), &stats, &templates, &banks);
        assert_eq!(items.len(), 1);
        assert!(items[0].visual.is_none());
        assert_eq!(
            items[0]
                .worn
                .iter()
                .map(|p| p.visual.id.as_str())
                .collect::<Vec<_>>(),
            ["armor", "pants"]
        );
        assert_eq!(items[0].hides_slots, ["Body"]);
        let cc_items = resolve_equipment_items(
            ["camp", "shirt", "unknown"],
            Some("race"),
            &stats,
            &templates,
            &banks,
        );
        assert_eq!(cc_items.len(), 1);
        assert_eq!(cc_items[0].stats, "shirt");
        let unresolved = resolve_equipment("set", Some("missing_race"), &stats, &templates, &banks);
        assert!(unresolved[0].worn.is_empty());
        assert!(unresolved[0].hides_slots.is_empty());
    }

    #[test]
    fn settings_read_race_slot_and_mask_mappings_from_nodes() {
        let res = bg3_lsf::Resource {
            metadata: Default::default(),
            regions: vec![
                bg3_lsf::Region {
                    name: "EquipmentRaces".into(),
                    node: Node::new("root").with_child(
                        Node::new("EquipmentRace")
                            .with_attr("Guid", fs("child"))
                            .with_attr("DefaultParent", fs("parent")),
                    ),
                },
                bg3_lsf::Region {
                    name: "VisualSetSlotsSystem".into(),
                    node: Node::new("root")
                        .with_child(
                            Node::new("EquipmentSlots").with_child(
                                Node::new("Object")
                                    .with_attr("MapKey", fs("Boots"))
                                    .with_attr("MapValue", fs("Footwear")),
                            ),
                        )
                        .with_child(
                            Node::new("NakedBodySystem").with_child(
                                Node::new("Object")
                                    .with_attr("MapKey", fs("TestRegion"))
                                    .with_attr("MapValue", Value::Byte(123)),
                            ),
                        ),
                },
            ],
        };
        let mut settings = EquipmentSettings::default();
        settings.read(&res);
        assert_eq!(settings.race_parents["child"], "parent");
        assert_eq!(settings.slot_names["Boots"], "Footwear");
        assert_eq!(settings.body_mask_values["TestRegion"], 123);
    }

    #[test]
    fn sparse_item_templates_merge_race_entries_and_inherit_slots() {
        let parent = Node::new("GameObjects")
            .with_attr("MapKey", fs("parent"))
            .with_child(
                Node::new("Equipment")
                    .with_child(Node::new("Slot").with_attr("Object", fs("Body")))
                    .with_child(Node::new("Visuals").with_child(race_visual("race", &["old"]))),
            );
        let child = Node::new("GameObjects")
            .with_attr("MapKey", fs("child"))
            .with_attr("ParentTemplateId", fs("parent"))
            .with_child(
                Node::new("Equipment").with_child(
                    Node::new("Visuals")
                        .with_child(race_visual("race", &["new", "pants"]))
                        .with_child(race_visual("other", &[])),
                ),
            );
        let eq = synthetic_templates(vec![parent, child]).equipment_for("child");
        assert_eq!(eq.slots, ["Body"]);
        assert_eq!(eq.visuals["race"], ["new", "pants"]);
        assert!(eq.visuals["other"].is_empty());
    }

    #[test]
    fn equipment_replaces_preset_slots_keeps_naked_body_and_exposes_masks() {
        let mut banks = ResourceBanks::default();
        let mut body = VisualResource {
            id: "body".into(),
            slot: Some("NakedBody".into()),
            ..Default::default()
        };
        banks.visuals.insert(body.id.clone(), body.clone());
        body.id = "old_shirt".into();
        banks.visuals.insert(body.id.clone(), body);
        banks.characters.character_visuals.insert(
            "cv".into(),
            CharacterVisualResource::read(
                &Node::new("Resource")
                    .with_attr("ID", fs("cv"))
                    .with_attr("ShowEquipmentVisuals", Value::Bool(true))
                    .with_attr("BodySetVisual", fs("body"))
                    .with_child(
                        Node::new("Slots")
                            .with_attr("Slot", fs("Body"))
                            .with_attr("VisualResource", fs("old_shirt")),
                    ),
            ),
        );
        let obj = LevelObject {
            id: "test".into(),
            name: "test".into(),
            kind: "character".into(),
            template: None,
            visual_template: None,
            is_blocker: None,
            transform: Default::default(),
            module: String::new(),
            character: Some(CharacterProps {
                character_visual: Some("cv".into()),
                ..Default::default()
            }),
        };
        let mut c = resolve_character(&obj, &RootTemplates::default(), &banks, "idle").unwrap();
        let shirt = VisualResource {
            vertex_color_mask_slots: vec!["Torso".into(), "Unknown".into()],
            ..Default::default()
        };
        let item = EquippedItem {
            stats: "shirt".into(),
            is_weapon: false,
            slot: "Breast".into(),
            template: "item".into(),
            visual: None,
            hides_slots: vec!["Body".into()],
            worn: vec![CharacterPart {
                slot: "Body".into(),
                visual: shirt,
                attach_bone: None,
            }],
            material_overrides: Default::default(),
            body_masks: vec![],
        };
        banks.equipment.body_mask_values.insert("Torso".into(), 73);
        let original = c.clone();
        c.apply_equipment(std::slice::from_ref(&item), &banks.equipment);
        assert_eq!(c.parts.len(), 2);
        assert_eq!(c.parts[0].visual.id, "body");
        assert!(!c.parts.iter().any(|p| p.visual.id == "old_shirt"));
        assert_eq!(
            c.body_masks,
            [
                BodyMask {
                    slot: "Torso".into(),
                    value: Some(73)
                },
                BodyMask {
                    slot: "Unknown".into(),
                    value: None
                }
            ]
        );
        let mut disabled = original;
        disabled.character_visual.show_equipment_visuals = false;
        disabled.apply_equipment(&[item], &banks.equipment);
        assert_eq!(disabled.parts[1].visual.id, "old_shirt");
        assert!(disabled.body_masks.is_empty());
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn worn_armour_resolves_laezel_and_human_fighter() {
        let Ok(install) = bg3_install::Install::locate() else {
            eprintln!("skipping: no BG3 install found");
            return;
        };
        let vfs = bg3_vfs::Vfs::from_install(&install).unwrap();
        let templates = RootTemplates::load(&vfs).unwrap();
        let banks = ResourceBanks::load(&vfs).unwrap();
        let stats =
            bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER).unwrap();
        let globals = crate::parse(
            &vfs,
            "Mods/Gustav/Globals/WLD_Main_A/Characters/_merged.lsf",
        )
        .unwrap();
        let laezel = globals
            .region("Templates")
            .unwrap()
            .children_named("GameObjects")
            .find(|n| n.attr_str("Name") == Some("S_Player_Laezel"))
            .unwrap();
        let laezel_template = laezel.attr_str("TemplateName").unwrap();
        let human = templates
            .templates
            .values()
            .find(|t| t.name == "Humans_Male_Player_Dev")
            .unwrap();
        for (template, equipment) in [
            (laezel_template, "EQ_Laezel"),
            (human.id.as_str(), "EQP_CC_Fighter"),
        ] {
            let race = templates
                .character_field(template, |p| p.equipment_race.as_ref())
                .unwrap();
            let items = resolve_equipment(equipment, Some(race), &stats, &templates, &banks);
            let armor = items.iter().find(|i| i.slot == "Breast").unwrap();
            assert!(!armor.worn.is_empty(), "{equipment}: no armour for {race}");
            for p in &armor.worn {
                assert!(vfs.exists(p.visual.source_file.as_deref().unwrap()));
                eprintln!("{equipment} {race}: {}", p.visual.name);
            }
        }
    }

    fn anim(key: &str, id: &str) -> Node {
        Node::new("Object")
            .with_attr("ID", fs(id))
            .with_attr("MapKey", fs(key))
    }

    fn subset(key: &str, fallback: &str, anims: Vec<Node>) -> Node {
        let mut list = Node::new("Animation");
        list.children = anims;
        Node::new("Object")
            .with_attr("FallBackSubSet", fs(fallback))
            .with_attr("MapKey", fs(key))
            .with_child(list)
    }

    fn set(subsets: Vec<Node>) -> AnimationSetResource {
        let mut s = Node::new("AnimationSubSets");
        s.children = subsets;
        let n = Node::new("Resource")
            .with_attr("ID", fs("set"))
            .with_attr("Name", fs("Test"))
            .with_child(
                Node::new("AnimationSet").with_child(Node::new("AnimationBank").with_child(s)),
            );
        AnimationSetResource::read(&n)
    }

    #[test]
    fn animation_set_lookup_follows_fallbacks() {
        let s = set(vec![
            subset(
                "",
                "",
                vec![anim("idle", "dflt_idle"), anim("walk", "dflt_walk")],
            ),
            subset("onehand", "", vec![anim("idle", "1h_idle")]),
            subset("dual", "onehand", vec![anim("attack", "dual_attack")]),
        ]);
        assert_eq!(s.subsets.len(), 3);
        assert_eq!(s.find("", "idle"), Some("dflt_idle"));
        assert_eq!(s.find("onehand", "idle"), Some("1h_idle"));
        assert_eq!(s.find("onehand", "walk"), Some("dflt_walk"));
        assert_eq!(s.find("dual", "idle"), Some("1h_idle"));
        assert_eq!(s.find("dual", "attack"), Some("dual_attack"));
        assert_eq!(s.find("missing", "walk"), Some("dflt_walk"));
        assert_eq!(s.find("", "run"), None);
    }

    #[test]
    fn animation_waterfall_prefers_override_and_falls_back_to_shared_set() {
        let mut banks = CharacterBanks::default();
        banks.animation_sets.insert(
            "own".into(),
            set(vec![subset("", "", vec![anim("melee", "own_melee")])]),
        );
        banks.animation_sets.insert(
            "shared".into(),
            set(vec![subset(
                "",
                "",
                vec![anim("melee", "shared_melee"), anim("death", "shared_death")],
            )]),
        );
        for id in ["own_melee", "shared_melee", "shared_death"] {
            banks.animations.insert(
                id.into(),
                AnimationResource {
                    id: id.into(),
                    ..Default::default()
                },
            );
        }
        let chain = vec!["missing_set".into(), "own".into(), "shared".into()];
        assert_eq!(
            banks.animation_in_sets(&chain, "", "melee").unwrap().id,
            "own_melee"
        );
        assert_eq!(
            banks.animation_in_sets(&chain, "", "death").unwrap().id,
            "shared_death"
        );
        assert!(banks.animation_in_sets(&chain, "", "ranged").is_none());
    }

    #[test]
    fn subset_cycle_and_missing_link_still_reach_default() {
        let s = set(vec![
            subset("a", "b", vec![]),
            subset("b", "a", vec![]),
            subset("broken", "absent", vec![]),
            subset("", "", vec![anim("hit", "default_hit")]),
        ]);
        assert_eq!(s.find("a", "hit"), Some("default_hit"));
        assert_eq!(s.find("broken", "hit"), Some("default_hit"));
        assert_eq!(s.find("a", "missing"), None);
    }

    #[test]
    fn simple_names_are_read_from_resource_not_a_compiled_guid_table() {
        let resource = bg3_lsf::Resource {
            metadata: Default::default(),
            regions: vec![bg3_lsf::Region {
                name: "ShortNames".into(),
                node: Node::new("root")
                    .with_child(
                        Node::new("ShortName")
                            .with_attr("Name", fs("test_attack"))
                            .with_attr("UUID", fs("test-guid")),
                    )
                    .with_child(Node::new("ShortName").with_attr("Name", fs("incomplete"))),
            }],
        };
        let names = read_simple_names(&resource);
        assert_eq!(
            names.get("test_attack").map(String::as_str),
            Some("test-guid")
        );
        assert!(!names.contains_key("incomplete"));
    }

    #[test]
    fn character_visual_reads_slots_and_overrides() {
        let n = Node::new("Resource")
            .with_attr("BaseVisual", fs("base"))
            .with_attr("BodySetVisual", fs("body"))
            .with_attr("ID", fs("cv"))
            .with_attr("Name", fs("Test_Body"))
            .with_attr("ShowEquipmentVisuals", Value::Bool(true))
            .with_child(
                Node::new("MaterialOverrides")
                    .with_attr("MaterialResource", fs(""))
                    .with_child(
                        Node::new("MaterialPresets").with_child(
                            Node::new("Object")
                                .with_attr("ForcePresetValues", Value::Bool(true))
                                .with_attr("GroupName", fs("03Hair"))
                                .with_attr("MapKey", fs("03Hair"))
                                .with_attr("MaterialPresetResource", fs("preset")),
                        ),
                    )
                    .with_child(
                        Node::new("ScalarParameters")
                            .with_attr("Enabled", Value::Bool(true))
                            .with_attr("Parameter", fs("GlowMultiplier"))
                            .with_attr("Value", Value::Float(2.0)),
                    ),
            )
            .with_child(
                Node::new("Slots")
                    .with_attr("Bone", fs(""))
                    .with_attr("Slot", fs("Head"))
                    .with_attr("VisualResource", fs("head")),
            )
            .with_child(
                Node::new("Slots")
                    .with_attr("Bone", fs("Dummy_Hat"))
                    .with_attr("Slot", fs("Hat"))
                    .with_attr("VisualResource", fs("hat")),
            );
        let cv = CharacterVisualResource::read(&n);
        assert_eq!(cv.base_visual.as_deref(), Some("base"));
        assert_eq!(cv.body_set_visual.as_deref(), Some("body"));
        assert!(cv.show_equipment_visuals);
        assert_eq!(cv.slots.len(), 2);
        assert_eq!(cv.slots[0].bone, None);
        assert_eq!(cv.slots[1].bone.as_deref(), Some("Dummy_Hat"));
        assert_eq!(cv.material_overrides.presets[0].preset, "preset");
        assert!(cv.material_overrides.presets[0].force);
        assert_eq!(
            cv.material_overrides.params,
            vec![ParamOverride {
                parameter: "GlowMultiplier".into(),
                value: vec![2.0],
                enabled: true
            }]
        );
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn nautiloid_characters_resolve() {
        let Ok(install) = bg3_install::Install::locate() else {
            eprintln!("skipping: no BG3 install found");
            return;
        };
        let vfs = bg3_vfs::Vfs::from_install(&install).unwrap();
        let level = crate::Level::load(&vfs, "TUT_Avernus_C").unwrap();
        let templates = RootTemplates::load(&vfs).unwrap();
        let banks = ResourceBanks::load(&vfs).unwrap();
        let blueprint = crate::parse(&vfs, CHARACTER_BLUEPRINT).unwrap();
        let idle = blueprint_variable_default(&blueprint, IDLE_BLUEPRINT_VARIABLE).unwrap();
        let simple_names = animation_simple_names(&vfs);
        assert!(
            CombatAnimation::ALL
                .iter()
                .all(|k| simple_names.contains_key(k.simple_name()))
        );
        assert_eq!(idle, IDLE_STILL_PEACE);

        let chars: Vec<_> = level
            .objects
            .iter()
            .filter(|o| o.kind == "character")
            .collect();
        let stats =
            bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER).unwrap();
        let mut equipped = 0;
        let mut resolved = 0;
        let mut with_skeleton = 0;
        let mut with_idle = 0;
        let mut failures = Vec::new();
        for o in &chars {
            match resolve_character(o, &templates, &banks, &idle) {
                Ok(c) => {
                    resolved += 1;
                    if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
                        for kind in CombatAnimation::ALL {
                            let clip = c.animation(&banks, &simple_names[kind.simple_name()]);
                            eprintln!(
                                "combat clip {} {kind:?}: {}",
                                c.name,
                                clip.as_ref().map_or("MISSING", |a| a.name.as_str())
                            );
                        }
                    }
                    let skel_ok = c
                        .skeleton
                        .as_ref()
                        .and_then(|s| s.source_file.as_deref())
                        .is_some_and(|f| vfs.exists(f));
                    with_skeleton += usize::from(skel_ok);
                    with_idle += usize::from(c.idle.is_some());
                    let items = c.props.equipment.as_deref().map_or(Vec::new(), |e| {
                        resolve_equipment(
                            e,
                            c.props.equipment_race.as_deref(),
                            &stats,
                            &templates,
                            &banks,
                        )
                    });
                    equipped += items.len();
                    if std::env::var_os("BG3_CHAR_DEBUG").is_some() {
                        for p in &c.parts {
                            for o in p.visual.objects.iter().filter(|o| o.lod == 0) {
                                let m = o
                                    .material_id
                                    .as_deref()
                                    .and_then(|m| banks.materials.get(m));
                                eprintln!(
                                    "    {:10} {:40} {:40} {:?}",
                                    p.slot,
                                    p.visual.name,
                                    m.map_or("-", |m| m.name.as_str()),
                                    m.and_then(|m| m.source_file.as_deref())
                                );
                            }
                        }
                    }
                    for i in &items {
                        eprintln!(
                            "    equipment {} ({}) → {}",
                            i.stats,
                            i.slot,
                            i.visual.as_ref().map_or("-", |v| v.name.as_str())
                        );
                    }
                    let missing: Vec<_> = c
                        .parts
                        .iter()
                        .filter(|p| {
                            !p.visual
                                .source_file
                                .as_deref()
                                .is_some_and(|f| vfs.exists(f))
                        })
                        .map(|p| p.slot.as_str())
                        .collect();
                    eprintln!(
                        "{:40} {:36} parts {:2} skel {} idle {:50} missing {:?}",
                        c.name,
                        c.character_visual.name,
                        c.parts.len(),
                        skel_ok,
                        c.idle.as_ref().map_or("-", |a| a.name.as_str()),
                        missing
                    );
                }
                Err(e) => failures.push((o.name.clone(), e)),
            }
        }
        eprintln!(
            "{} characters: {resolved} resolved, {with_skeleton} with skeleton GR2, {with_idle} with idle, {equipped} equipment visuals; failures {failures:?}",
            chars.len()
        );
        assert!(failures.is_empty());
        assert_eq!(with_skeleton, resolved);
    }

    #[test]
    fn blueprint_variable_default_by_name() {
        let var = |name: &str, value: &str| {
            Node::new("Variables")
                .with_attr("Name", Value::LSString(name.to_owned()))
                .with_child(
                    Node::new("Value")
                        .with_attr("Type", fs("AnimationSimpleName"))
                        .with_attr("Value", fs(value)),
                )
        };
        let res = bg3_lsf::Resource {
            metadata: Default::default(),
            regions: vec![bg3_lsf::Region {
                name: "Genome".into(),
                node: Node::new("Genome")
                    .with_child(var("Other", "x"))
                    .with_child(var("IDLE_DesiredStillShortName", "idle-guid")),
            }],
        };
        assert_eq!(
            blueprint_variable_default(&res, "IDLE_DesiredStillShortName").as_deref(),
            Some("idle-guid")
        );
        assert_eq!(blueprint_variable_default(&res, "Missing"), None);
    }
}
