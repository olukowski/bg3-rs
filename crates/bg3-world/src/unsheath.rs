//! Drawn and sheathed weapons: where an equipped weapon hangs and which animation subset the
//! character uses, both read from the install.
//!
//! * `Public/<Module>/EquipmentTypes/EquipmentTypes.lsx`: per equipment type (the item root
//!   template's `EquipmentTypeID`) the character bones a weapon attaches to when sheathed and
//!   unsheathed, the weapon-model bones that go on them, and the weapon type names used for
//!   animation (`WeaponType_OneHanded` / `WeaponType_TwoHanded`).
//! * `Public/<Module>/WeaponAnimationSetData/WeaponAnimationSetData.lsx`: a (main hand, off
//!   hand) weapon-type pair → the animation subset GUID (`AnimationSetUUID`), the key of an
//!   `AnimationSubSets` entry in the character's animation sets.
//!
//! The selection rules are ported from the engine; see `docs/research/characters.md`
//! ("Drawn and sheathed weapons") for what is verified and what is assumed.

use std::collections::HashMap;

use crate::str_attr;

/// Weapon-model bone placed on the character's hand bone when nothing else is named.
pub const DUMMY_ATTACHMENT: &str = "Dummy_Attachment";
/// Weapon-model bone placed on a sheath bone when the equipment type names none.
pub const DUMMY_SHEATH: &str = "Dummy_Sheath";
/// Weapon type of an empty hand (and the default of a missing weapon-type attribute).
pub const UNARMED: &str = "Unarmed1H";

/// The character's weapon state.
// bg3: eoc::unsheath::EState (4.1.1.7631656): Instrument, Melee, Ranged, Sheathed, in this
// order (the `ls.anubis.game.UnsheathState` names in the binary).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnsheathState {
    Instrument,
    Melee,
    Ranged,
    #[default]
    Sheathed,
}

impl UnsheathState {
    pub const ALL: [Self; 4] = [Self::Instrument, Self::Melee, Self::Ranged, Self::Sheathed];

    /// Whether the weapon in `slot` is in the hand in this state. Assumption: the per-slot
    /// "unsheathed" flag the client passes to `ecl::equipment::GetAttachmentBoneName` follows
    /// the slots `eoc::unsheath::ResolveSubSetFromState` reads for the state.
    pub fn draws(self, slot: WeaponSlot) -> bool {
        match self {
            Self::Melee => matches!(slot, WeaponSlot::MeleeMain | WeaponSlot::MeleeOff),
            Self::Ranged => matches!(slot, WeaponSlot::RangedMain | WeaponSlot::RangedOff),
            Self::Instrument => slot == WeaponSlot::Instrument,
            Self::Sheathed => false,
        }
    }
}

/// The state a spell's `Sheathing` field (`SpellSheathing` value list) asks for.
// bg3: eoc::unsheath::ResolveStateFromSpell (4.1.1.7631656): Instrument, Melee and Ranged
// map to the states of the same name, `WeaponSet` to the active weapon set (ranged set →
// Ranged, otherwise Melee), `DontChange` keeps the current state, and anything else
// (`Sheathed`, an unknown value) is Sheathed. Assumption: `Somatic` keeps the current state
// (the engine resolves it through further data that is not traced).
pub fn state_from_sheathing(
    sheathing: &str,
    current: UnsheathState,
    ranged_set: bool,
) -> UnsheathState {
    match sheathing.trim() {
        "Instrument" => UnsheathState::Instrument,
        "Melee" => UnsheathState::Melee,
        "Ranged" => UnsheathState::Ranged,
        "WeaponSet" => weapon_set_state(ranged_set),
        "DontChange" | "Somatic" | "" => current,
        _ => UnsheathState::Sheathed,
    }
}

/// The state of the active weapon set (`eoc::WeaponSetComponent`): ranged → Ranged, else Melee.
pub fn weapon_set_state(ranged_set: bool) -> UnsheathState {
    if ranged_set {
        UnsheathState::Ranged
    } else {
        UnsheathState::Melee
    }
}

/// The equipment slots a weapon visual can occupy (`EEquipmentSlot` 3–6 and the instrument).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WeaponSlot {
    MeleeMain,
    MeleeOff,
    RangedMain,
    RangedOff,
    Instrument,
}

