//! Appearance options read from CC resources (4.1.1.7631656). IDs in a choice identify
//! CC entries, rather than their VisualBank/MaterialPresetBank resources; this preserves
//! distinct entries sharing a resource, including the explicit “none” visuals.
use std::collections::BTreeMap;

use bg3_vfs::Vfs;

use super::data::{CcData, each_module, handle, int, nodes, upsert};
use super::rules::CharacterChoice;
use crate::Guid;
use crate::load::{LoadError, guid_attr, s};

// bg3: Public/<Module>/Races/Races.lsx, Race/*Colors@Object (4.1.1.7631656).
const RACE_PALETTES: [(&str, &str); 10] = [
    ("Skin", "SkinColors"),
    ("Hair", "HairColors"),
    ("Eye", "EyeColors"),
    ("HairHighlight", "HairHighlightColors"),
    ("HairGraying", "HairGrayingColors"),
    ("Makeup", "MakeupColors"),
    ("LipsMakeup", "LipsMakeupColors"),
    ("Tattoo", "TattooColors"),
    ("HornColor", "HornColors"),
    ("HornTipColor", "HornTipColors"),
];

pub(super) fn read_race_palettes(
    n: &bg3_lsf::Node,
) -> std::collections::HashMap<String, Vec<Guid>> {
    RACE_PALETTES
        .into_iter()
        .map(|(kind, child)| (kind.to_owned(), super::data::objects(n, child)))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub head: Option<Guid>,
    pub hair: Option<Guid>,
    pub beard: Option<Guid>,
    pub horns: Option<Guid>,
    pub tail: Option<Guid>,
    pub piercing: Option<Guid>,
    pub private_parts: Option<Guid>,
    pub skin_color: Option<Guid>,
    pub hair_color: Option<Guid>,
    pub eye_color: Option<Guid>,
    pub other_visuals: BTreeMap<String, Guid>,
    pub materials: BTreeMap<String, Guid>,
}

impl Appearance {
    pub fn visual(&self, slot: &str) -> Option<Guid> {
        match slot {
            "Head" => self.head,
            "Hair" => self.hair,
            "Beard" => self.beard,
            "Horns" => self.horns,
            "Tail" => self.tail,
            "Piercing" => self.piercing,
            "Private Parts" => self.private_parts,
            _ => self.other_visuals.get(slot).copied(),
        }
    }

    pub fn set_visual(&mut self, slot: &str, id: Option<Guid>) {
        let field = match slot {
            "Head" => &mut self.head,
            "Hair" => &mut self.hair,
            "Beard" => &mut self.beard,
            "Horns" => &mut self.horns,
            "Tail" => &mut self.tail,
            "Piercing" => &mut self.piercing,
            "Private Parts" => &mut self.private_parts,
            _ => {
                if let Some(id) = id {
                    self.other_visuals.insert(slot.to_owned(), id);
                } else {
                    self.other_visuals.remove(slot);
                }
                return;
            }
        };
        *field = id;
    }

    pub fn material(&self, kind: &str) -> Option<Guid> {
        match kind {
            "Skin" => self.skin_color,
            "Hair" => self.hair_color,
            "Eye" => self.eye_color,
            _ => self.materials.get(kind).copied(),
        }
    }

    pub fn set_material(&mut self, kind: &str, id: Option<Guid>) {
        match kind {
            "Skin" => self.skin_color = id,
            "Hair" => self.hair_color = id,
            "Eye" => self.eye_color = id,
            _ => {
                if let Some(id) = id {
                    self.materials.insert(kind.to_owned(), id);
                } else {
                    self.materials.remove(kind);
                }
            }
        }
    }

    pub fn visuals(&self) -> Vec<(String, Guid)> {
        let mut out: Vec<_> = [
            "Head",
            "Hair",
            "Beard",
            "Horns",
            "Tail",
            "Piercing",
            "Private Parts",
        ]
        .into_iter()
        .filter_map(|slot| self.visual(slot).map(|id| (slot.to_owned(), id)))
        .collect();
        out.extend(self.other_visuals.iter().map(|(k, v)| (k.clone(), *v)));
        out
    }

