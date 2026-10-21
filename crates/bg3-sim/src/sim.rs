//! The headless world: entities, flags, tags, statuses, timers and the event queue.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use bg3_rules::levelmap::LevelMaps;

use crate::character::{CharacterDef, CharacterSheet};
use crate::dialog::{DialogSystem, Notification};
use crate::entity::{Entities, Entity, EntityKind, Origin};
use crate::events::{SimEvent, SimValue};
use crate::flags::{FlagDefs, FlagError, FlagUsage, FlagValues};
use crate::guid::{Guid, osiris_name};
use crate::status::{ActiveStatus, StatusDefs};
use crate::tags::{TagDefs, TagSource};
use crate::timers::{self, Timers};

/// The GUIDSTRING the engine passes for "no object" in `FlagSet`/`FlagCleared`.
pub const NULL_OBJECT: &str = "NULL_00000000-0000-0000-0000-000000000000";

/// A root template, as far as the sim needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateInfo {
    pub id: Guid,
    pub name: String,
    pub kind: EntityKind,
    pub parent: Option<Guid>,
    /// The template's own `Tags`.
    pub tags: Vec<Guid>,
    /// `Race` (characters).
    pub race: Option<Guid>,
    /// `Stats` entry name.
    pub stats: Option<String>,
    /// `StatusList`: statuses applied at spawn.
    pub statuses: Vec<String>,
    /// `Faction`.
    pub faction: Option<Guid>,
    /// `LevelOverride` (characters), if > 0.
    pub level_override: Option<i32>,
}

/// A race (`Public/<Module>/Races/Races.lsx`).
#[derive(Clone, Debug, PartialEq)]
pub struct RaceInfo {
    pub id: Guid,
    pub name: String,
    pub parent: Option<Guid>,
    pub tags: Vec<Guid>,
}

/// A placed object (level or globals), before it becomes an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedObject {
    pub id: Guid,
    pub name: String,
    pub kind: EntityKind,
    pub template: Option<Guid>,
    pub level: Option<String>,
    pub global: bool,
    pub position: [f32; 3],
    /// The instance's own `Tags`.
    pub tags: Vec<Guid>,
    /// `Stats` override.
    pub stats: Option<String>,
    /// `StatusList`.
    pub statuses: Vec<String>,
    /// `Race` override.
    pub race: Option<Guid>,
    /// `Faction` override.
    pub faction: Option<Guid>,
    /// `LevelOverride`, if > 0.
    pub level_override: Option<i32>,
}

/// A weight category (`Public/<Module>/WeightCategories/WeightCategories.lsx`).
#[derive(Clone, Debug, PartialEq)]
pub struct WeightCategory {
    /// Grams.
    pub max_weight: i64,
    pub tags: Vec<Guid>,
}

/// Level names from the modules' `meta.lsx` (`ModuleInfo`), a later module's non-empty value
/// replacing an earlier one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleLevels {
    /// `StartupLevelName`: where a new game goes after character creation.
    pub startup: String,
    /// `CharacterCreationLevelName`.
    pub character_creation: String,
    /// `MenuLevelName`.
    pub menu: String,
    /// `LobbyLevelName`.
    pub lobby: String,
}

impl ModuleLevels {
    /// `IsGameLevel`: not one of the module's system levels (character creation, menu,
    /// lobby). *Inferred*: `SYS_CC_I` → 0 and `TUT_Avernus_C` → 1 in the trace.
    pub fn is_game_level(&self, level: &str) -> bool {
        !level.is_empty()
            && ![&self.character_creation, &self.menu, &self.lobby]
                .iter()
                .any(|l| l.as_str() == level)
    }
}

/// Static game data the sim runs on. Built by [`crate::load`] from an install, or by hand in
/// tests.
#[derive(Clone, Debug, Default)]
pub struct GameData {
    pub flags: FlagDefs,
    pub tags: TagDefs,
    pub statuses: StatusDefs,
    pub templates: HashMap<Guid, TemplateInfo>,
    pub races: HashMap<Guid, RaceInfo>,
    /// Level names from the module metadata.
    pub module_levels: ModuleLevels,
    /// Faction names (`Public/<Module>/Factions/Factions.lsx`).
    pub factions: HashMap<Guid, String>,
    /// Ascending by `max_weight`.
    pub weight_categories: Vec<WeightCategory>,
    /// `Weight` (kg) of every stats entry that has one.
    pub stats_weight: HashMap<String, f32>,
    /// Stats `type` of every stats entry (`Weapon`, `Armor`, `Character`, `StatusData`, …).
    pub stats_type: HashMap<String, String>,
    /// Objects from every level's `Globals` directory: they exist for the whole game.
    pub globals: Vec<PlacedObject>,
    /// Per-level objects (`Levels/<Level>/{Characters,Items,Triggers}`), by level name.
    pub levels: HashMap<String, Vec<PlacedObject>>,
    /// `Character` stats entries, by name.
    pub characters: HashMap<String, CharacterDef>,
    /// Level maps (`ProficiencyBonusScaling`, `LevelMapValue(…)`).
    pub level_maps: LevelMaps,
    /// `DifficultyClasses`: the DC of each difficulty class (dialog rolls refer to them).
    pub difficulty_classes: HashMap<Guid, i32>,
    /// What combat needs (factions, resources, weapons, spells, …).
    pub combat: crate::combat::CombatData,
}

