//! Root templates (`Public/<Module>/RootTemplates/_merged.lsf`), keyed by `MapKey`.

use std::collections::HashMap;

use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::{Result, parse, str_attr};

#[derive(Debug, Clone)]
pub struct RootTemplate {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub parent: Option<String>,
    pub visual_template: Option<String>,
    pub physics_template: Option<String>,
    /// `IsBlocker`, if this template sets it (scenery helpers: invisible blockers, camera
    /// sliders, fade proxies).
    pub is_blocker: Option<bool>,
    /// `EquipmentTypeID` (items): the `EquipmentTypes.lsx` entry (see `unsheath.rs`).
    pub equipment_type: Option<String>,
    /// Worn visuals and per-item race fallback overrides (`Equipment` child).
    pub equipment: Option<crate::characters::EquipmentVisuals>,
    /// Light fields set on this template (`light` / `CombinedLight` templates only).
    pub light: Option<crate::lights::LightProps>,
    /// Character fields set on this template (`character` templates only).
    pub character: Option<crate::characters::CharacterProps>,
    /// Light probe fields set on this template (`lightProbe` templates only).
    pub probe: Option<crate::probes::ProbeProps>,
    /// Fog volume fields set on this template (`fogVolume` templates only).
    pub fog: Option<crate::fog::FogProps>,
    /// Decal fields set on this template (`decal` templates only).
    pub decal: Option<crate::decals::DecalProps>,
    /// `RecieveDecal` (sic), if this template sets it.
    pub receive_decal: Option<bool>,
}

#[derive(Debug, Default)]
pub struct RootTemplates {
    pub templates: HashMap<String, RootTemplate>,
}

impl RootTemplates {
    pub fn load(vfs: &Vfs) -> Result<Self> {
        Self::load_modules(vfs, BASE_LOAD_ORDER)
    }