    pub fn material_choices(&self) -> Vec<(String, Guid)> {
        let mut out: Vec<_> = ["Skin", "Hair", "Eye"]
            .into_iter()
            .filter_map(|kind| self.material(kind).map(|id| (kind.to_owned(), id)))
            .collect();
        out.extend(self.materials.iter().map(|(k, v)| (k.clone(), *v)));
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppearanceVisual {
    pub id: Guid,
    pub slot: String,
    pub display: Option<String>,
    pub visual: Guid,
    pub bone: Option<String>,
    pub race: Option<Guid>,
    pub subrace: Option<Guid>,
    pub body_type: Option<u8>,
    pub body_shape: Option<u8>,
    pub root_template: Option<Guid>,
    pub head: Option<Guid>,
    pub default_body_type: Option<u8>,
    pub default_skin: Option<Guid>,
    pub shared: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppearanceMaterial {
    pub id: Guid,
    pub kind: String,
    pub name: String,
    pub display: Option<String>,
    pub preset: Guid,
    pub ui_color: Option<String>,
    pub skin_type: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcMaterialSwap {
    pub id: Guid,
    pub name: String,
    pub kind: String,
    pub source: Guid,
    pub target: Guid,
    pub active: Option<Guid>,
    pub inactive: Option<Guid>,
}

#[derive(Clone, Debug, Default)]
pub struct AppearanceData {
    pub visuals: Vec<AppearanceVisual>,
    pub materials: Vec<AppearanceMaterial>,
    pub material_swaps: Vec<CcMaterialSwap>,
}

/// Bevy-free, resolved resources sent from the sim to the viewer.
#[derive(Clone, Debug, Default)]
pub struct ResolvedAppearance {
    /// Slot, VisualBank UUID (null removes the slot), optional rigid attachment bone.
    pub visuals: Vec<(String, Guid, Option<String>)>,
    /// Category, MaterialPresetBank UUID, in application order.
    pub presets: Vec<(String, Guid)>,
}

impl AppearanceData {
    pub fn load(vfs: &Vfs) -> Result<Self, LoadError> {
        let mut out = Self::default();
        for (file, shared) in [
            ("CharacterCreationAppearanceVisuals", false),
            ("CharacterCreationSharedVisuals", true),
        ] {
            each_module(vfs, &format!("CharacterCreation/{file}.lsx"), |res| {
                for n in nodes(res, file, &file[..file.len() - 1]) {
                    let Some(v) = AppearanceVisual::read(n, shared) else {
                        continue;
                    };
                    upsert(&mut out.visuals, v, |v| v.id);
                }
            })?;
        }
        for (file, category) in [
            ("CharacterCreationSkinColors", "Skin"),
            ("CharacterCreationHairColors", "Hair"),
            ("CharacterCreationEyeColors", "Eye"),
            ("CharacterCreationAppearanceMaterials", ""),
        ] {
            let dir = if category.is_empty() {
                "CharacterCreation"
            } else {
                "CharacterCreationPresets"
            };
            each_module(vfs, &format!("{dir}/{file}.lsx"), |res| {
                for n in nodes(res, file, &file[..file.len() - 1]) {
                    let (Some(id), Some(preset)) =
                        (guid_attr(n, "UUID"), guid_attr(n, "MaterialPresetUUID"))
                    else {
                        continue;
                    };
                    let m = AppearanceMaterial {
                        id,
                        preset,
                        kind: if category.is_empty() {
                            s(n, "MaterialType").unwrap_or_default()
                        } else {
                            category.to_owned()
                        },
                        name: s(n, "Name").unwrap_or_default(),
                        display: handle(n, "DisplayName"),
                        ui_color: s(n, "UIColor"),
                        skin_type: s(n, "SkinType"),
                    };
                    upsert(&mut out.materials, m, |m| m.id);
                }
            })?;
        }
        each_module(
            vfs,
            "CharacterCreationPresets/CharacterCreationMaterialOverrides.lsx",
            |res| {
                for n in nodes(
                    res,
                    "CharacterCreationMaterialOverrides",
                    "CharacterCreationMaterialOverride",
                ) {
                    let (Some(id), Some(source), Some(target)) = (
                        guid_attr(n, "UUID"),
                        guid_attr(n, "SourceMaterialUUID"),
                        guid_attr(n, "TargetMaterialUUID"),
                    ) else {
                        continue;
                    };
                    upsert(
                        &mut out.material_swaps,
                        CcMaterialSwap {
                            id,
                            source,
                            target,
                            name: s(n, "Name").unwrap_or_default(),
                            kind: s(n, "MaterialType").unwrap_or_default(),
                            active: guid_attr(n, "ActiveMaterialPresetUUID"),
                            inactive: guid_attr(n, "InactiveMaterialPresetUUID"),
                        },
                        |s| s.id,
                    );
                }
            },
        )?;
        Ok(out)
    }

    pub fn material_options(&self, kind: &str) -> Vec<&AppearanceMaterial> {
        self.materials.iter().filter(|m| m.kind == kind).collect()
    }

    pub fn material_kinds(&self) -> Vec<String> {
        let mut kinds: Vec<_> = self.materials.iter().map(|m| m.kind.clone()).collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }
}

impl CcData {
    /// Skin/Hair/Eye race lists reference CC entry UUIDs, not MaterialPresetBank UUIDs.
    /// Assumption: the nearest non-empty list in ParentGuid ancestry replaces ancestors;
    /// empty lists inherit (e.g. rock gnome colours).
    // bg3: Races/Races.lsx, ParentGuid and *Colors/Object (4.1.1.7631656).
    pub fn material_options(&self, c: &CharacterChoice, kind: &str) -> Vec<&AppearanceMaterial> {
        // Other *Colors lists reference Lists/ColorDefinitions.lsx, not appearance
        // material entries. Those selectors choose patterns/default presets, not colours.
        if !matches!(kind, "Skin" | "Hair" | "Eye") {
            return self.appearance.material_options(kind);
        }
        let mut next = c.subrace.or(Some(c.race));
        let mut seen = Vec::new();
        while let Some(id) = next {
            if seen.contains(&id) {
                break;
            }
            seen.push(id);
            let Some(race) = self.race(id) else {
                break;
            };
            if let Some(ids) = race.palettes.get(kind).filter(|ids| !ids.is_empty()) {
                return self
                    .appearance
                    .material_options(kind)
                    .into_iter()
                    .filter(|m| ids.contains(&m.id))
                    .collect();
            }
            next = race.parent;
        }
        Vec::new()
    }

    /// bg3: CharacterCreationAppearanceVisuals/{RaceUUID,BodyType,BodyShape,RootTemplate},
    /// CharacterCreationSharedVisuals (4.1.1.7631656). Absent filters are unrestricted.
    pub fn appearance_options(&self, c: &CharacterChoice, slot: &str) -> Vec<&AppearanceVisual> {
        let template = self
            .preset(c.race, c.subrace, c.body_type, c.body_shape)
            .map(|p| p.root_template);
        let mut allowed = Vec::new();
        let mut seen = Vec::new();
        let mut next = c.subrace.or(Some(c.race));
        while let Some(id) = next {
            if seen.contains(&id) {
                break;
            }
            seen.push(id);
            let Some(race) = self.race(id) else {
                break;
            };
            allowed.extend(race.visuals.iter().copied());
            next = race.parent;
        }
        // Race Visuals lists constrain shared Hair/Beard choices. Unlisted categories
        // (e.g. Piercing) have no race lists and keep their explicit resource filters.
        let listed_slot = self.races.iter().flat_map(|r| &r.visuals).any(|id| {
            self.appearance
                .visuals
                .iter()
                .any(|v| v.id == *id && v.shared && v.slot == slot)
        });
        self.appearance
            .visuals
            .iter()
            .filter(|v| {
                v.slot == slot
                    && (!v.shared || !listed_slot || allowed.contains(&v.id))
                    && v.race.is_none_or(|r| r == c.race || Some(r) == c.subrace)
                    && v.subrace.is_none_or(|r| Some(r) == c.subrace)
                    && v.body_type.is_none_or(|b| b == c.body_type)
                    && v.body_shape.is_none_or(|b| b == c.body_shape)
                    && v.root_template.is_none_or(|t| Some(t) == template)
                    && v.head.is_none_or(|h| Some(h) == c.appearance.head)
            })
            .collect()
    }

    pub fn appearance_slots(&self, c: &CharacterChoice) -> Vec<String> {
        let mut slots: Vec<_> = self
            .appearance
            .visuals
            .iter()
            .map(|v| v.slot.clone())
            .collect();
        slots.sort();
        slots.dedup();
        slots.retain(|s| !self.appearance_options(c, s).is_empty());
        slots
    }

    /// Fill explicit body defaults and select an allowed colour when unspecified.
    /// Assumption: the first allowed colour in merged CC table order is our default.
    /// Assumption: DefaultForBodyType selects the default entry for the matching body type.
    pub fn complete_appearance(&self, c: &mut CharacterChoice) {
        for kind in ["Skin", "Hair", "Eye"] {
            if c.appearance.material(kind).is_none()
                && let Some(m) = self.material_options(c, kind).first()
            {
                c.appearance.set_material(kind, Some(m.id));
            }
        }
        for slot in self.appearance_slots(c) {
            if c.appearance.visual(&slot).is_none()
                && let Some(v) = self
                    .appearance_options(c, &slot)
                    .into_iter()
                    .find(|v| v.default_body_type == Some(c.body_type))
            {
                c.appearance.set_visual(&slot, Some(v.id));
            }
        }
    }

    pub fn resolve_appearance(&self, a: &Appearance) -> ResolvedAppearance {
        ResolvedAppearance {
            visuals: a
                .visuals()
                .into_iter()
                .filter_map(|(slot, id)| {
                    self.appearance
                        .visuals
                        .iter()
                        .find(|v| v.id == id)
                        .map(|v| (slot, v.visual, v.bone.clone()))
                })
                .collect(),
            // Assumption: selected presets follow template presets; colour lists precede
            // the other material choices in stable category order. Own parameters stay last.
            presets: a
                .material_choices()
                .into_iter()
                .filter_map(|(kind, id)| {
                    self.appearance
                        .materials
                        .iter()
                        .find(|m| m.id == id && m.kind == kind)
                        .map(|m| (kind, m.preset))
                })
                .collect(),
        }
    }
}

impl AppearanceVisual {
    fn read(n: &bg3_lsf::Node, shared: bool) -> Option<Self> {
        let id = guid_attr(n, "UUID")?;
        let visual = guid_attr(n, "VisualResource")?;
        let nonnull = |key| guid_attr(n, key).filter(|id| !id.is_null());
        Some(Self {
            id,
            visual,
            shared,
            slot: s(n, "SlotName").unwrap_or_default(),
            display: handle(n, "DisplayName"),
            bone: s(n, "BoneName"),
            race: nonnull("RaceUUID"),
            subrace: nonnull("SubRaceUUID"),
            body_type: int(n, "BodyType").map(|v| v as u8),
            body_shape: int(n, "BodyShape").map(|v| v as u8),
            root_template: nonnull("RootTemplate"),
            head: nonnull("HeadAppearanceUUID"),
            default_body_type: int(n, "DefaultForBodyType").map(|v| v as u8),
            default_skin: nonnull("DefaultSkinColor"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_lsf::{Node, Value};

    #[test]
    fn visual_parsing_and_module_replacement_preserve_absent_filters() {
        let node = Node::new("CharacterCreationAppearanceVisual")
            .with_attr("UUID", Value::FixedString(Guid(10).to_string()))
            .with_attr("VisualResource", Value::FixedString(Guid(11).to_string()))
            .with_attr("SlotName", Value::FixedString("Hair".into()))
            .with_attr("BodyType", Value::Byte(1));
        let specific = AppearanceVisual::read(&node, false).unwrap();
        assert_eq!(specific.body_type, Some(1));
        assert_eq!(specific.race, None);
        let shared = Node::new("CharacterCreationSharedVisual")
            .with_attr("UUID", Value::FixedString(Guid(10).to_string()))
            .with_attr("VisualResource", Value::FixedString(Guid::NULL.to_string()))
            .with_attr("SlotName", Value::FixedString("Hair".into()));
        let mut rows = vec![specific];
        upsert(
            &mut rows,
            AppearanceVisual::read(&shared, true).unwrap(),
            |v| v.id,
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].visual.is_null());
        assert_eq!(rows[0].body_type, None);
    }

    #[test]
    fn options_filter_body_race_template_head_and_validate_slot_and_material() {
        let mut d = CcData::default();
        let mut c = CharacterChoice {
            race: Guid(1),
            subrace: Some(Guid(2)),
            ..Default::default()
        };
        let hair = AppearanceVisual {
            id: Guid(10),
            slot: "Hair".into(),
            visual: Guid(20),
            race: Some(Guid(1)),
            body_type: Some(0),
            body_shape: Some(0),
            ..Default::default()
        };
        let shared = AppearanceVisual {
            id: Guid(11),
            slot: "Hair".into(),
            shared: true,
            ..Default::default()
        };
        d.appearance.visuals = vec![hair, shared];
        assert_eq!(d.appearance_options(&c, "Hair").len(), 2);
        c.body_type = 1;
        assert_eq!(d.appearance_options(&c, "Hair").len(), 1);
        c.body_type = 0;
        c.race = Guid(3);
        assert_eq!(d.appearance_options(&c, "Hair").len(), 1);
        c.race = Guid(1);
        c.appearance.hair = Some(Guid(10));
        assert!(
            !d.validate(&c)
                .iter()
                .any(|e| matches!(e, super::super::CcError::NotAnOption { .. }))
        );
        c.appearance.head = Some(Guid(10)); // valid entry, wrong slot
        c.appearance.skin_color = Some(Guid(99));
        assert_eq!(
            d.validate(&c)
                .iter()
                .filter(|e| matches!(e, super::super::CcError::NotAnOption { .. }))
                .count(),
            2
        );
        d.appearance.visuals[0].root_template = Some(Guid(30));
        assert_eq!(d.appearance_options(&c, "Hair").len(), 1);
        d.appearance.visuals[0].root_template = None;
        d.appearance.visuals[0].head = Some(Guid(40));
        assert_eq!(d.appearance_options(&c, "Hair").len(), 1);
        c.appearance.head = Some(Guid(40));
        assert_eq!(d.appearance_options(&c, "Hair").len(), 2);
    }

    #[test]
    fn defaults_are_explicit_and_resolution_preserves_null_and_bone() {
        let mut d = CcData::default();
        d.appearance.visuals.push(AppearanceVisual {
            id: Guid(10),
            slot: "Hair".into(),
            bone: Some("head".into()),
            ..Default::default()
        });
        let mut c = CharacterChoice::default();
        d.complete_appearance(&mut c);
        assert!(c.appearance.hair.is_none());
        d.appearance.visuals[0].default_body_type = Some(0);
        d.complete_appearance(&mut c);
        assert_eq!(c.appearance.hair, Some(Guid(10)));
        assert_eq!(
            d.resolve_appearance(&c.appearance).visuals,
            vec![("Hair".into(), Guid::NULL, Some("head".into()))]
        );
    }
}

#[cfg(test)]
mod race_visual_tests {
    use super::super::CcRace;
    use super::*;

    #[test]
    fn shared_visuals_use_race_lists_inherited_by_subrace() {
        let mut d = CcData {
            races: vec![
                CcRace {
                    id: Guid(1),
                    visuals: vec![Guid(10)],
                    ..Default::default()
                },
                CcRace {
                    id: Guid(2),
                    parent: Some(Guid(1)),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        d.appearance.visuals = vec![
            AppearanceVisual {
                id: Guid(10),
                slot: "Hair".into(),
                shared: true,
                ..Default::default()
            },
            AppearanceVisual {
                id: Guid(11),
                slot: "Hair".into(),
                shared: true,
                ..Default::default()
            },
            AppearanceVisual {
                id: Guid(12),
                slot: "Piercing".into(),
                shared: true,
                ..Default::default()
            },
        ];
        let c = CharacterChoice {
            race: Guid(1),
            subrace: Some(Guid(2)),
            ..Default::default()
        };
        assert_eq!(
            d.appearance_options(&c, "Hair")
                .iter()
                .map(|v| v.id)
                .collect::<Vec<_>>(),
            [Guid(10)]
        );
        assert_eq!(d.appearance_options(&c, "Piercing").len(), 1);
        let mut wrong = c;
        wrong.appearance.hair = Some(Guid(11));
        assert!(
            d.validate(&wrong)
                .iter()
                .any(|e| matches!(e, super::super::CcError::NotAnOption { .. }))
        );
    }
}

#[cfg(test)]
mod palette_tests {
    use super::super::CcRace;
    use super::*;
    use bg3_lsf::{Node, Value};

    #[test]
    fn palette_object_attributes_are_cc_entry_ids() {
        let mut race = Node::new("Race");
        for (_, child) in RACE_PALETTES {
            race.children.push(
                Node::new(child).with_attr("Object", Value::FixedString(Guid(10).to_string())),
            );
        }
        let palettes = read_race_palettes(&race);
        for (kind, _) in RACE_PALETTES {
            assert_eq!(palettes[kind], vec![Guid(10)]);
        }
    }

    #[test]
    fn subrace_overrides_nonempty_palettes_and_inherits_empty_categories() {
        let mut d = CcData {
            races: vec![
                CcRace {
                    id: Guid(1),
                    palettes: [
                        ("Skin".into(), vec![Guid(10)]),
                        ("Hair".into(), vec![Guid(20)]),
                    ]
                    .into(),
                    ..Default::default()
                },
                CcRace {
                    id: Guid(2),
                    parent: Some(Guid(1)),
                    palettes: [("Skin".into(), vec![Guid(11)]), ("Hair".into(), vec![])].into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        d.appearance.materials = [(10, "Skin"), (11, "Skin"), (20, "Hair"), (30, "Scar")]
            .into_iter()
            .map(|(id, kind)| AppearanceMaterial {
                id: Guid(id),
                preset: Guid(id + 100),
                kind: kind.into(),
                ..Default::default()
            })
            .collect();
        let mut c = CharacterChoice {
            race: Guid(1),
            ..Default::default()
        };
        assert_eq!(d.material_options(&c, "Skin")[0].id, Guid(10));
        c.subrace = Some(Guid(2));
        assert_eq!(
            d.material_options(&c, "Skin")
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            vec![Guid(11)]
        );
        assert_eq!(d.material_options(&c, "Hair")[0].id, Guid(20));
        assert_eq!(d.material_options(&c, "Scar")[0].id, Guid(30));
        assert!(d.material_options(&c, "HornColor").is_empty());
        d.complete_appearance(&mut c);
        assert_eq!(c.appearance.skin_color, Some(Guid(11)));
        assert_eq!(c.appearance.hair_color, Some(Guid(20)));
        let rejected = |c: &CharacterChoice| {
            d.validate(c).iter().any(|e| {
                matches!(e,
            super::super::CcError::NotAnOption { slot, .. } if slot == "appearance Skin material")
            })
        };
        assert!(!rejected(&c));
        c.appearance.skin_color = Some(Guid(10));
        assert!(rejected(&c));
        c.appearance.skin_color = Some(Guid(111)); // preset UUID is not the CC entry UUID
        assert!(rejected(&c));
        d.races[1].palettes.get_mut("Skin").unwrap().clear();
        assert_eq!(d.material_options(&c, "Skin")[0].id, Guid(10));
        d.races[0].parent = Some(Guid(2)); // malformed ancestry terminates
        assert!(d.material_options(&c, "Eye").is_empty());
    }
}