impl GameData {
    /// The `ParentTemplateId` chain of a template, the template first.
    pub fn template_chain(&self, template: Option<Guid>) -> Vec<&TemplateInfo> {
        let mut out: Vec<&TemplateInfo> = Vec::new();
        let mut cur = template;
        while let Some(t) = cur.and_then(|id| self.templates.get(&id)) {
            if out.iter().any(|o| o.id == t.id) || out.len() >= 64 {
                break;
            }
            out.push(t);
            cur = t.parent;
        }
        out
    }

    /// The tags an object starts with, in the order the engine announces them at spawn:
    /// the race's tags (the race, then its parent races), then the tags of every template in
    /// the `ParentTemplateId` chain from the root ancestor down, then the instance's own.
    ///
    /// *Inferred from the trace* (the `TagSet` events the engine raises for the characters of
    /// the opening), not read in the binary.
    pub fn initial_tags(&self, obj: &PlacedObject) -> Vec<Guid> {
        let mut out: Vec<Guid> = Vec::new();
        let mut push = |t: Guid| {
            if !out.contains(&t) {
                out.push(t);
            }
        };
        let chain = self.template_chain(obj.template);
        let race = obj.race.or_else(|| chain.iter().find_map(|t| t.race));
        let mut r = race;
        let mut depth = 0;
        while let Some(info) = r.and_then(|id| self.races.get(&id)) {
            info.tags.iter().copied().for_each(&mut push);
            r = info.parent;
            depth += 1;
            if depth > 16 {
                break;
            }
        }
        for t in chain.iter().rev() {
            t.tags.iter().copied().for_each(&mut push);
        }
        obj.tags.iter().copied().for_each(&mut push);
        out
    }

    /// The stats entry of an object: its own `Stats`, else the nearest template's.
    pub fn stats_name<'a>(&'a self, obj: &'a PlacedObject) -> Option<&'a str> {
        obj.stats.as_deref().or_else(|| {
            self.template_chain(obj.template)
                .into_iter()
                .find_map(|t| t.stats.as_deref())
        })
    }

    /// `LevelOverride` of an object: its own, else the nearest template's.
    pub fn level_override(&self, obj: &PlacedObject) -> Option<i32> {
        obj.level_override.or_else(|| {
            self.template_chain(obj.template)
                .into_iter()
                .find_map(|t| t.level_override)
        })
    }

    /// The rule stats a character starts with, if its stats entry is a `Character` entry.
    pub fn character_sheet(&self, obj: &PlacedObject) -> Option<CharacterSheet> {
        if obj.kind != EntityKind::Character {
            return None;
        }
        let def = self.characters.get(self.stats_name(obj)?)?;
        Some(def.sheet(self.level_override(obj), &self.level_maps))
    }

    /// The faction of an object: its own `Faction`, else the nearest template's.
    pub fn faction(&self, obj: &PlacedObject) -> Option<Guid> {
        obj.faction.or_else(|| {
            self.template_chain(obj.template)
                .into_iter()
                .find_map(|t| t.faction)
        })
    }

    /// The weight-category tags of an object: the first category (ascending `MaxWeight`) that
    /// holds the stats `Weight` (kg → g).
    ///
    /// *Inferred from the trace* (`WEIGHT_*` `TagSet` events after spawn); the engine side is
    /// `eoc::WeightCategoryBoostComponent` / `WeightCategories.cpp`, not read in detail.
    pub fn weight_tags(&self, obj: &PlacedObject) -> Vec<Guid> {
        let Some(w) = self.stats_name(obj).and_then(|s| self.stats_weight.get(s)) else {
            return Vec::new();
        };
        let grams = (f64::from(*w) * 1000.0).round() as i64;
        self.weight_categories
            .iter()
            .find(|c| grams <= c.max_weight)
            .map_or_else(Vec::new, |c| c.tags.clone())
    }

    /// Statuses an object starts with: its template chain's `StatusList` (root ancestor
    /// first), then its own.
    ///
    /// *Inferred from the trace* (`StatusAttempt`/`StatusApplied` with story action id 0
    /// after spawn).
    pub fn initial_statuses(&self, obj: &PlacedObject) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let chain = self.template_chain(obj.template);
        for s in chain
            .iter()
            .rev()
            .flat_map(|t| t.statuses.iter())
            .chain(obj.statuses.iter())
        {
            if !out.contains(s) {
                out.push(s.clone());
            }
        }
        out
    }
}

