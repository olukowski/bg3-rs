//! Entities: the objects the story names by GUIDSTRING.
//!
//! The story identifies an object by `Name_<MapKey>` (level objects and globals) or by a
//! generated name for runtime-created ones (`Gnomes_Male_Deep_Player_<uuid>` for the created
//! avatar, `WPN_HUM_Shortsword_A_0_<uuid>` for generated loot); the engine resolves only the
//! UUID (`esv::StoryUtils::OsirisParamGUIDToGameObject`) and renders objects back as
//! `Name_<uuid>`.

use std::collections::HashMap;

use crate::character::CharacterSheet;
use crate::guid::{Guid, osiris_name};
use crate::status::ActiveStatus;
use crate::tags::EntityTags;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Character,
    Item,
    Trigger,
    /// Anything else placed in a level (scenery, lights, splines, …). The story never uses
    /// these in the opening.
    Other,
    /// Seen only as an event argument whose declared type does not tell.
    Unknown,
}

impl EntityKind {
    /// From a level object `Type` (`character`, `item`, `trigger`, …).
    pub fn from_level_type(t: &str) -> Self {
        match t {
            "character" => Self::Character,
            "item" => Self::Item,
            "trigger" => Self::Trigger,
            _ => Self::Other,
        }
    }

    /// From the Osiris alias type of an argument (`CHARACTER`, `ITEM`, `TRIGGER`).
    pub fn from_osiris_type(t: &str) -> Self {
        match t {
            "CHARACTER" => Self::Character,
            "ITEM" => Self::Item,
            "TRIGGER" => Self::Trigger,
            _ => Self::Unknown,
        }
    }
}

/// How the sim learned about an entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Loaded from level / globals data.
    Data,
    /// Created at runtime by something the sim does not model yet (loot, character creation),
    /// and learned from an engine event that names it.
    Observed,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub id: Guid,
    /// The `Name` part of the story's `Name_<uuid>`.
    pub name: String,
    pub kind: EntityKind,
    pub origin: Origin,
    /// Root template (`TemplateName`).
    pub template: Option<Guid>,
    /// `LevelName` the object belongs to (`GetRegion`).
    pub level: Option<String>,
    /// Defined in a `Globals` directory (exists in every level).
    pub global: bool,
    pub position: [f32; 3],
    pub tags: EntityTags,
    pub statuses: Vec<ActiveStatus>,
    /// The user (`UserID`) controlling a character, if known.
    pub user: Option<i32>,
    /// The inventory holding an item, if known.
    pub holder: Option<Guid>,
    /// Stats entry (`Stats` of the object or its nearest template).
    pub stats: Option<String>,
    /// Current faction.
    pub faction: Option<Guid>,
    /// What the rules need of a character (abilities, HP, level, proficiencies), from its
    /// stats entry; `None` for objects without a `Character` stats entry.
    pub sheet: Option<CharacterSheet>,
}

impl Entity {
    pub fn new(id: Guid, name: &str, kind: EntityKind, origin: Origin) -> Self {
        Self {
            id,
            name: name.to_owned(),
            kind,
            origin,
            template: None,
            level: None,
            global: false,
            position: [0.0; 3],
            tags: EntityTags::default(),
            statuses: Vec::new(),
            user: None,
            holder: None,
            stats: None,
            faction: None,
            sheet: None,
        }
    }

    /// `Name_<uuid>`, as the engine passes the entity to the story.
    pub fn osiris_name(&self) -> String {
        osiris_name(&self.name, self.id)
    }
}

/// All entities, in creation order.
///
/// Every UUID the sim has seen also gets a small numeric **handle** (`u32`) that never changes
/// or gets reused, even when the entity is dropped with its level and comes back later. The
/// rules engine (`bg3_rules::EntityId`) and presentation layers refer to entities by handle.
#[derive(Clone, Debug, Default)]
pub struct Entities {
    list: Vec<Entity>,
    index: HashMap<Guid, usize>,
    /// Handle → UUID, append-only.
    handles: Vec<Guid>,
    handle_of: HashMap<Guid, u32>,
}

impl Entities {
    /// Insert or replace (by id, keeping the original position in the order).
    pub fn insert(&mut self, e: Entity) -> &mut Entity {
        self.handle_or_assign(e.id);
        match self.index.get(&e.id) {
            Some(&i) => {
                self.list[i] = e;
                &mut self.list[i]
            }
            None => {
                self.index.insert(e.id, self.list.len());
                self.list.push(e);
                self.list.last_mut().expect("just pushed")
            }
        }
    }

    fn handle_or_assign(&mut self, id: Guid) -> u32 {
        if let Some(&h) = self.handle_of.get(&id) {
            return h;
        }
        let h = self.handles.len() as u32;
        self.handles.push(id);
        self.handle_of.insert(id, h);
        h
    }

    /// The handle of an entity the sim knows (or knew).
    pub fn handle(&self, id: Guid) -> Option<u32> {
        self.handle_of.get(&id).copied()
    }

    /// The UUID behind a handle (the entity may no longer exist).
    pub fn id_of(&self, handle: u32) -> Option<Guid> {
        self.handles.get(handle as usize).copied()
    }

    /// The live entity behind a handle.
    pub fn by_handle(&self, handle: u32) -> Option<&Entity> {
        self.get(self.id_of(handle)?)
    }

    pub fn by_handle_mut(&mut self, handle: u32) -> Option<&mut Entity> {
        let id = self.id_of(handle)?;
        self.get_mut(id)
    }

    /// Drop every entity matching `f`, keeping the order of the rest (handles stay valid).
    pub fn remove_where(&mut self, f: impl Fn(&Entity) -> bool) {
        self.list.retain(|e| !f(e));
        self.index = self
            .list
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();
    }

    pub fn get(&self, id: Guid) -> Option<&Entity> {
        self.index.get(&id).map(|&i| &self.list[i])
    }

    pub fn get_mut(&mut self, id: Guid) -> Option<&mut Entity> {
        self.index.get(&id).map(|&i| &mut self.list[i])
    }

    pub fn contains(&self, id: Guid) -> bool {
        self.index.contains_key(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.list.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Entity> {
        self.list.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}