impl WeaponSlot {
    /// From the stats `Slot` name.
    pub fn from_stats(slot: &str) -> Option<Self> {
        Some(match slot {
            "Melee Main Weapon" => Self::MeleeMain,
            "Melee Offhand Weapon" => Self::MeleeOff,
            "Ranged Main Weapon" => Self::RangedMain,
            "Ranged Offhand Weapon" => Self::RangedOff,
            "MusicalInstrument" => Self::Instrument,
            _ => return None,
        })
    }

    pub fn is_off_hand(self) -> bool {
        matches!(self, Self::MeleeOff | Self::RangedOff)
    }

    /// The other hand of the same weapon set.
    pub fn opposite(self) -> Option<Self> {
        match self {
            Self::MeleeMain => Some(Self::MeleeOff),
            Self::MeleeOff => Some(Self::MeleeMain),
            Self::RangedMain => Some(Self::RangedOff),
            Self::RangedOff => Some(Self::RangedMain),
            Self::Instrument => None,
        }
    }
}

/// One `EquipmentType` entry. Unset fields are `None` (the engine's `NONE`), except the two
/// with engine defaults (`BoneVersatileUnsheathed`, the unsheathed source bones).
// bg3: eoc::EquipmentTypeData::Visit (4.1.1.7631656)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EquipmentType {
    pub id: String,
    pub name: String,
    pub weapon_type_one_handed: Option<String>,
    pub weapon_type_two_handed: Option<String>,
    pub bone_main_sheathed: Option<String>,
    pub bone_main_unsheathed: Option<String>,
    pub bone_off_hand_sheathed: Option<String>,
    pub bone_off_hand_unsheathed: Option<String>,
    pub bone_versatile_sheathed: Option<String>,
    /// Default `Dummy_R_Hand`.
    pub bone_versatile_unsheathed: String,
    pub source_bone_sheathed: Option<String>,
    pub source_bone_versatile_sheathed: Option<String>,
    /// Default `Dummy_Attachment`.
    pub source_bone_versatile_unsheathed: String,
    /// Default `Dummy_Attachment` (`Dummy_Attachment_Rage` on two-handed weapons).
    pub source_bone_alternative_unsheathed: String,
}

impl EquipmentType {
    fn read(n: &bg3_lsf::Node) -> Option<Self> {
        let s = |k: &str| str_attr(n, k);
        Some(Self {
            id: s("UUID")?,
            name: s("Name").unwrap_or_default(),
            weapon_type_one_handed: s("WeaponType_OneHanded"),
            weapon_type_two_handed: s("WeaponType_TwoHanded"),
            bone_main_sheathed: s("BoneMainSheathed"),
            bone_main_unsheathed: s("BoneMainUnsheathed"),
            bone_off_hand_sheathed: s("BoneOffHandSheathed"),
            bone_off_hand_unsheathed: s("BoneOffHandUnsheathed"),
            bone_versatile_sheathed: s("BoneVersatileSheathed"),
            bone_versatile_unsheathed: s("BoneVersatileUnsheathed")
                .unwrap_or_else(|| "Dummy_R_Hand".to_owned()),
            source_bone_sheathed: s("SourceBoneSheathed"),
            source_bone_versatile_sheathed: s("SourceBoneVersatileSheathed"),
            source_bone_versatile_unsheathed: s("SourceBoneVersatileUnsheathed")
                .unwrap_or_else(|| DUMMY_ATTACHMENT.to_owned()),
            source_bone_alternative_unsheathed: s("SourceBoneAlternativeUnsheathed")
                .unwrap_or_else(|| DUMMY_ATTACHMENT.to_owned()),
        })
    }
}

/// How a weapon is held, the boolean inputs of the bone selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grip {
    pub unsheathed: bool,
    pub off_hand: bool,
    /// A `Versatile` weapon with the other hand empty (wielded two-handed).
    pub versatile: bool,
    /// The alternative grip (`SourceBoneAlternativeUnsheathed`, e.g. while raging); not
    /// modelled by the sim, always false there.
    pub alternative: bool,
}