/// The world.
///
/// Cloning is cheap enough to explore alternatives (the trace harness clones it to search a
/// dialog's choices): the static [`GameData`] is shared.
#[derive(Clone, Debug, Default)]
pub struct Sim {
    pub data: Arc<GameData>,
    pub entities: Entities,
    pub flags: FlagValues,
    pub timers: Timers,
    /// Seconds since the start.
    now: f64,
    events: VecDeque<SimEvent>,
    /// Entities whose tag sources changed since the last [`Sim::update_tags`].
    dirty_tags: Vec<Guid>,
    /// Loaded level (its non-global objects exist).
    pub level: Option<String>,
    /// Characters in the party, in join order.
    pub party: Vec<Guid>,
    /// Level requested by the story, loaded once the current one unloads.
    pub pending_level: Option<String>,
    /// Running dialogs (see [`crate::dialog`]).
    pub dialogs: DialogSystem,
    /// The dice: rolls made by the rules and dialogs.
    pub dice: crate::rules::SimDice,
    /// Quest steps unlocked by dialogs (`Quest` set-flags). The journal is not modelled.
    pub quest_steps: HashSet<Guid>,
    /// What happened, for the presentation layer (lines to show, choices to offer, …).
    pub(crate) notifications: Vec<Notification>,
    /// Presentation state for a live front end (see [`crate::presentation`]).
    pub stage: crate::presentation::Stage,
    /// Fights and combatants (see [`crate::combat`]).
    pub combat: crate::combat::CombatState,
}

/// An argument of an observed engine event: the declared Osiris type name of the parameter
/// (`CHARACTER`, `GUIDSTRING`, `STRING`, …) and the value as printed (strings without quotes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedArg {
    pub ty: String,
    pub text: String,
}

impl Sim {
    pub fn new(data: GameData) -> Self {
        Self::with_data(Arc::new(data))
    }

    /// A sim over shared game data (several sims can run on one copy).
    pub fn with_data(data: Arc<GameData>) -> Self {
        let mut sim = Self {
            data: data.clone(),
            ..Self::default()
        };
        for o in &data.globals {
            sim.spawn(o, Origin::Data);
        }
        // The template tags are merged (and announced with `TagSet`) at the first
        // `update_tags`, like the engine does after `GameModeStarted`.
        sim
    }

    pub fn now(&self) -> f64 {
        self.now
    }

    // ---- entities ------------------------------------------------------------------------

    fn spawn(&mut self, o: &PlacedObject, origin: Origin) {
        let tags = self.data.initial_tags(o);
        let mut e = Entity::new(o.id, &o.name, o.kind, origin);
        e.template = o.template;
        e.level = o.level.clone();
        e.global = o.global;
        e.position = o.position;
        e.stats = self.data.stats_name(o).map(str::to_owned);
        e.faction = self.data.faction(o);
        e.sheet = self.data.character_sheet(o);
        for t in tags {
            e.tags.add(TagSource::Template, t);
        }
        if matches!(o.kind, EntityKind::Character | EntityKind::Item) {
            for t in self.data.weight_tags(o) {
                e.tags.add(TagSource::Engine, t);
            }
        }
        let statuses = self.data.initial_statuses(o);
        let who = e.osiris_name();
        for s in &statuses {
            e.statuses.push(ActiveStatus {
                id: s.clone(),
                source: Guid::NULL,
                remaining: -1.0,
            });
        }
        self.entities.insert(e);
        self.dirty_tags.push(o.id);
        for s in statuses {
            let args = vec![
                SimValue::Guid(who.clone()),
                SimValue::Str(s),
                SimValue::Guid(Guid::NULL.to_string()),
                SimValue::Int(0),
            ];
            self.push_event("StatusAttempt", args.clone());
            self.push_event("StatusApplied", args);
        }
    }

    /// Make a level's own objects exist (globals always do). Their template tags are merged
    /// at the next [`Sim::update_tags`], which raises `TagSet` for them.
    pub fn load_level(&mut self, level: &str) {
        self.level = Some(level.to_owned());
        let data = self.data.clone();
        for o in data.levels.get(level).into_iter().flatten() {
            if !self.entities.contains(o.id) {
                self.spawn(o, Origin::Data);
            }
        }
    }

    /// Drop a level's own objects (not globals, not runtime-created ones).
    pub fn unload_level(&mut self, level: &str) {
        self.entities.remove_where(|e| {
            !e.global && e.origin == Origin::Data && e.level.as_deref() == Some(level)
        });
        if self.level.as_deref() == Some(level) {
            self.level = None;
        }
    }

