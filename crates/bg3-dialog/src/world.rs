//! The game state a dialog reads and writes, behind the [`DialogWorld`] trait, and a simple
//! in-memory implementation ([`MemoryWorld`]).
//!
//! The runner resolves *which* flag store a dialog flag refers to (flag type, speaker); the
//! world decides how that store behaves (e.g. a `User` flag is shared by all characters of one
//! player, a `Party` flag by the whole party) and reports which objects actually changed, so the
//! runner can emit `FlagSet` / `FlagCleared` exactly when the game would.
//! Background: `docs/formats/dialog.md` ("Flags").

use std::collections::{HashMap, HashSet};

use bg3_lsf::Guid;

use crate::defs::Definitions;

/// A game object taking part in a dialog (a character or item), identified by its UUID.
pub type ActorId = Guid;

/// Flag storage type: the `Usage` byte of a flag resource (`Public/*/Flags/*.lsf`), which is the
/// engine's `EFlagType` enum.
// bg3: ls::anubis::game::EFlagType names table (4.1.1.7631656)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FlagType {
    /// 1: one global value, invisible to Osiris events.
    Local,
    /// 2: per player (all characters controlled by the same user).
    User,
    /// 3: per party.
    Party,
    /// 4: per object ("Object" in dialog files).
    Character,
    /// 5: one global value.
    Global,
    /// 6: per object, in the object's dialog-flag store.
    Dialog,
    /// 7: like `User`, in the dialog-flag store.
    UserShape,
    /// 8: per object (and nearby party members), in the dialog-flag store.
    DialogInstance,
}

impl FlagType {
    /// From a flag resource's `Usage` byte. `0` (Invalid) and unknown values give `None`.
    pub fn from_usage(usage: u8) -> Option<Self> {
        Some(match usage {
            1 => Self::Local,
            2 => Self::User,
            3 => Self::Party,
            4 => Self::Character,
            5 => Self::Global,
            6 => Self::Dialog,
            7 => Self::UserShape,
            8 => Self::DialogInstance,
            _ => return None,
        })
    }

    /// Whether the flag lives on a speaker (needs a `paramval`).
    pub fn per_object(self) -> bool {
        !matches!(self, Self::Local | Self::Global)
    }

    /// Whether setting the flag is reported to Osiris (`FlagSet` / `FlagCleared`).
    pub fn raises_events(self) -> bool {
        self != Self::Local
    }
}

/// One object whose flag value changed through [`DialogWorld::set_flag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagChange {
    /// The object, or `None` for global stores.
    pub target: Option<ActorId>,
    /// The new value.
    pub value: bool,
}

/// What a roll node asks the world to roll.
#[derive(Debug, Clone, PartialEq)]
pub struct RollRequest {
    /// The dialog and node (for logging / UI).
    pub dialog: crate::model::DialogRef,
    pub node: Guid,
    /// `true` for `PassiveRoll` nodes.
    pub passive: bool,
    /// The rolling character (the roll node's speaker).
    pub roller: Option<ActorId>,
    /// The character rolled against (`RollTargetSpeaker`).
    pub target: Option<ActorId>,
    pub roll: crate::model::Roll,
}

/// The result of a roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollOutcome {
    pub success: bool,
    /// Critical success / failure (`CRITICALITYTYPE` in Osiris events), if any.
    pub critical: Option<bool>,
}

/// Game state used by dialogs. Everything is keyed by UUIDs; callers that talk to Osiris map
/// them to `Name_uuid` strings themselves.
pub trait DialogWorld {
    /// The storage type of a flag resource (its `Usage`), or `None` if no resource exists.
    fn flag_type(&self, flag: &Guid) -> Option<FlagType>;

    /// Reads a flag. `target` is the speaker's object for per-object types and `None` for
    /// `Global`/`Local`. Unset flags read `false`.
    fn get_flag(&self, flag: &Guid, ty: FlagType, target: Option<&ActorId>) -> bool;

    /// Writes a flag and returns the objects whose value changed (the game raises one Osiris
    /// event per entry; an unchanged value raises none). `dialog_instance` is the running
    /// dialog's instance id.
    fn set_flag(
        &mut self,
        flag: &Guid,
        ty: FlagType,
        target: Option<&ActorId>,
        value: bool,
        dialog_instance: u32,
    ) -> Vec<FlagChange>;