/// Where a weapon visual goes: the weapon model's `source` bone is placed on the character's
/// `bone`. `bone == None` (the type names no bone, e.g. a sheathed shield) means the visual is
/// not attached, and so not drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub bone: Option<String>,
    pub source: String,
}

/// Pick the character bone and weapon-model bone for a weapon.
// bg3: ecl::equipment::GetAttachmentBoneName (4.1.1.7631656), the weapon-slot branch.
pub fn attachment(ty: Option<&EquipmentType>, grip: Grip) -> Attachment {
    let Some(t) = ty else {
        // No equipment type: the right hand when drawn, else a generic weapon dummy.
        return if grip.unsheathed {
            Attachment {
                bone: Some("Dummy_R_Hand".to_owned()),
                source: DUMMY_ATTACHMENT.to_owned(),
            }
        } else {
            Attachment {
                bone: Some("Dummy_Weapon_R_1H".to_owned()),
                source: DUMMY_SHEATH.to_owned(),
            }
        };
    };
    let pick = |b: &Option<String>| b.clone();
    let (bone, source) = if grip.unsheathed && grip.alternative {
        let bone = if grip.off_hand {
            pick(&t.bone_off_hand_unsheathed)
        } else {
            Some(t.bone_versatile_unsheathed.clone())
        };
        (bone, Some(t.source_bone_alternative_unsheathed.clone()))
    } else if grip.versatile {
        let bone = match (grip.off_hand, grip.unsheathed) {
            (true, true) => pick(&t.bone_off_hand_unsheathed),
            (true, false) => pick(&t.bone_off_hand_sheathed),
            (false, true) => Some(t.bone_versatile_unsheathed.clone()),
            (false, false) => pick(&t.bone_versatile_sheathed),
        };
        let source = if grip.unsheathed {
            Some(t.source_bone_versatile_unsheathed.clone())
        } else {
            t.source_bone_versatile_sheathed.clone()
        };
        (bone, source)
    } else {
        let bone = match (grip.off_hand, grip.unsheathed) {
            (true, true) => pick(&t.bone_off_hand_unsheathed),
            (true, false) => pick(&t.bone_off_hand_sheathed),
            (false, true) => pick(&t.bone_main_unsheathed),
            (false, false) => pick(&t.bone_main_sheathed),
        };
        // Drawn: the default `Dummy_Attachment` stays.
        let source = if grip.unsheathed {
            None
        } else {
            t.source_bone_sheathed.clone()
        };
        (bone, source)
    };
    Attachment {
        bone,
        source: source.unwrap_or_else(|| DUMMY_ATTACHMENT.to_owned()),
    }
}

/// A weapon in one hand, for the weapon-type pair.
#[derive(Clone, Copy, Debug)]
pub struct HeldWeapon<'a> {
    pub ty: Option<&'a EquipmentType>,
    /// The weapon has the `Twohanded` or `Versatile` property.
    pub two_handed: bool,
}

/// The (main hand, off hand) weapon types of a weapon set.
// bg3: eoc::ResolveWeaponData (4.1.1.7631656): both default to `Unarmed1H`. With both hands
// filled each uses its one-handed type; a single weapon uses its two-handed type when it is
// `Twohanded` or `Versatile`. A weapon without an equipment type leaves its hand unarmed.
pub fn weapon_types(main: Option<HeldWeapon>, off: Option<HeldWeapon>) -> (String, String) {
    let one = |w: &HeldWeapon| w.ty.map(|t| t.weapon_type_one_handed.clone());
    let single = |w: &HeldWeapon| {
        w.ty.map(|t| {
            if w.two_handed {
                t.weapon_type_two_handed.clone()
            } else {
                t.weapon_type_one_handed.clone()
            }
        })
    };
    let (main, off) = (
        main.filter(|w| w.ty.is_some()),
        off.filter(|w| w.ty.is_some()),
    );
    let (m, o) = match (main, off) {
        (Some(m), Some(o)) => (one(&m), one(&o)),
        (Some(m), None) => (single(&m), None),
        (None, Some(o)) => (None, single(&o)),
        (None, None) => (None, None),
    };
    // A type with no weapon type (`NONE`) keeps that name, so it matches no animation set.
    let name = |t: Option<Option<String>>| match t {
        None => UNARMED.to_owned(),
        Some(t) => t.unwrap_or_else(|| "NONE".to_owned()),
    };
    (name(m), name(o))
}