    pub fn load_modules(vfs: &Vfs, modules: &[&str]) -> Result<Self> {
        let mut out = Self::default();
        for module in modules {
            let path = format!("Public/{module}/RootTemplates/_merged.lsf");
            if !vfs.exists(&path) {
                continue;
            }
            let res = parse(vfs, &path)?;
            for t in game_objects(&res) {
                out.templates.insert(t.id.clone(), t);
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Option<&RootTemplate> {
        self.templates.get(id)
    }

    /// Resolve the visual for a template, walking `ParentTemplateId` when unset.
    ///
    /// Assumption: an empty field means "inherit from parent" (to be confirmed).
    pub fn visual_for(&self, id: &str) -> Option<&str> {
        let mut current = self.get(id)?;
        for _ in 0..32 {
            if let Some(v) = &current.visual_template {
                return Some(v);
            }
            current = self.get(current.parent.as_deref()?)?;
        }
        None
    }

    /// `IsBlocker` of a level object: its own value, else the first template up the
    /// `ParentTemplateId` chain that sets it, else false.
    ///
    /// Template fields are value/is-set pairs (`eoc::SceneryTemplate::Visit`); an unset field
    /// takes the parent's value, as for [`RootTemplates::visual_for`].
    pub fn is_blocker(&self, obj: &crate::LevelObject) -> bool {
        if let Some(b) = obj.is_blocker {
            return b;
        }
        let mut current = obj.template.as_deref().and_then(|t| self.get(t));
        for _ in 0..32 {
            let Some(t) = current else { break };
            if let Some(b) = t.is_blocker {
                return b;
            }
            current = t.parent.as_deref().and_then(|p| self.get(p));
        }
        false
    }

    /// Whether decals project onto a level object: its own `RecieveDecal` (`own`, from
    /// [`crate::Level::receive_decal`]), else the first template up the chain that sets it,
    /// else true (the field's default in `ls::GameObjectTemplate::Visit`). The renderer passes
    /// it to the object's G-buffer pass as `_ObjectReceiveDecal`.
    // bg3: ls::GameObjectTemplate::Visit (4.1.1.7631656)
    pub fn receives_decals(&self, obj: &crate::LevelObject, own: Option<bool>) -> bool {
        if let Some(b) = own {
            return b;
        }
        let mut current = obj.template.as_deref().and_then(|t| self.get(t));
        for _ in 0..32 {
            let Some(t) = current else { break };
            if let Some(b) = t.receive_decal {
                return b;
            }
            current = t.parent.as_deref().and_then(|p| self.get(p));
        }
        true
    }
}

impl RootTemplates {
    /// The item's `EquipmentTypeID`, walking `ParentTemplateId` when unset (same sparse-field
    /// assumption as [`Self::visual_for`]).
    pub fn equipment_type_for(&self, id: &str) -> Option<&str> {
        let mut current = self.get(id)?;
        for _ in 0..32 {
            if let Some(v) = &current.equipment_type {
                return Some(v);
            }
            current = self.get(current.parent.as_deref()?)?;
        }
        None
    }
}

pub(crate) fn game_objects(res: &bg3_lsf::Resource) -> impl Iterator<Item = RootTemplate> + '_ {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children.iter().filter(|c| c.name == "GameObjects"))
        .filter_map(|n| {
            let kind = str_attr(n, "Type").unwrap_or_default();
            let light = matches!(kind.as_str(), "light" | "CombinedLight")
                .then(|| crate::lights::LightProps::read(n));
            let character =
                (kind == "character").then(|| crate::characters::CharacterProps::read(n));
            let probe = (kind == "lightProbe").then(|| crate::probes::ProbeProps::read(n));
            let fog = (kind == "fogVolume").then(|| crate::fog::FogProps::read(n));
            let decal = (kind == "decal").then(|| crate::decals::DecalProps::read(n));
            Some(RootTemplate {
                id: str_attr(n, "MapKey")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                kind,
                parent: str_attr(n, "ParentTemplateId"),
                visual_template: str_attr(n, "VisualTemplate"),
                physics_template: str_attr(n, "PhysicsTemplate"),
                is_blocker: n.attr("IsBlocker").and_then(|v| v.as_bool()),
                equipment_type: str_attr(n, "EquipmentTypeID"),
                equipment: n
                    .child("Equipment")
                    .map(crate::characters::EquipmentVisuals::read),
                light,
                character,
                probe,
                fog,
                decal,
                receive_decal: n.attr("RecieveDecal").and_then(|v| v.as_bool()),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LevelObject;

    fn template(id: &str, parent: Option<&str>, is_blocker: Option<bool>) -> RootTemplate {
        RootTemplate {
            id: id.into(),
            name: id.into(),
            kind: "scenery".into(),
            parent: parent.map(Into::into),
            visual_template: Some(format!("{id}_visual")),
            physics_template: None,
            is_blocker,
            equipment_type: None,
            equipment: None,
            light: None,
            character: None,
            probe: None,
            fog: None,
            decal: None,
            receive_decal: None,
        }
    }

    fn object(kind: &str, template: &str, is_blocker: Option<bool>) -> LevelObject {
        LevelObject {
            id: "obj".into(),
            name: "obj".into(),
            kind: kind.into(),
            template: Some(template.into()),
            visual_template: None,
            is_blocker,
            transform: Default::default(),
            character: None,
            module: String::new(),
        }
    }

    fn templates() -> RootTemplates {
        let list = [
            template("base", None, None),
            template("helper", Some("base"), Some(true)),
            // Sets nothing itself: inherits `helper`'s blocker flag.
            template("slider", Some("helper"), None),
            template("unblocked", Some("helper"), Some(false)),
            template("wall", Some("base"), None),
        ];
        RootTemplates {
            templates: list.into_iter().map(|t| (t.id.clone(), t)).collect(),
        }
    }

    #[test]
    fn blocker_scenery_has_no_visual() {
        let t = templates();
        assert!(!object("scenery", "helper", None).draws_visual(&t));
        assert!(!object("scenery", "slider", None).draws_visual(&t));
        assert!(object("scenery", "unblocked", None).draws_visual(&t));
        assert!(object("scenery", "wall", None).draws_visual(&t));
        assert!(object("scenery", "missing", None).draws_visual(&t));
    }

    #[test]
    fn instance_is_blocker_overrides_the_template() {
        let t = templates();
        assert!(object("scenery", "helper", Some(false)).draws_visual(&t));
        assert!(!object("scenery", "wall", Some(true)).draws_visual(&t));
    }

    #[test]
    fn receive_decal_defaults_to_true_and_inherits() {
        let mut t = templates();
        t.templates.get_mut("helper").unwrap().receive_decal = Some(false);
        assert!(t.receives_decals(&object("scenery", "wall", None), None));
        assert!(!t.receives_decals(&object("scenery", "slider", None), None));
        assert!(t.receives_decals(&object("scenery", "slider", None), Some(true)));
        assert!(t.receives_decals(&object("scenery", "missing", None), None));
    }

    #[test]
    fn blocker_rule_is_scenery_only() {
        // `IsBlocker` on items (e.g. the Nautiloid's rigged tentacles set it to false) doesn't
        // take part in the scenery visual rule.
        assert!(object("item", "helper", None).draws_visual(&templates()));
    }
}