    /// Whether an object has a tag (regular tags or tags added by dialogs).
    fn has_tag(&self, target: &ActorId, tag: &Guid) -> bool;

    /// Adds (`true`) or removes a dialog tag on an object (`Tag` set-flags).
    fn set_dialog_tag(&mut self, target: &ActorId, tag: &Guid, value: bool);

    /// Whether the quest step a `Quest` flag refers to is unlocked.
    fn quest_step_unlocked(&self, _flag: &Guid) -> bool {
        false
    }

    /// Unlocks the quest step of a `Quest` set-flag for `target` (the value is ignored by the
    /// game: a set always unlocks).
    fn unlock_quest_step(&mut self, _flag: &Guid, _target: &ActorId) {}

    /// Evaluates a `Script` flag (`ScriptFlags.lsx` condition script) for the given speakers
    /// (index = speaker slot). The default treats every script flag as false.
    fn script_flag(&self, _flag: &Guid, _speakers: &[Option<ActorId>]) -> bool {
        false
    }

    /// Rolls an ability check for a roll node.
    fn roll(&mut self, request: &RollRequest) -> RollOutcome;

    /// A random index below `n` (`n` ≥ 2), used to pick one of several text lines. The game
    /// uses `rand() % n`; the default always picks the first line.
    fn random(&mut self, _n: usize) -> usize {
        0
    }
}

/// Which store a flag value lives in, inside [`MemoryWorld`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Store {
    /// The object's (or the global) variable store.
    Main,
    /// The object's dialog-flag store (`Dialog`, `UserShape`, `DialogInstance`).
    Dialog,
}

/// A self-contained [`DialogWorld`]: flags and tags in hash maps, flag types from
/// [`Definitions`] (or set by hand), and a roll function you inject.
pub struct MemoryWorld {
    flag_types: HashMap<Guid, FlagType>,
    values: HashMap<(Guid, Store, Option<ActorId>), bool>,
    tags: HashSet<(ActorId, Guid)>,
    quest_steps: HashSet<Guid>,
    script_flags: HashMap<Guid, bool>,
    /// Player user id per character (for `User` / `UserShape` flags).
    pub users: HashMap<ActorId, u32>,
    /// Party members (for `Party` flags).
    pub party: Vec<ActorId>,
    roller: Box<dyn FnMut(&RollRequest) -> RollOutcome + Send + Sync>,
}

impl std::fmt::Debug for MemoryWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryWorld")
            .field("flags", &self.values.len())
            .field("tags", &self.tags.len())
            .finish_non_exhaustive()
    }
}

impl Default for MemoryWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryWorld {
    /// An empty world whose rolls always succeed.
    pub fn new() -> Self {
        MemoryWorld {
            flag_types: HashMap::new(),
            values: HashMap::new(),
            tags: HashSet::new(),
            quest_steps: HashSet::new(),
            script_flags: HashMap::new(),
            users: HashMap::new(),
            party: Vec::new(),
            roller: Box::new(|_| RollOutcome {
                success: true,
                critical: None,
            }),
        }
    }

    /// An empty world that knows the flag types of every flag resource in `defs`.
    pub fn with_definitions(defs: &Definitions) -> Self {
        let mut w = Self::new();
        w.flag_types = defs
            .flags
            .values()
            .filter_map(|f| Some((f.uuid, FlagType::from_usage(f.usage)?)))
            .collect();
        w
    }

    /// Declares a flag resource (as if it had `Usage` = `ty`).
    pub fn define_flag(&mut self, flag: Guid, ty: FlagType) {
        self.flag_types.insert(flag, ty);
    }

    /// Replaces the roll function.
    pub fn set_roller(
        &mut self,
        roller: impl FnMut(&RollRequest) -> RollOutcome + Send + Sync + 'static,
    ) {
        self.roller = Box::new(roller);
    }

    pub fn add_tag(&mut self, target: ActorId, tag: Guid) {
        self.tags.insert((target, tag));
    }

    pub fn set_quest_step(&mut self, flag: Guid, unlocked: bool) {
        if unlocked {
            self.quest_steps.insert(flag);
        } else {
            self.quest_steps.remove(&flag);
        }
    }

