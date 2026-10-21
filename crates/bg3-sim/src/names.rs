//! Localized object names; the story API and presentation share handle inheritance.
use bg3_loca::Loca;
use bg3_vfs::Vfs;

use crate::{Guid, Sim, load::LoadError};

pub fn load_localization(vfs: &Vfs, language: &str) -> Result<Loca, LoadError> {
    let mut loca = Loca::new();
    let mut files: Vec<_> = vfs
        .files_under(&format!("Localization/{language}/"))
        .filter(|f| f.entry.name.to_ascii_lowercase().ends_with(".loca"))
        .collect();
    files.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    for file in files {
        loca.merge_bytes(&file.read()?)?;
    }
    Ok(loca)
}

impl Sim {
    /// Runtime override, placed-object DisplayName, then nearest root-template DisplayName.
    // bg3: esv::OsirisGameFunctions::GetDisplayName (4.1.1.7631656)
    pub fn display_name_handle(&self, id: Guid) -> Option<&str> {
        let e = self.entity(id)?;
        e.story.display_name.as_deref().or_else(|| {
            let objects = &self.data.story.objects;
            objects
                .get(&id)
                .and_then(|d| d.display_name.as_deref())
                .or_else(|| {
                    self.data
                        .template_chain(e.template)
                        .into_iter()
                        .find_map(|t| objects.get(&t.id)?.display_name.as_deref())
                })
        })
    }

    /// Localized presentation name. Missing translations leave fallback choice to the caller.
    // bg3: esv::OsirisGameFunctions::GetDisplayName (4.1.1.7631656)
    pub fn display_name(&self, id: Guid) -> Option<String> {
        self.data
            .loca
            .text(self.display_name_handle(id)?)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entity, EntityKind, GameData, Origin, TemplateInfo, story_data::object_data};
    use bg3_lsf::{Attribute, Node, TranslatedString, Value};

    #[test]
    fn translated_template_name_placed_and_story_overrides() {
        let id = Guid(1);
        let template = Guid(2);
        let parent = Guid(3);
        let mut data = GameData {
            loca: Loca::parse(&bg3_loca::write([
                ("h_test_template", 1, "Test creature"),
                ("h_test_placed", 1, "Placed creature"),
                ("h_test_override", 1, "Chosen name"),
            ]))
            .unwrap(),
            ..Default::default()
        };
        for (id, parent) in [(template, Some(parent)), (parent, None)] {
            data.templates.insert(
                id,
                TemplateInfo {
                    id,
                    parent,
                    name: "Synthetic".into(),
                    kind: EntityKind::Character,
                    tags: vec![],
                    race: None,
                    stats: None,
                    statuses: vec![],
                    faction: None,
                    level_override: None,
                },
            );
        }
        let mut node = Node::new("GameObjects");
        node.attributes.push(Attribute {
            name: "DisplayName".into(),
            value: Value::TranslatedString(TranslatedString {
                value: None,
                handle: "h_test_template".into(),
                version: 1,
            }),
        });
        data.story.objects.insert(parent, object_data(&node));
        let mut entity = Entity::new(
            id,
            "Internal_Object_Name",
            EntityKind::Character,
            Origin::Data,
        );
        entity.template = Some(template);
        let mut sim = Sim::new(data);
        sim.entities.insert(entity);
        assert_eq!(sim.display_name(id).as_deref(), Some("Test creature"));
        assert_eq!(sim.display_name(Guid(99)), None);
        let data = std::sync::Arc::make_mut(&mut sim.data);
        node.attributes[0].value = Value::TranslatedString(TranslatedString {
            value: None,
            handle: "h_test_placed".into(),
            version: 1,
        });
        data.story.objects.insert(id, object_data(&node));
        assert_eq!(sim.display_name(id).as_deref(), Some("Placed creature"));
        sim.entities.get_mut(id).unwrap().story.display_name = Some("h_test_override".into());
        assert_eq!(sim.display_name(id).as_deref(), Some("Chosen name"));
        sim.entities.get_mut(id).unwrap().story.display_name = Some("h_missing".into());
        assert_eq!(
            sim.display_name(id),
            None,
            "missing translation lets caller use the object name"
        );
    }
}