    /// Learn about an object the engine created (loot, the created avatar, …) from an event
    /// that names it. `text` is the GUIDSTRING as the engine printed it (`Name_<uuid>`).
    pub fn observe_entity(&mut self, text: &str, kind: EntityKind) -> Option<Guid> {
        let id = Guid::from_osiris(text).filter(|g| !g.is_null())?;
        match self.entities.get_mut(id) {
            Some(e) => {
                if e.kind == EntityKind::Unknown && kind != EntityKind::Unknown {
                    e.kind = kind;
                }
            }
            None => {
                let name = text
                    .len()
                    .checked_sub(37)
                    .filter(|&n| text.as_bytes().get(n) == Some(&b'_'))
                    .map_or("", |n| &text[..n]);
                self.entities
                    .insert(Entity::new(id, name, kind, Origin::Observed));
            }
        }
        Some(id)
    }

    pub fn entity(&self, id: Guid) -> Option<&Entity> {
        self.entities.get(id)
    }

    /// How the engine renders an object for the story: `Name_<uuid>` if known, else the bare
    /// UUID, `NULL_…` for the null id.
    pub fn render(&self, id: Guid) -> String {
        if id.is_null() {
            return NULL_OBJECT.to_owned();
        }
        match self.entities.get(id) {
            Some(e) => e.osiris_name(),
            None => id.to_string(),
        }
    }

    /// `GetRegion`: the level an object is in; an item in an inventory is where its holder
    /// is. `None` if the sim does not know.
    pub fn region(&self, id: Guid) -> Option<&str> {
        let mut cur = self.entities.get(id)?;
        for _ in 0..16 {
            match cur.holder.and_then(|h| self.entities.get(h)) {
                Some(h) => cur = h,
                None => break,
            }
        }
        cur.level.as_deref()
    }

    /// `Exists`: any entity with that UUID.
    // bg3: esv::OsirisGameFunctions::Exists (4.1.1.7631656)
    pub fn exists(&self, id: Guid) -> bool {
        self.entities.contains(id)
    }

    // ---- engine events the sim does not produce itself -----------------------------------

    /// Take note of an event the engine raised that the sim cannot produce yet (it was fed to
    /// the story from outside: a trace, the player, a system not written yet), so the sim's
    /// state follows: entities named in it exist, tags/statuses/flags it reports are set,
    /// party and level changes happen.
    pub fn observe(&mut self, name: &str, args: &[ObservedArg]) {
        for a in args {
            let kind = EntityKind::from_osiris_type(&a.ty);
            if kind != EntityKind::Unknown {
                self.observe_entity(&a.text, kind);
            }
        }
        let text = |i: usize| args.get(i).map_or("", |a| a.text.as_str());
        let id = |i: usize| Guid::from_osiris(text(i)).unwrap_or(Guid::NULL);
        if name.contains("Dialog") {
            let texts: Vec<&str> = args.iter().map(|a| a.text.as_str()).collect();
            self.observe_dialog_event(name, &texts);
        }
        match (name, args.len()) {
            ("TagSet", 2) => {
                if let Some(e) = self.observe_entity(text(0), EntityKind::Unknown)
                    && let Some(e) = self.entities.get_mut(e)
                {
                    e.tags.add(TagSource::Engine, id(1));
                    e.tags.force_merged(id(1), true);
                }
            }
            ("TagCleared", 2) => {
                if let Some(e) = self.entities.get_mut(id(0)) {
                    e.tags.remove(TagSource::Engine, id(1));
                    e.tags.remove(TagSource::Template, id(1));
                    e.tags.force_merged(id(1), false);
                }
            }
            ("StatusApplied", 4) => {
                if let Some(e) = self.observe_entity(text(0), EntityKind::Unknown)
                    && let Some(e) = self.entities.get_mut(e)
                    && !e.statuses.iter().any(|s| s.id == text(1))
                {
                    e.statuses.push(ActiveStatus {
                        id: text(1).to_owned(),
                        source: id(2),
                        remaining: -1.0,
                    });
                }
            }
            ("StatusRemoved", 4) => {
                if let Some(e) = self.entities.get_mut(id(0)) {
                    e.statuses.retain(|s| s.id != text(1));
                }
            }
            ("FlagSet" | "FlagCleared", 3) => {
                let flag = id(0);
                let holder = match self.data.flags.get(flag).map(|d| d.usage) {
                    Some(FlagUsage::Global) => Guid::NULL,
                    _ => id(1),
                };
                self.flags.put(holder, flag, name == "FlagSet");
            }
            // (template, object, holder, add type) / (object, template, level): the template
            // tells what the object is.
            ("TemplateAddedTo", 4) => self.observe_templated(text(1), id(0)),
            ("EnteredLevel", 3) => {
                self.observe_templated(text(0), id(1));
                if let Some(e) = self.entities.get_mut(id(0)) {
                    e.level = Some(text(2).to_owned());
                }
            }
            ("AddedTo", 3) => {
                self.observe_entity(text(0), EntityKind::Item);
                let holder = self.observe_entity(text(1), EntityKind::Unknown);
                if let Some(e) = self.entities.get_mut(id(0)) {
                    e.holder = holder;
                }
            }
            ("HitpointsChanged", 2) => {
                // Hit points and death are not modelled: the engine's state moves before
                // its event arrives (`GetHitpointsPercentage` reports 0 and `IsDead` 1 before
                // `HitpointsChanged`/`Died` are delivered), so mirroring events would lag.
                self.observe_entity(text(0), EntityKind::Unknown);
            }
            ("CharacterReservedUserIDChanged", 3) => {
                if let Some(e) = self.entities.get_mut(id(0)) {
                    e.user = text(2).parse().ok();
                }
            }
            ("CharacterJoinedParty", 1) => {
                let c = id(0);
                if !c.is_null() && !self.party.contains(&c) {
                    self.party.push(c);
                }
            }
            ("CharacterLeftParty", 1) => {
                let c = id(0);
                self.party.retain(|p| *p != c);
            }
            ("LevelUnloading", 1) => {
                self.unload_level(text(0));
                if let Some(next) = self.pending_level.take() {
                    self.load_level(&next);
                }
            }
            ("LevelLoaded", 1) if self.level.as_deref() != Some(text(0)) => {
                self.load_level(text(0));
            }
            _ => {}
        }
    }