    /// Fixes the result of a script flag (unset script flags are false).
    pub fn set_script_flag(&mut self, flag: Guid, value: bool) {
        self.script_flags.insert(flag, value);
    }

    /// All flag values stored so far, as `(flag, object, value)`.
    pub fn flags(&self) -> impl Iterator<Item = (Guid, Option<ActorId>, bool)> + '_ {
        self.values.iter().map(|(&(f, _, t), &v)| (f, t, v))
    }

    fn store_of(ty: FlagType) -> Store {
        match ty {
            FlagType::Dialog | FlagType::UserShape | FlagType::DialogInstance => Store::Dialog,
            _ => Store::Main,
        }
    }

    /// The objects a write of this type fans out to.
    fn write_targets(&self, ty: FlagType, target: Option<&ActorId>) -> Vec<Option<ActorId>> {
        match (ty, target) {
            (FlagType::Global | FlagType::Local, _) => vec![None],
            (_, None) => Vec::new(),
            (FlagType::User | FlagType::UserShape, Some(t)) => {
                let mut out = vec![Some(*t)];
                if let Some(user) = self.users.get(t) {
                    let mut others: Vec<_> = self
                        .users
                        .iter()
                        .filter(|(a, u)| *u == user && *a != t)
                        .map(|(a, _)| Some(*a))
                        .collect();
                    others.sort();
                    out.extend(others);
                }
                out
            }
            (FlagType::Party, Some(t)) => {
                let mut out = vec![Some(*t)];
                out.extend(self.party.iter().filter(|a| *a != t).map(|a| Some(*a)));
                out
            }
            (_, Some(t)) => vec![Some(*t)],
        }
    }
}

impl DialogWorld for MemoryWorld {
    fn flag_type(&self, flag: &Guid) -> Option<FlagType> {
        self.flag_types.get(flag).copied()
    }

    fn get_flag(&self, flag: &Guid, ty: FlagType, target: Option<&ActorId>) -> bool {
        let store = Self::store_of(ty);
        let get = |t: Option<ActorId>| {
            self.values
                .get(&(*flag, store, t))
                .copied()
                .unwrap_or(false)
        };
        match (ty, target) {
            (FlagType::Global | FlagType::Local, _) => get(None),
            (_, None) => false,
            // A User flag is set if any character of the same user has it; a Party flag if any
            // party member has it. UserShape reads only the speaker's own store.
            (FlagType::User | FlagType::Party, Some(t)) => {
                self.write_targets(ty, Some(t)).into_iter().any(get)
            }
            (_, Some(t)) => get(Some(*t)),
        }
    }

    fn set_flag(
        &mut self,
        flag: &Guid,
        ty: FlagType,
        target: Option<&ActorId>,
        value: bool,
        _dialog_instance: u32,
    ) -> Vec<FlagChange> {
        let store = Self::store_of(ty);
        let mut changes = Vec::new();
        for t in self.write_targets(ty, target) {
            // bg3: esv::OsirisVariableHelper::SetIndividualFlag (4.1.1.7631656): no event when an
            // existing value is unchanged; clearing a never-set flag is silent.
            let previous = self.values.insert((*flag, store, t), value);
            let changed = match previous {
                Some(p) => p != value,
                None => value,
            };
            if changed {
                changes.push(FlagChange { target: t, value });
            }
        }
        changes
    }

    fn has_tag(&self, target: &ActorId, tag: &Guid) -> bool {
        self.tags.contains(&(*target, *tag))
    }

    fn set_dialog_tag(&mut self, target: &ActorId, tag: &Guid, value: bool) {
        if value {
            self.tags.insert((*target, *tag));
        } else {
            self.tags.remove(&(*target, *tag));
        }
    }

    fn quest_step_unlocked(&self, flag: &Guid) -> bool {
        self.quest_steps.contains(flag)
    }

    fn unlock_quest_step(&mut self, flag: &Guid, _target: &ActorId) {
        self.quest_steps.insert(*flag);
    }

    fn script_flag(&self, flag: &Guid, _speakers: &[Option<ActorId>]) -> bool {
        self.script_flags.get(flag).copied().unwrap_or(false)
    }

    fn roll(&mut self, request: &RollRequest) -> RollOutcome {
        (self.roller)(request)
    }
}
