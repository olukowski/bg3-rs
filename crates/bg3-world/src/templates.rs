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
    /// Worn visuals and per-item race fallback overrides (`Equipment` child).
    pub equipment: Option<crate::characters::EquipmentVisuals>,
    /// Light fields set on this template (`light` / `CombinedLight` templates only).
    pub light: Option<crate::lights::LightProps>,
    /// Character fields set on this template (`character` templates only).
    pub character: Option<crate::characters::CharacterProps>,
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
            Some(RootTemplate {
                id: str_attr(n, "MapKey")?,
                name: str_attr(n, "Name").unwrap_or_default(),
                kind,
                parent: str_attr(n, "ParentTemplateId"),
                visual_template: str_attr(n, "VisualTemplate"),
                physics_template: str_attr(n, "PhysicsTemplate"),
                equipment: n
                    .child("Equipment")
                    .map(crate::characters::EquipmentVisuals::read),
                light,
                character,
            })
        })
}