    /// An object named by an event together with its root template. An object the sim did
    /// not create gets its template's tags (announced with `TagSet` at the next merge, like
    /// a spawn).
    fn observe_templated(&mut self, text: &str, template: Guid) {
        let kind = self
            .data
            .templates
            .get(&template)
            .map_or(EntityKind::Unknown, |t| t.kind);
        let Some(id) = self.observe_entity(text, kind) else {
            return;
        };
        let tags = {
            let Some(e) = self.entities.get(id) else {
                return;
            };
            if e.template.is_some() || template.is_null() {
                return;
            }
            let probe = PlacedObject {
                id,
                name: e.name.clone(),
                kind: e.kind,
                template: Some(template),
                level: None,
                global: false,
                position: [0.0; 3],
                tags: Vec::new(),
                stats: None,
                statuses: Vec::new(),
                race: None,
                faction: None,
                level_override: None,
            };
            let sheet = self.data.character_sheet(&probe);
            (e.origin == Origin::Observed).then(|| {
                let mut t = self.data.initial_tags(&probe);
                t.extend(self.data.weight_tags(&probe));
                (t, sheet)
            })
        };
        let chain = self.data.template_chain(Some(template));
        let stats = chain.iter().find_map(|t| t.stats.clone());
        let faction = chain.iter().find_map(|t| t.faction);
        if let Some(e) = self.entities.get_mut(id) {
            e.template = Some(template);
            if e.stats.is_none() {
                e.stats = stats;
            }
            if e.faction.is_none() {
                e.faction = faction;
            }
            if let Some((tags, sheet)) = tags {
                if e.sheet.is_none() {
                    e.sheet = sheet;
                }
                for t in tags {
                    e.tags.add(TagSource::Template, t);
                }
            }

            self.dirty_tags.push(id);
        }
    }

    /// `TeleportPartiesToLevelWithMovie(level, …)`: the level is loaded when the current one
    /// has unloaded.
    pub fn request_level(&mut self, level: &str) -> bool {
        if level.is_empty() {
            return false;
        }
        self.pending_level = Some(level.to_owned());
        true
    }

    // ---- events --------------------------------------------------------------------------

    pub fn push_event(&mut self, name: &str, args: Vec<SimValue>) {
        self.events.push_back(SimEvent::new(name, args));
    }

    /// Run the end-of-frame systems (tag merge) and hand out the queued events, oldest first.
    pub fn take_events(&mut self) -> Vec<SimEvent> {
        self.update_tags();
        self.events.drain(..).collect()
    }

    pub fn pending_events(&self) -> impl Iterator<Item = &SimEvent> {
        self.events.iter()
    }

    /// Advance the clock: fire due timers (queued as events) and merge tags.
    pub fn advance(&mut self, dt: f64) {
        self.now += dt.max(0.0);
        self.fire_timers();
        self.update_tags();
    }

    /// One frame of the live game: requested dialogs start, then [`Sim::advance`].
    pub fn tick(&mut self, dt: f64) {
        self.update_dialogs();
        self.advance(dt);
    }

