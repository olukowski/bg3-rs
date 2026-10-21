//! A small, object-safe view of the story state for other systems (the dialogue runner reads
//! and writes flags and checks tags through it). [`crate::Sim`] implements it; tests can use
//! their own implementation.

use crate::entity::{Entity, EntityKind};
use crate::flags::FlagError;
use crate::guid::Guid;
use crate::sim::Sim;

/// Flags, tags and entity lookup. Objects and flags are identified by UUID (parse the story's
/// `Name_<uuid>` with [`Guid::from_osiris`]). Writes queue the same story events as the
/// Osiris calls (`FlagSet`, `TagSet`, …); the owner of the sim delivers them.
pub trait StoryState {
    /// `GetFlag(flag, object)`; `object` is ignored for global flags (pass [`Guid::NULL`]).
    fn flag(&self, flag: Guid, object: Guid) -> Result<bool, FlagError>;
    /// `SetFlag` / `ClearFlag` with a dialog instance id (0 outside dialogs).
    fn set_flag(
        &mut self,
        flag: Guid,
        object: Guid,
        value: bool,
        dialog_instance: i32,
    ) -> Result<(), FlagError>;
    /// `IsTagged(object, tag)`.
    fn is_tagged(&self, object: Guid, tag: Guid) -> bool;
    /// `SetTag` / `ClearTag`; false if the object cannot carry story tags.
    fn set_tag(&mut self, object: Guid, tag: Guid, value: bool) -> bool;
    /// `Exists(object)`.
    fn exists(&self, object: Guid) -> bool;
    /// The object's kind, if it exists.
    fn kind(&self, object: Guid) -> Option<EntityKind>;
    /// The story's rendering of an object (`Name_<uuid>`).
    fn render(&self, object: Guid) -> String;
    /// Flag name by UUID (`GetFlagName`).
    fn flag_name(&self, flag: Guid) -> Option<&str>;
    /// Tag name by UUID.
    fn tag_name(&self, tag: Guid) -> Option<&str>;
}

impl StoryState for Sim {
    fn flag(&self, flag: Guid, object: Guid) -> Result<bool, FlagError> {
        self.get_flag(flag, object)
    }

    fn set_flag(
        &mut self,
        flag: Guid,
        object: Guid,
        value: bool,
        dialog_instance: i32,
    ) -> Result<(), FlagError> {
        Sim::set_flag(self, flag, object, value, dialog_instance, true)
    }

    fn is_tagged(&self, object: Guid, tag: Guid) -> bool {
        Sim::is_tagged(self, object, tag)
    }

    fn set_tag(&mut self, object: Guid, tag: Guid, value: bool) -> bool {
        if value {
            Sim::set_tag(self, object, tag)
        } else {
            self.clear_tag(object, tag)
        }
    }

    fn exists(&self, object: Guid) -> bool {
        Sim::exists(self, object)
    }

    fn kind(&self, object: Guid) -> Option<EntityKind> {
        self.entity(object).map(|e: &Entity| e.kind)
    }

    fn render(&self, object: Guid) -> String {
        Sim::render(self, object)
    }

    fn flag_name(&self, flag: Guid) -> Option<&str> {
        self.data.flags.get(flag).map(|d| d.name.as_str())
    }

    fn tag_name(&self, tag: Guid) -> Option<&str> {
        self.data.tags.get(tag).map(|d| d.name.as_str())
    }
}
