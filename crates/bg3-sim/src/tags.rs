//! Tags: definitions from the `Tags` resources and per-entity tag sets.
//!
//! An entity's effective tags (`eoc::TagComponent`) are merged by `esv::TagSystem::Update`
//! from several sources: template tags (`esv::tags::TemplateTagComponent`), tags set by the
//! story (`esv::tags::OsirisTagComponent`), race, boost, dialog, … tags. The story's `SetTag`
//! only edits the Osiris source and marks the entity dirty; the merge runs later in the frame
//! and raises `TagSet` / `TagCleared` for the difference
//! (`esv::StoryGameEventManager::ThrowTagSet` / `ThrowTagCleared`). `IsTagged` looks at the
//! Osiris source first and then at the merged set, so a tag set by the story is visible at
//! once, while a cleared one stays visible until the merge.

use std::collections::HashMap;

use crate::guid::Guid;

/// One tag definition (`Public/<Module>/Tags/<uuid>.lsf`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagDef {
    pub id: Guid,
    pub name: String,
    /// `Categories/Category/Name` values (`Story`, `Dialog`, `Race`, …).
    pub categories: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TagDefs {
    by_id: HashMap<Guid, TagDef>,
    order: Vec<Guid>,
    by_name: HashMap<String, Guid>,
}

impl TagDefs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, def: TagDef) {
        if !self.by_id.contains_key(&def.id) {
            self.order.push(def.id);
        }
        self.by_name.insert(def.name.clone(), def.id);
        self.by_id.insert(def.id, def);
    }

    pub fn get(&self, id: Guid) -> Option<&TagDef> {
        self.by_id.get(&id)
    }

    /// A tag by name (conditions name tags: `Tagged('UNDEAD')`); the last definition wins.
    pub fn by_name(&self, name: &str) -> Option<Guid> {
        self.by_name.get(name).copied()
    }

    /// Definitions in deterministic resource load order.
    pub fn iter(&self) -> impl Iterator<Item = &TagDef> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// Where a tag on an entity comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TagSource {
    /// Root template / level object `Tags` (`TemplateTagComponent`).
    Template,
    /// The story's `SetTag` (`OsirisTagComponent`).
    Osiris,
    /// Everything else the engine adds (race, boosts, …). The sim does not model these
    /// rules yet; tags seen in engine `TagSet` events land here.
    Engine,
    /// Tags added by dialog `Tag` set-flags (`esv::tags::DialogTagComponent`).
    Dialog,
}

/// The tags of one entity: each source's list (insertion order) and the merged set as of the
/// last [`EntityTags::merge`].
#[derive(Clone, Debug, Default)]
pub struct EntityTags {
    sources: Vec<(TagSource, Vec<Guid>)>,
    merged: Vec<Guid>,
}

impl EntityTags {
    pub fn source(&self, s: TagSource) -> &[Guid] {
        self.sources
            .iter()
            .find(|(k, _)| *k == s)
            .map_or(&[], |(_, v)| v.as_slice())
    }

    /// Add to a source; false if it was already there.
    pub fn add(&mut self, s: TagSource, tag: Guid) -> bool {
        let list = match self.sources.iter_mut().position(|(k, _)| *k == s) {
            Some(i) => &mut self.sources[i].1,
            None => {
                self.sources.push((s, Vec::new()));
                &mut self.sources.last_mut().expect("just pushed").1
            }
        };
        if list.contains(&tag) {
            false
        } else {
            list.push(tag);
            true
        }
    }

    /// Remove from a source; false if it was not there.
    pub fn remove(&mut self, s: TagSource, tag: Guid) -> bool {
        match self.sources.iter_mut().find(|(k, _)| *k == s) {
            Some((_, list)) => match list.iter().position(|t| *t == tag) {
                Some(i) => {
                    list.remove(i);
                    true
                }
                None => false,
            },
            None => false,
        }
    }

    /// Put a tag into (or take it out of) the merged set directly, for a merge the engine
    /// reported (`TagSet`/`TagCleared` it raised itself) without announcing it again.
    pub fn force_merged(&mut self, tag: Guid, present: bool) {
        let has = self.merged.contains(&tag);
        if present && !has {
            self.merged.push(tag);
        } else if !present && has {
            self.merged.retain(|t| *t != tag);
        }
    }

    /// The merged set as of the last merge.
    pub fn merged(&self) -> &[Guid] {
        &self.merged
    }

    /// `IsTagged`: the Osiris source, then the merged set.
    // bg3: esv::OsirisGameFunctions::IsTagged → LegacyConversion::IsTagged (4.1.1.7631656)
    pub fn is_tagged(&self, tag: Guid) -> bool {
        self.source(TagSource::Osiris).contains(&tag) || self.merged.contains(&tag)
    }

    /// Whether a dialog sees the tag on a speaker: the merged set, the Osiris source and the
    /// dialog source.
    // bg3: esv::EoCDialogGameInterface::IsSpeakerTagged → esv::StoryUtils::IsTagged (4.1.1.7631656)
    pub fn is_tagged_for_dialog(&self, tag: Guid) -> bool {
        self.is_tagged(tag) || self.source(TagSource::Dialog).contains(&tag)
    }

    /// Recompute the merged set (sources in [`TagSource`] order, first occurrence wins) and
    /// return `(added, removed)` in merged order.
    // bg3: esv::TagSystem::Update (4.1.1.7631656)
    pub fn merge(&mut self) -> (Vec<Guid>, Vec<Guid>) {
        let next = self.compute();
        let added = next
            .iter()
            .filter(|t| !self.merged.contains(t))
            .copied()
            .collect();
        let removed = self
            .merged
            .iter()
            .filter(|t| !next.contains(t))
            .copied()
            .collect();
        self.merged = next;
        (added, removed)
    }

    fn compute(&self) -> Vec<Guid> {
        let mut order: Vec<TagSource> = self.sources.iter().map(|(k, _)| *k).collect();
        order.sort();
        let mut next: Vec<Guid> = Vec::new();
        for s in order {
            for t in self.source(s) {
                if !next.contains(t) {
                    next.push(*t);
                }
            }
        }
        next
    }

    /// Whether the next merge would change the merged set.
    pub fn is_dirty(&self) -> bool {
        self.compute() != self.merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_tag_on_top_of_template_tag_does_not_change_the_merged_set() {
        let (a, b) = (Guid(1), Guid(2));
        let mut t = EntityTags::default();
        t.add(TagSource::Template, a);
        assert_eq!(t.merge(), (vec![a], vec![]));
        assert!(t.add(TagSource::Osiris, a));
        assert!(t.add(TagSource::Osiris, b));
        assert!(!t.add(TagSource::Osiris, b));
        assert!(t.is_tagged(b), "visible before the merge");
        assert_eq!(t.merge(), (vec![b], vec![]));
        assert!(t.remove(TagSource::Osiris, a));
        assert!(t.remove(TagSource::Osiris, b));
        assert!(t.is_tagged(b), "still merged until the next update");
        assert!(t.is_dirty());
        assert_eq!(t.merge(), (vec![], vec![b]));
        assert!(t.is_tagged(a) && !t.is_tagged(b));
        assert!(!t.is_dirty());
    }
}