    // ---- flags ---------------------------------------------------------------------------

    /// The flag holders `scr::SetFlag` would write for `(flag, object)`.
    // bg3: scr::SetFlag, scr::GetFlag (4.1.1.7631656)
    pub(crate) fn flag_holders(
        &self,
        flag: Guid,
        object: Guid,
    ) -> Result<(FlagUsage, Vec<Guid>), FlagError> {
        if flag.is_null() {
            return Err(FlagError::UnknownFlag);
        }
        let def = self.data.flags.get(flag).ok_or(FlagError::UnknownFlag)?;
        let usage = def.usage;
        match usage {
            FlagUsage::Unscriptable(_) => Err(FlagError::Unscriptable),
            FlagUsage::Other(_) => Err(FlagError::BadUsage),
            FlagUsage::Global => Ok((usage, vec![Guid::NULL])),
            _ => {
                let e = self
                    .entities
                    .get(object)
                    .filter(|_| !object.is_null())
                    .ok_or(FlagError::NoObject)?;
                match usage {
                    FlagUsage::User | FlagUsage::UserShared | FlagUsage::Party => {
                        if e.kind != EntityKind::Character {
                            return Err(FlagError::NotCharacter);
                        }
                        Ok((usage, self.flag_group(e, usage)))
                    }
                    _ => {
                        if !matches!(e.kind, EntityKind::Character | EntityKind::Item) {
                            return Err(FlagError::NotGameObject);
                        }
                        Ok((usage, vec![object]))
                    }
                }
            }
        }
    }

    /// User flags are copied onto every character with the same user id
    /// (`OsirisVariableHelper::SetUserFlag` walks the party); party flags onto every party
    /// member. Without that information, the character alone.
    fn flag_group(&self, e: &Entity, usage: FlagUsage) -> Vec<Guid> {
        let mut out = vec![e.id];
        let in_party = |id: Guid| self.party.contains(&id);
        match usage {
            FlagUsage::Party if in_party(e.id) => {
                out.extend(self.party.iter().copied().filter(|&p| p != e.id));
            }
            FlagUsage::User | FlagUsage::UserShared => {
                if let Some(u) = e.user {
                    out.extend(
                        self.entities
                            .iter()
                            .filter(|o| o.id != e.id && o.user == Some(u) && in_party(o.id))
                            .map(|o| o.id),
                    );
                }
            }
            _ => {}
        }
        out
    }

    /// `SetFlag` / `ClearFlag`. A `FlagSet` / `FlagCleared` event is queued for every holder
    /// whose value changed, if `notify` (the calls' last argument; 1 in every call of the
    /// opening). Setting a flag that is already set raises nothing (trace: three
    /// `SetFlag(GLO_DifficultyMode_Medium, NULL, 0, 1)` give one `FlagSet`).
    // bg3: esv::OsirisDialogFunctions::SetFlag / ClearFlag → scr::SetFlag (4.1.1.7631656)
    pub fn set_flag(
        &mut self,
        flag: Guid,
        object: Guid,
        value: bool,
        dialog_instance: i32,
        notify: bool,
    ) -> Result<(), FlagError> {
        let (_, holders) = self.flag_holders(flag, object)?;
        for h in holders {
            if self.flags.put(h, flag, value) && notify {
                self.push_flag_event(flag, h, value, dialog_instance);
            }
        }
        Ok(())
    }

    /// Queue `FlagSet` / `FlagCleared(flag, holder, dialog instance)`.
    pub(crate) fn push_flag_event(
        &mut self,
        flag: Guid,
        holder: Guid,
        value: bool,
        dialog_instance: i32,
    ) {
        let name = if value { "FlagSet" } else { "FlagCleared" };
        let args = vec![
            SimValue::Guid(self.flag_text(flag)),
            SimValue::Guid(self.render(holder)),
            SimValue::Int(dialog_instance),
        ];
        self.push_event(name, args);
    }

    /// `GetFlag`.
    // bg3: esv::OsirisDialogFunctions::GetFlag → scr::GetFlag (4.1.1.7631656)
    pub fn get_flag(&self, flag: Guid, object: Guid) -> Result<bool, FlagError> {
        let (_, holders) = self.flag_holders(flag, object)?;
        Ok(self.flags.get(holders[0], flag))
    }

    /// `Name_<uuid>` of a flag (bare UUID if unknown).
    pub fn flag_text(&self, flag: Guid) -> String {
        match self.data.flags.get(flag) {
            Some(d) => osiris_name(&d.name, flag),
            None => flag.to_string(),
        }
    }

    // ---- tags ----------------------------------------------------------------------------

