//! Story flags: definitions from the `Flags` resources and their runtime values.
//!
//! Definitions live in `Public/<Module>/Flags/<uuid>.lsf` (region `Flags`: `UUID`, `Name`,
//! `Description`, `Usage`), loaded by `ls::FlagManager` (`ls::Flag::Visit` reads `Usage` as a
//! byte). The runtime values are kept by `esv::OsirisVariableHelper` as *individual* flags,
//! one set per object plus one for the null object (global flags); user and party flags are
//! individual flags copied onto every character of the user / party.
//!
//! `scr::SetFlag` (4.1.1.7631656) dispatches on `Usage`; see [`FlagUsage`].

use std::collections::{HashMap, HashSet};

use crate::guid::Guid;

/// `Usage` byte of a flag definition, as `scr::SetFlag` interprets it.
// bg3: scr::SetFlag, scr::GetFlag (4.1.1.7631656)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagUsage {
    /// 0 or 1: not settable from the story (`scr::SetFlag` returns error 13).
    Unscriptable(u8),
    /// 2: user flag, needs a character; stored on every party character with the same user id
    /// (`OsirisVariableHelper::SetUserFlag`, last bool false).
    User,
    /// 3: party flag, needs a character (`OsirisVariableHelper::SetPartyFlag`).
    Party,
    /// 4: object flag on a character or item (`SetIndividualFlag` on the object).
    Object,
    /// 5: global flag; the object argument is ignored (`SetIndividualFlag` on the null object).
    Global,
    /// 6: object flag kept in the character's dialog variable manager
    /// (`(anonymous)::SetFlagDialog`). Behaves as an object flag here.
    Local,
    /// 7: user flag variant (`SetUserFlag` with its last bool true).
    UserShared,
    /// Anything above 7: error.
    Other(u8),
}

impl FlagUsage {
    pub fn from_raw(v: u8) -> Self {
        match v {
            0 | 1 => Self::Unscriptable(v),
            2 => Self::User,
            3 => Self::Party,
            4 => Self::Object,
            5 => Self::Global,
            6 => Self::Local,
            7 => Self::UserShared,
            _ => Self::Other(v),
        }
    }

    pub fn raw(self) -> u8 {
        match self {
            Self::Unscriptable(v) | Self::Other(v) => v,
            Self::User => 2,
            Self::Party => 3,
            Self::Object => 4,
            Self::Global => 5,
            Self::Local => 6,
            Self::UserShared => 7,
        }
    }

    /// User and party flags need a character (`1 << usage & 0x8c` in `scr::SetFlag`).
    pub fn needs_character(self) -> bool {
        matches!(self, Self::User | Self::Party | Self::UserShared)
    }
}

/// One flag definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagDef {
    pub id: Guid,
    pub name: String,
    pub usage: FlagUsage,
    pub description: String,
}

/// All flag definitions, keyed by UUID.
#[derive(Clone, Debug, Default)]
pub struct FlagDefs {
    by_id: HashMap<Guid, FlagDef>,
}

impl FlagDefs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace, for a later module) a definition.
    pub fn insert(&mut self, def: FlagDef) {
        self.by_id.insert(def.id, def);
    }

    pub fn get(&self, id: Guid) -> Option<&FlagDef> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FlagDef> {
        self.by_id.values()
    }
}

/// Why a flag operation failed. The numbers are `scr::Error` codes returned by
/// `scr::SetFlag`; `esv::OsirisDialogFunctions::SetFlag` turns them into a failed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlagError {
    /// 12: null or unknown flag.
    #[error("unknown flag")]
    UnknownFlag,
    /// 13: `Usage` 0 or 1.
    #[error("flag cannot be used from the story")]
    Unscriptable,
    /// 0: the object is null or no longer exists (non-global flag).
    #[error("object does not exist")]
    NoObject,
    /// 2: a user/party flag on something that is not a character.
    #[error("user/party flag needs a character")]
    NotCharacter,
    /// 1: an object flag on something that is neither a character nor an item.
    #[error("object flag needs a character or an item")]
    NotGameObject,
    /// `Usage` > 7.
    #[error("unsupported flag usage")]
    BadUsage,
}

/// Runtime flag values: `(holder, flag)` pairs, where the holder is the null id for global
/// flags.
// bg3: esv::OsirisVariableHelper::SetIndividualFlag / GetIndividualFlag (4.1.1.7631656)
#[derive(Clone, Debug, Default)]
pub struct FlagValues {
    set: HashSet<(Guid, Guid)>,
}

impl FlagValues {
    pub fn get(&self, holder: Guid, flag: Guid) -> bool {
        self.set.contains(&(holder, flag))
    }

    /// Set or clear; returns whether the value changed.
    pub fn put(&mut self, holder: Guid, flag: Guid, value: bool) -> bool {
        if value {
            self.set.insert((holder, flag))
        } else {
            self.set.remove(&(holder, flag))
        }
    }

    /// Every set flag of a holder (the null id for global flags), unordered.
    pub fn of_holder(&self, holder: Guid) -> impl Iterator<Item = Guid> + '_ {
        self.set
            .iter()
            .filter(move |(h, _)| *h == holder)
            .map(|(_, f)| *f)
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}
