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
}

impl CharacterProps {
    pub fn read(n: &Node) -> Self {
        Self {
            character_visual: str_attr(n, "CharacterVisualResourceID"),
            animation_set: str_attr(n, "AnimationSetResourceID"),
            equipment: str_attr(n, "Equipment"),
            stats: str_attr(n, "Stats"),
            equipment_race: str_attr(n, "EquipmentRace").filter(|s| !is_null_guid(s)),
            default_state: n.attr("DefaultState").and_then(|v| v.as_i64()),
        }
    }
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
    fn read(n: &Node) -> Self {
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

/// The animation-type ("simple name") GUID the character blueprint asks for when standing
/// still out of combat: the default of the `IDLE_DesiredStillShortName` variable in
/// `Public/Shared/Assets/Blueprints/Characters_Common.lsbs`. Read at runtime with
/// [`blueprint_variable_default`]; this constant is only the reference build's value, used as a
/// fallback and in tests.
pub const IDLE_STILL_PEACE: &str = "07e81afc-95c4-452b-ac68-5cbb74710cca";

/// The common character animation blueprint (compiled "Genome" resource).
pub const CHARACTER_BLUEPRINT: &str = "Public/Shared/Assets/Blueprints/Characters_Common.lsbs";

/// Blueprint variable holding the idle animation type.
pub const IDLE_BLUEPRINT_VARIABLE: &str = "IDLE_DesiredStillShortName";

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
    /// Animation sets in lookup order.
    pub animation_sets: Vec<String>,
    /// The idle animation, if found.
    pub idle: Option<AnimationResource>,
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
    CharacterProps {
        character_visual: pick(own.character_visual, |p| p.character_visual.as_ref()),
        animation_set: pick(own.animation_set, |p| p.animation_set.as_ref()),
        equipment: pick(own.equipment, |p| p.equipment.as_ref()),
        stats: pick(own.stats, |p| p.stats.as_ref()),
        equipment_race: pick(own.equipment_race, |p| p.equipment_race.as_ref()),
        default_state,
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
    let idle = animation_sets
        .iter()
        .filter_map(|s| banks.characters.animation_sets.get(s))
        .find_map(|set| set.find("", idle_simple_name))
        .and_then(|id| banks.characters.animations.get(id))
        .cloned();

    Ok(ResolvedCharacter {
        id: obj.id.clone(),
        name: obj.name.clone(),
        template: obj.template.clone(),
        props,
        character_visual: cv,
        skeleton,
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
        assert_eq!(idle, IDLE_STILL_PEACE);

        let chars: Vec<_> = level
            .objects
            .iter()
            .filter(|o| o.kind == "character")
            .collect();
        let mut resolved = 0;
        let mut with_skeleton = 0;
        let mut with_idle = 0;
        let mut failures = Vec::new();
        for o in &chars {
            match resolve_character(o, &templates, &banks, &idle) {
                Ok(c) => {
                    resolved += 1;
                    let skel_ok = c
                        .skeleton
                        .as_ref()
                        .and_then(|s| s.source_file.as_deref())
                        .is_some_and(|f| vfs.exists(f));
                    with_skeleton += usize::from(skel_ok);
                    with_idle += usize::from(c.idle.is_some());
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
            "{} characters: {resolved} resolved, {with_skeleton} with skeleton GR2, {with_idle} with idle; failures {failures:?}",
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