    /// `SetTag`: characters and items only; false (call failed) otherwise.
    // bg3: esv::OsirisGameFunctions::SetTag (4.1.1.7631656)
    pub fn set_tag(&mut self, object: Guid, tag: Guid) -> bool {
        let Some(e) = self.entities.get_mut(object) else {
            return false;
        };
        if !matches!(e.kind, EntityKind::Character | EntityKind::Item) || tag.is_null() {
            return false;
        }
        if e.tags.add(TagSource::Osiris, tag) {
            self.dirty_tags.push(object);
        }
        true
    }

    /// `ClearTag`.
    // bg3: esv::OsirisGameFunctions::ClearTag (4.1.1.7631656)
    pub fn clear_tag(&mut self, object: Guid, tag: Guid) -> bool {
        let Some(e) = self.entities.get_mut(object) else {
            return false;
        };
        if !matches!(e.kind, EntityKind::Character | EntityKind::Item) || tag.is_null() {
            return false;
        }
        if e.tags.remove(TagSource::Osiris, tag) {
            self.dirty_tags.push(object);
        }
        true
    }

    /// Mark an object's tags for the next merge.
    pub(crate) fn mark_tags_dirty(&mut self, object: Guid) {
        self.dirty_tags.push(object);
    }

    /// `IsTagged`.
    pub fn is_tagged(&self, object: Guid, tag: Guid) -> bool {
        self.entities
            .get(object)
            .is_some_and(|e| e.tags.is_tagged(tag))
    }

    /// The tag merge (`esv::TagSystem::Update`): queue `TagSet` / `TagCleared` for every
    /// change, entity by entity in the order they became dirty.
    pub fn update_tags(&mut self) {
        let dirty = std::mem::take(&mut self.dirty_tags);
        let mut seen: Vec<Guid> = Vec::new();
        for id in dirty {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            let Some(e) = self.entities.get_mut(id) else {
                continue;
            };
            let (added, removed) = e.tags.merge();
            let who = e.osiris_name();
            for t in added {
                self.push_event(
                    "TagSet",
                    vec![SimValue::Guid(who.clone()), SimValue::Guid(t.to_string())],
                );
            }
            for t in removed {
                self.push_event(
                    "TagCleared",
                    vec![SimValue::Guid(who.clone()), SimValue::Guid(t.to_string())],
                );
            }
        }
    }

    // ---- factions ------------------------------------------------------------------------

    /// `SetFaction(object, faction)`: queue `BaseFactionChanged(character, old, new)` (bare
    /// faction UUIDs) if the faction changes. False if the object does not exist.
    ///
    /// *Verified by trace*: every `SetFaction` of the opening is followed by one
    /// `BaseFactionChanged` whose old faction is the object's `Faction` from the level data.
    // bg3: esv::OsirisGameFunctions::SetFaction (4.1.1.7631656)
    pub fn set_faction(&mut self, object: Guid, faction: Guid) -> bool {
        let Some(e) = self.entities.get_mut(object) else {
            return false;
        };
        let old = e.faction;
        if old == Some(faction) || faction.is_null() {
            return true;
        }
        e.faction = Some(faction);
        if e.kind == EntityKind::Character {
            let who = e.osiris_name();
            self.push_event(
                "BaseFactionChanged",
                vec![
                    SimValue::Guid(who),
                    SimValue::Guid(old.unwrap_or(Guid::NULL).to_string()),
                    SimValue::Guid(faction.to_string()),
                ],
            );
        }
        true
    }

    /// `GetFaction`: `Name_<uuid>` of the object's faction; `None` if unknown.
    pub fn faction_text(&self, object: Guid) -> Option<String> {
        let f = self.entities.get(object)?.faction?;
        Some(osiris_name(self.data.factions.get(&f)?, f))
    }

    // ---- statuses ------------------------------------------------------------------------

    /// `ApplyStatus(object, status, duration, force, source)`: make the status active and
    /// queue `StatusAttempt` + `StatusApplied`. False if the object does not exist. The last
    /// event argument is the story action id: -1 for statuses the story applies (trace: every
    /// status from `ApplyStatus` reports -1, statuses the engine applies at spawn report 0).
    pub fn apply_status(
        &mut self,
        object: Guid,
        status: &str,
        duration: f32,
        source: Guid,
    ) -> bool {
        self.apply_status_with(object, status, duration, source, -1)
    }

    /// [`Sim::apply_status`] with the story action id the events report.
    pub fn apply_status_with(
        &mut self,
        object: Guid,
        status: &str,
        duration: f32,
        source: Guid,
        story_action: i32,
    ) -> bool {
        let Some(e) = self.entities.get_mut(object) else {
            return false;
        };
        match e.statuses.iter_mut().find(|s| s.id == status) {
            Some(s) => s.remaining = duration,
            None => e.statuses.push(ActiveStatus {
                id: status.to_owned(),
                source,
                remaining: duration,
            }),
        }
        let who = e.osiris_name();
        let args = vec![
            SimValue::Guid(who),
            SimValue::Str(status.to_owned()),
            SimValue::Guid(source.to_string()),
            SimValue::Int(story_action),
        ];
        self.push_event("StatusAttempt", args.clone());

        self.push_event("StatusApplied", args);
        true
    }