/// Install-owned weapon data: equipment types and weapon animation sets.
#[derive(Clone, Debug, Default)]
pub struct WeaponData {
    /// By `UUID`.
    pub equipment_types: HashMap<String, EquipmentType>,
    /// (main hand, off hand) weapon type → animation subset GUID.
    pub animation_sets: HashMap<(String, String), String>,
}

impl WeaponData {
    /// Read both files of every module in load order (later modules override by key).
    pub fn load(vfs: &bg3_vfs::Vfs, modules: &[&str]) -> crate::Result<Self> {
        let mut out = Self::default();
        for module in modules {
            for path in [
                format!("Public/{module}/EquipmentTypes/EquipmentTypes.lsx"),
                format!("Public/{module}/WeaponAnimationSetData/WeaponAnimationSetData.lsx"),
            ] {
                if vfs.exists(&path) {
                    out.read(&crate::parse(vfs, &path)?);
                }
            }
        }
        Ok(out)
    }

    pub fn read(&mut self, res: &bg3_lsf::Resource) {
        if let Some(root) = res.region("EquipmentTypes") {
            for t in root
                .children_named("EquipmentType")
                .filter_map(EquipmentType::read)
            {
                self.equipment_types.insert(t.id.clone(), t);
            }
        }
        if let Some(root) = res.region("WeaponAnimationSetData") {
            for n in root.children_named("WeaponAnimationSet") {
                // bg3: eoc::WeaponAnimationSet::Visit (4.1.1.7631656): hands default to Unarmed1H.
                let hand = |k| str_attr(n, k).unwrap_or_else(|| UNARMED.to_owned());
                if let Some(set) = str_attr(n, "AnimationSetUUID") {
                    self.animation_sets
                        .insert((hand("MainHand"), hand("OffHand")), set);
                }
            }
        }
    }

    pub fn equipment_type(&self, id: Option<&str>) -> Option<&EquipmentType> {
        self.equipment_types.get(id?)
    }

    /// The animation subset of a weapon-type pair; `""` (the default subset) when the pair is
    /// not listed.
    // bg3: eoc::WeaponAnimationSetData::GetAnimationSet (4.1.1.7631656): an exact lookup of
    // the pair, empty names reading as Unarmed1H. Assumption: the static default it returns
    // for a missing pair selects no subset.
    pub fn animation_subset(&self, main: &str, off: &str) -> &str {
        let norm = |s: &str| {
            if s.is_empty() {
                UNARMED.to_owned()
            } else {
                s.to_owned()
            }
        };
        self.animation_sets
            .get(&(norm(main), norm(off)))
            .map_or("", String::as_str)
    }
}

/// A character's equipped weapon visual, as far as the selection needs it.
#[derive(Clone, Copy, Debug)]
pub struct WeaponItem<'a> {
    pub slot: WeaponSlot,
    pub equipment_type: Option<&'a str>,
    pub two_handed: bool,
    pub versatile: bool,
}

/// The animation subset for a state and the equipped weapons.
// bg3: eoc::unsheath::ResolveSubSetFromState (4.1.1.7631656): Melee reads the melee slots,
// Ranged the ranged slots, Instrument the instrument (`Voice` without one); Sheathed uses the
// empty (default) subset.
pub fn subset_for_state<'a>(
    data: &'a WeaponData,
    state: UnsheathState,
    items: &[WeaponItem],
) -> &'a str {
    let held = |slot: WeaponSlot| {
        items.iter().find(|i| i.slot == slot).map(|i| HeldWeapon {
            ty: data.equipment_type(i.equipment_type),
            two_handed: i.two_handed || i.versatile,
        })
    };
    let (main, off) = match state {
        UnsheathState::Sheathed => return "",
        UnsheathState::Melee => {
            weapon_types(held(WeaponSlot::MeleeMain), held(WeaponSlot::MeleeOff))
        }
        UnsheathState::Ranged => {
            weapon_types(held(WeaponSlot::RangedMain), held(WeaponSlot::RangedOff))
        }
        UnsheathState::Instrument => {
            let ty = held(WeaponSlot::Instrument)
                .and_then(|w| w.ty)
                .and_then(|t| t.weapon_type_one_handed.clone())
                .unwrap_or_else(|| "Voice".to_owned());
            (ty, UNARMED.to_owned())
        }
    };
    data.animation_subset(&main, &off)
}