    /// `RemoveStatus(object, status, source)`.
    pub fn remove_status(&mut self, object: Guid, status: &str) -> bool {
        let Some(e) = self.entities.get_mut(object) else {
            return false;
        };
        let Some(i) = e.statuses.iter().position(|s| s.id == status) else {
            return true;
        };
        let s = e.statuses.remove(i);
        let who = e.osiris_name();
        self.push_event(
            "StatusRemoved",
            vec![
                SimValue::Guid(who),
                SimValue::Str(status.to_owned()),
                SimValue::Guid(s.source.to_string()),
                SimValue::Int(-1),
            ],
        );
        true
    }

    /// `HasActiveStatus`.
    pub fn has_active_status(&self, object: Guid, status: &str) -> bool {
        self.entities
            .get(object)
            .is_some_and(|e| e.statuses.iter().any(|s| s.id == status))
    }

    /// `HasActiveStatusWithGroup` / `HasAppliedStatusWithGroup`: some active status of the
    /// object is in the group. `None` if the object has a status the sim has no definition
    /// for (engine statuses like `DYING`), or the object is unknown.
    pub fn has_status_in_group(&self, object: Guid, group: &str) -> Option<bool> {
        let e = self.entities.get(object)?;
        let mut found = false;
        for s in &e.statuses {
            let def = self.data.statuses.get(&s.id)?;
            found |= def.groups.iter().any(|g| g == group);
        }
        Some(found)
    }

    /// `IsStatusFromGroup(status, group)`: the group is in the status' `StatusGroups`.
    pub fn is_status_from_group(&self, status: &str, group: &str) -> bool {
        self.data
            .statuses
            .get(status)
            .is_some_and(|d| d.groups.iter().any(|g| g == group))
    }

    // ---- timers --------------------------------------------------------------------------

    /// `TimerLaunch(name, ms)`.
    // bg3: esv::OsirisGameFunctions::TimerLaunch (4.1.1.7631656)
    pub fn timer_launch(&mut self, name: &str, ms: i32) -> bool {
        if name.is_empty() {
            return false;
        }
        self.timers
            .start(self.now, name, Guid::NULL, ms, timers::flags::STORY);
        true
    }

    /// `TimerCancel(name)`.
    pub fn timer_cancel(&mut self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        self.timers.stop(name, Guid::NULL);
        true
    }

    /// `ObjectTimerLaunch(object, name, ms, story_flag)` (and `RealtimeObjectTimerLaunch`
    /// with `realtime`). False (call failed) if the object does not exist or the name is
    /// empty.
    // bg3: esv::OsirisGameFunctions::ObjectTimerLaunch / RealtimeObjectTimerLaunch (4.1.1.7631656)
    pub fn object_timer_launch(
        &mut self,
        object: Guid,
        name: &str,
        ms: i32,
        story_flag: bool,
        realtime: bool,
    ) -> bool {
        if !self.exists(object) || name.is_empty() {
            return false;
        }
        let mut flags = timers::flags::OBJECT;
        if realtime {
            flags |= timers::flags::REALTIME;
        } else if story_flag {
            flags |= timers::flags::STORY;
        }
        self.timers.start(self.now, name, object, ms, flags);
        true
    }

    /// `ObjectTimerCancel` / `RealtimeObjectTimerCancel`.
    pub fn object_timer_cancel(&mut self, object: Guid, name: &str) -> bool {
        if !self.exists(object) || name.is_empty() {
            return false;
        }
        self.timers.stop(name, object);
        true
    }

    fn fire_timers(&mut self) {
        for t in self.timers.take_due(self.now) {
            self.queue_timer_finished(&t.name, t.object);
        }
    }

    fn queue_timer_finished(&mut self, name: &str, object: Guid) {
        if object.is_null() {
            self.push_event("TimerFinished", vec![SimValue::Str(name.to_owned())]);
        } else {
            let who = self.render(object);
            self.push_event(
                "ObjectTimerFinished",
                vec![SimValue::Guid(who), SimValue::Str(name.to_owned())],
            );
        }
    }

    /// Fire one running timer now, whatever its due time; false if it is not running.
    ///
    /// For trace alignment: a reference trace has no timestamps, so the replay fires a timer
    /// when the game did, after checking that the sim has it running.
    pub fn force_timer(&mut self, name: &str, object: Guid) -> bool {
        if !self.timers.stop(name, object) {
            return false;
        }
        self.queue_timer_finished(name, object);
        true
    }
}