/// The sheathed and drawn attachments of one weapon visual.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponPlacement {
    pub slot: WeaponSlot,
    pub sheathed: Attachment,
    pub unsheathed: Attachment,
}

impl WeaponPlacement {
    pub fn for_state(&self, state: UnsheathState) -> &Attachment {
        if state.draws(self.slot) {
            &self.unsheathed
        } else {
            &self.sheathed
        }
    }
}

/// Placements of `item` among the character's `items`. Assumption: the client's versatile
/// flag means "a `Versatile` weapon whose other hand (same weapon set) is empty" (the engine
/// tests the opposite slot against a set of slots that is not traced).
pub fn placement(data: &WeaponData, item: &WeaponItem, items: &[WeaponItem]) -> WeaponPlacement {
    let ty = data.equipment_type(item.equipment_type);
    let other_empty = item
        .slot
        .opposite()
        .is_none_or(|o| !items.iter().any(|i| i.slot == o));
    let grip = |unsheathed| Grip {
        unsheathed,
        off_hand: item.slot.is_off_hand(),
        versatile: item.versatile && other_empty,
        alternative: false,
    };
    WeaponPlacement {
        slot: item.slot,
        sheathed: attachment(ty, grip(false)),
        unsheathed: attachment(ty, grip(true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic types in the shape of the install's entries (values are our own).
    fn sword() -> EquipmentType {
        EquipmentType {
            id: "sword".into(),
            name: "Sword".into(),
            weapon_type_one_handed: Some("Slash1".into()),
            weapon_type_two_handed: Some("Slash2".into()),
            bone_main_sheathed: Some("Back_R".into()),
            bone_main_unsheathed: Some("Hand_R".into()),
            bone_off_hand_sheathed: Some("Back_L".into()),
            bone_off_hand_unsheathed: Some("Hand_L".into()),
            bone_versatile_sheathed: Some("Back_V".into()),
            bone_versatile_unsheathed: "Hand_V".into(),
            source_bone_sheathed: Some("Sheath".into()),
            source_bone_versatile_sheathed: Some("Sheath_V".into()),
            source_bone_versatile_unsheathed: "Attach_V".into(),
            source_bone_alternative_unsheathed: "Attach_Alt".into(),
        }
    }

    fn shield() -> EquipmentType {
        EquipmentType {
            id: "shield".into(),
            weapon_type_one_handed: Some("Board".into()),
            weapon_type_two_handed: Some("Board".into()),
            bone_main_unsheathed: Some("Hand_L".into()),
            bone_off_hand_unsheathed: Some("Hand_L".into()),
            bone_versatile_unsheathed: "Dummy_R_Hand".into(),
            source_bone_versatile_unsheathed: DUMMY_ATTACHMENT.into(),
            source_bone_alternative_unsheathed: DUMMY_ATTACHMENT.into(),
            ..Default::default()
        }
    }

    fn at(bone: Option<&str>, source: &str) -> Attachment {
        Attachment {
            bone: bone.map(str::to_owned),
            source: source.to_owned(),
        }
    }

    fn grip(unsheathed: bool, off_hand: bool, versatile: bool, alternative: bool) -> Grip {
        Grip {
            unsheathed,
            off_hand,
            versatile,
            alternative,
        }
    }

    #[test]
    fn bone_selection_covers_every_branch() {
        let t = sword();
        let a = |g| attachment(Some(&t), g);
        // Plain grip: drawn keeps Dummy_Attachment, sheathed uses SourceBoneSheathed.
        assert_eq!(
            a(grip(true, false, false, false)),
            at(Some("Hand_R"), DUMMY_ATTACHMENT)
        );
        assert_eq!(
            a(grip(false, false, false, false)),
            at(Some("Back_R"), "Sheath")
        );
        assert_eq!(
            a(grip(true, true, false, false)),
            at(Some("Hand_L"), DUMMY_ATTACHMENT)
        );
        assert_eq!(
            a(grip(false, true, false, false)),
            at(Some("Back_L"), "Sheath")
        );
        // Versatile: versatile bones and source bones; the off hand keeps its own bones.
        assert_eq!(
            a(grip(true, false, true, false)),
            at(Some("Hand_V"), "Attach_V")
        );
        assert_eq!(
            a(grip(false, false, true, false)),
            at(Some("Back_V"), "Sheath_V")
        );
        assert_eq!(
            a(grip(true, true, true, false)),
            at(Some("Hand_L"), "Attach_V")
        );
        assert_eq!(
            a(grip(false, true, true, false)),
            at(Some("Back_L"), "Sheath_V")
        );
        // Alternative grip only matters when drawn.
        assert_eq!(
            a(grip(true, false, false, true)),
            at(Some("Hand_V"), "Attach_Alt")
        );
        assert_eq!(
            a(grip(true, true, true, true)),
            at(Some("Hand_L"), "Attach_Alt")
        );
        assert_eq!(
            a(grip(false, false, false, true)),
            at(Some("Back_R"), "Sheath")
        );
    }

    #[test]
    fn untyped_weapons_and_unnamed_bones() {
        assert_eq!(
            attachment(None, grip(true, true, false, false)),
            at(Some("Dummy_R_Hand"), DUMMY_ATTACHMENT)
        );
        assert_eq!(
            attachment(None, grip(false, false, false, false)),
            at(Some("Dummy_Weapon_R_1H"), DUMMY_SHEATH)
        );
        // A sheathed shield has no bone: not drawn. Its source falls back to Dummy_Attachment.
        let s = shield();
        assert_eq!(
            attachment(Some(&s), grip(false, true, false, false)),
            at(None, DUMMY_ATTACHMENT)
        );
        assert_eq!(
            attachment(Some(&s), grip(true, true, false, false)),
            at(Some("Hand_L"), DUMMY_ATTACHMENT)
        );
    }

    #[test]
    fn weapon_type_pairs() {
        let (sw, sh) = (sword(), shield());
        let w = |ty, two_handed| Some(HeldWeapon { ty, two_handed });
        assert_eq!(weapon_types(None, None), (UNARMED.into(), UNARMED.into()));
        // A lone versatile/two-handed weapon uses its two-handed type.
        assert_eq!(
            weapon_types(w(Some(&sw), true), None),
            ("Slash2".into(), UNARMED.into())
        );
        assert_eq!(
            weapon_types(w(Some(&sw), false), None),
            ("Slash1".into(), UNARMED.into())
        );
        // Both hands: one-handed types.
        assert_eq!(
            weapon_types(w(Some(&sw), true), w(Some(&sh), false)),
            ("Slash1".into(), "Board".into())
        );
        assert_eq!(
            weapon_types(None, w(Some(&sh), false)),
            (UNARMED.into(), "Board".into())
        );
        // An untyped weapon leaves its hand unarmed.
        assert_eq!(
            weapon_types(w(None, true), w(Some(&sh), false)),
            (UNARMED.into(), "Board".into())
        );
    }

    fn data() -> WeaponData {
        let mut d = WeaponData::default();
        d.equipment_types.insert("sword".into(), sword());
        d.equipment_types.insert("shield".into(), shield());
        for (m, o, s) in [
            ("Slash1", UNARMED, "set-1h"),
            ("Slash2", UNARMED, "set-2h"),
            ("Slash1", "Board", "set-shield"),
            ("Voice", UNARMED, "set-voice"),
        ] {
            d.animation_sets.insert((m.into(), o.into()), s.into());
        }
        d
    }

    fn item(slot: WeaponSlot, ty: &str, versatile: bool) -> WeaponItem<'_> {
        WeaponItem {
            slot,
            equipment_type: Some(ty),
            two_handed: false,
            versatile,
        }
    }

    #[test]
    fn subset_follows_state_and_slots() {
        let d = data();
        let sword_only = [item(WeaponSlot::MeleeMain, "sword", true)];
        let sword_shield = [
            item(WeaponSlot::MeleeMain, "sword", true),
            item(WeaponSlot::MeleeOff, "shield", false),
        ];
        assert_eq!(
            subset_for_state(&d, UnsheathState::Melee, &sword_only),
            "set-2h"
        );
        assert_eq!(
            subset_for_state(&d, UnsheathState::Melee, &sword_shield),
            "set-shield"
        );
        assert_eq!(
            subset_for_state(&d, UnsheathState::Sheathed, &sword_shield),
            ""
        );
        // No ranged weapon: (Unarmed1H, Unarmed1H) is not listed → default subset.
        assert_eq!(
            subset_for_state(&d, UnsheathState::Ranged, &sword_shield),
            ""
        );
        assert_eq!(
            subset_for_state(&d, UnsheathState::Instrument, &sword_shield),
            "set-voice"
        );
        assert_eq!(d.animation_subset("", ""), "");
    }

    #[test]
    fn placement_uses_versatile_only_with_a_free_hand() {
        let d = data();
        let alone = [item(WeaponSlot::MeleeMain, "sword", true)];
        let p = placement(&d, &alone[0], &alone);
        assert_eq!(p.sheathed, at(Some("Back_V"), "Sheath_V"));
        assert_eq!(p.unsheathed, at(Some("Hand_V"), "Attach_V"));
        let both = [
            item(WeaponSlot::MeleeMain, "sword", true),
            item(WeaponSlot::MeleeOff, "shield", false),
        ];
        let p = placement(&d, &both[0], &both);
        assert_eq!(p.sheathed, at(Some("Back_R"), "Sheath"));
        assert_eq!(
            p.for_state(UnsheathState::Melee).bone.as_deref(),
            Some("Hand_R")
        );
        assert_eq!(
            p.for_state(UnsheathState::Ranged).bone.as_deref(),
            Some("Back_R")
        );
        let s = placement(&d, &both[1], &both);
        assert_eq!(s.for_state(UnsheathState::Sheathed).bone, None);
        assert_eq!(
            s.for_state(UnsheathState::Melee).bone.as_deref(),
            Some("Hand_L")
        );
    }

    #[test]
    fn spell_sheathing() {
        use UnsheathState::*;
        assert_eq!(state_from_sheathing("Melee", Sheathed, true), Melee);
        assert_eq!(state_from_sheathing("Ranged", Melee, false), Ranged);
        assert_eq!(state_from_sheathing("Instrument", Melee, false), Instrument);
        assert_eq!(state_from_sheathing("Sheathed", Melee, false), Sheathed);
        assert_eq!(state_from_sheathing("DontChange", Ranged, false), Ranged);
        assert_eq!(state_from_sheathing("Somatic", Melee, false), Melee);
        assert_eq!(state_from_sheathing("WeaponSet", Sheathed, true), Ranged);
        assert_eq!(state_from_sheathing("WeaponSet", Sheathed, false), Melee);
        assert!(Melee.draws(WeaponSlot::MeleeOff));
        assert!(!Melee.draws(WeaponSlot::RangedMain));
        assert!(!Sheathed.draws(WeaponSlot::MeleeMain));
    }

    #[test]
    fn reads_both_files_with_engine_defaults() {
        use bg3_lsf::{Node, Region, Resource, Value};
        let fs = |s: &str| Value::FixedString(s.to_owned());
        let ty = Node::new("EquipmentType")
            .with_attr("UUID", fs("t1"))
            .with_attr("Name", fs("Stick"))
            .with_attr("BoneMainSheathed", fs("Back_R"));
        let set = Node::new("WeaponAnimationSet")
            .with_attr("MainHand", fs("Bow2"))
            .with_attr("AnimationSetUUID", fs("set-bow"));
        let mut d = WeaponData::default();
        for (name, child) in [("EquipmentTypes", ty), ("WeaponAnimationSetData", set)] {
            d.read(&Resource {
                metadata: Default::default(),
                regions: vec![Region {
                    name: name.into(),
                    node: Node::new("root").with_child(child),
                }],
            });
        }
        let t = &d.equipment_types["t1"];
        assert_eq!(t.bone_main_sheathed.as_deref(), Some("Back_R"));
        assert_eq!(t.bone_versatile_unsheathed, "Dummy_R_Hand");
        assert_eq!(t.source_bone_versatile_unsheathed, DUMMY_ATTACHMENT);
        assert_eq!(t.source_bone_sheathed, None);
        assert_eq!(d.animation_subset("Bow2", ""), "set-bow");
    }
}
