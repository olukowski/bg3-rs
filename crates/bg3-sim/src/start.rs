//! Starting a new game on the sim the way the engine starts the campaign, up to the player
//! waking up on the Nautiloid (`TUT_Avernus_C`).
//!
//! The order is the runtime trace's (`docs/research/osiris-runtime-log.md`, "Startup
//! sequence"; `docs/research/character-creation.md`): `InitGame`,
//! `GameModeStarted("Campaign")`, the character-creation level (`LevelLoaded`,
//! `CharacterCreationStarted`, `LevelGameplayStarted`, `LevelGameplayReady`, the intro movie),
//! then the created avatar and the guardian enter the level and become the player's
//! (`CharacterMadePlayer`, `CharacterReservedUserIDChanged`, `GainedControl`,
//! `CharacterSelectedForUser`, `CompanionSelectedForUser`, `CharacterJoinedParty`),
//! `CharacterCreationFinished`, the character-creation result goes on the avatar
//! ([`crate::cc`]), and the story's `TeleportPartiesToLevelWithMovie` takes the party to the
//! startup level.
//!
//! The avatar is made from a [`CharacterChoice`] (default: the trace's player, a male deep
//! gnome cleric of Mystra with the Guild Artisan background, see [`trace_choice`]) with a
//! fixed UUID (the engine makes a random one). The guardian is the trace's.

use crate::cc::{CcData, CcError, CharacterChoice, CreatedCharacter};
use crate::driver::Game;
use crate::entity::EntityKind;
use crate::events::{SimEvent, SimValue};
use crate::guid::Guid;
use crate::presentation::TriggerDef;
use crate::sim::NULL_OBJECT;

/// The root template of the trace's avatar (`CharacterCreationPresets`: deep gnome, male).
pub const TRACE_AVATAR_TEMPLATE: &str = "f660278d-6b97-4bfd-850d-66d40fd89d78";
/// The root template of the trace's guardian (`CompanionSelectedForUser`).
pub const TRACE_GUARDIAN_TEMPLATE: &str = "c40a12ef-20f6-4c49-b287-8dc85eb75689";
/// The trace's avatar (`full-nautiloid.log`): race `Gnome`, subrace `Gnome_Deep` (its body
/// preset's template and the `GNOME`/`DEEPGNOME` tags), class `Cleric` (`CLERIC`; the
/// `EQP_CC_Cleric` items in its `AddedTo` events), background Guild Artisan
/// (`GUILD_ARTISAN`), deity Mystra (`MYSTRA`, `ALIGN_GOOD`). Subclass, scores and picks are
/// not in the trace; [`trace_choice`] leaves them to the data's defaults.
pub const TRACE_RACE: &str = "f1b3f884-4029-4f0f-b158-1f9fe0ae5a0d";
pub const TRACE_SUBRACE: &str = "3560f4a2-c0b8-4f8b-baf8-6b6eaef0c160";
pub const TRACE_AVATAR_CLASS: &str = "114e7aee-d1d4-4371-8d90-8a2080592faf";
pub const TRACE_BACKGROUND: &str = "e9aeac40-6c4e-4d81-9a24-e9eeb6208e85";
pub const TRACE_DEITY: &str = "3e1c9bfc-f15c-46c1-8c2d-725da5e4d896";
/// Placeholder UUIDs of the created avatar and guardian (the engine makes random ones).
pub const AVATAR_ID: &str = "a7a7a7a7-0000-4000-8000-000000000001";
pub const GUARDIAN_ID: &str = "a7a7a7a7-0000-4000-8000-000000000002";
/// The first local user (`CharacterReservedUserIDChanged(…, -65536, 65537)` in the trace).
pub const USER_ID: i32 = 65537;
const NO_USER: i32 = -65536;

fn uuid(s: &str) -> Guid {
    Guid::parse(s).expect("uuid")
}

/// The trace's character choices; the rest is left open for [`CcData::complete`] (the first
/// subclass, the class's ability preset, the `DefaultValues` picks).
pub fn trace_choice() -> CharacterChoice {
    CharacterChoice {
        race: uuid(TRACE_RACE),
        subrace: Some(uuid(TRACE_SUBRACE)),
        body_type: 0,
        body_shape: 0,
        class: uuid(TRACE_AVATAR_CLASS),
        background: uuid(TRACE_BACKGROUND),
        deity: Some(uuid(TRACE_DEITY)),
        ..CharacterChoice::default()
    }
}

/// How to start.
#[derive(Clone, Debug)]
pub struct NewGame {
    /// The player's character; `None`: [`trace_choice`]. Open choices are filled with the
    /// defaults ([`CcData::complete`]).
    pub character: Option<CharacterChoice>,
    pub guardian_template: Guid,
    /// Triggers of the startup level ([`crate::presentation::load_triggers`]); the party is
    /// placed at its start trigger (`TriggerType` `TriggerStart`).
    pub startup_triggers: Vec<TriggerDef>,
}

impl Default for NewGame {
    fn default() -> Self {
        Self {
            character: None,
            guardian_template: uuid(TRACE_GUARDIAN_TEMPLATE),
            startup_triggers: Vec::new(),
        }
    }
}

impl NewGame {
    /// The avatar this start makes (its choice completed and built), or why it can't.
    pub fn create(&self, cc: &CcData) -> Result<CreatedCharacter, Vec<CcError>> {
        let mut choice = self.character.clone().unwrap_or_else(trace_choice);
        cc.complete(&mut choice);
        cc.build(&choice)
    }
}

/// What [`new_game`] did.
#[derive(Clone, Debug)]
pub struct Started {
    pub avatar: Guid,
    /// The avatar's root template (its body preset's).
    pub avatar_template: Guid,
    pub level: String,
    /// The story asked for the startup level itself (else [`new_game`] did).
    pub story_requested_level: bool,
    /// Events delivered to the story.
    pub events: usize,
}

fn s(v: &str) -> SimValue {
    SimValue::Str(v.to_owned())
}

fn g(v: String) -> SimValue {
    SimValue::Guid(v)
}

/// Start a new campaign on `game` (fresh: story not initialised, sim with its globals only;
/// set a dialog bank first for the dialogs to run). An invalid character choice falls back
/// to the trace's (with a message on stderr); see [`try_new_game`].
pub fn new_game(game: &mut Game, opts: &NewGame) -> Started {
    match try_new_game(game, opts) {
        Ok(s) => s,
        Err(errs) => {
            for e in &errs {
                eprintln!("character creation: {e}");
            }
            eprintln!("character creation: using the trace's character");
            let fallback = NewGame {
                character: None,
                ..opts.clone()
            };
            try_new_game(game, &fallback).expect("the trace's character is valid")
        }
    }
}

/// [`new_game`], or the reasons the character choice is not valid (nothing is changed then).
pub fn try_new_game(game: &mut Game, opts: &NewGame) -> Result<Started, Vec<CcError>> {
    let created = opts.create(&game.sim.data.cc)?;
    let mut events = 0;
    let mut deliver = |game: &mut Game, name: &str, args: Vec<SimValue>| {
        game.event(SimEvent::new(name, args));
        events += 1 + game.tick(0.0);
    };
    game.sim.stage.live = true;
    let levels = game.sim.data.module_levels.clone();
    let cc = levels.character_creation.clone();
    game.sim.load_level(&cc);
    game.init();
    deliver(
        game,
        "GameModeStarted",
        vec![s("Campaign"), SimValue::Int(0), SimValue::Int(0)],
    );
    deliver(game, "LevelLoaded", vec![s(&cc)]);
    deliver(game, "CharacterCreationStarted", vec![]);
    deliver(game, "LevelGameplayStarted", vec![s(&cc), SimValue::Int(0)]);
    deliver(game, "LevelGameplayReady", vec![s(&cc), SimValue::Int(0)]);
    deliver(game, "MovieFinished", vec![s("GUS_CGI01_Part1")]);

    // Character creation is done: the avatar and the guardian enter the level.
    let template_name = |game: &Game, t: Guid| {
        game.sim
            .data
            .templates
            .get(&t)
            .map_or("Player".to_owned(), |t| t.name.clone())
    };
    let avatar_template = created.template;
    let avatar_id = uuid(AVATAR_ID);
    let guardian_id = uuid(GUARDIAN_ID);
    let avatar = format!("{}_{avatar_id}", template_name(game, avatar_template));
    let guardian = format!(
        "{}_{guardian_id}",
        template_name(game, opts.guardian_template)
    );
    let enter = |game: &mut Game, who: &str, template: Guid| {
        game.sim.observe_entity(who, EntityKind::Character);
        vec![g(who.to_owned()), g(template.to_string()), s(&cc)]
    };
    let args = enter(game, &avatar, avatar_template);
    deliver(game, "EnteredLevel", args);
    deliver(game, "CharacterMadePlayer", vec![g(avatar.clone())]);
    deliver(
        game,
        "CharacterReservedUserIDChanged",
        vec![
            g(avatar.clone()),
            SimValue::Int(NO_USER),
            SimValue::Int(USER_ID),
        ],
    );
    deliver(game, "GainedControl", vec![g(avatar.clone())]);
    let args = enter(game, &guardian, opts.guardian_template);
    deliver(game, "EnteredLevel", args);
    deliver(
        game,
        "CharacterSelectedForUser",
        vec![g(avatar.clone()), SimValue::Int(USER_ID)],
    );
    deliver(
        game,
        "CompanionSelectedForUser",
        vec![g(guardian.clone()), SimValue::Int(USER_ID)],
    );
    deliver(game, "CharacterJoinedParty", vec![g(avatar.clone())]);
    deliver(game, "CharacterCreationFinished", vec![]);
    // The character-creation result: stats, class/background/deity tags, combat set-up
    // (the trace's tags for it follow `CharacterCreationFinished`).
    game.sim.apply_created_character(avatar_id, &created);
    let applied = game.tick(0.0);

    // The story asks for the startup level (`TeleportPartiesToLevelWithMovie`).
    let story_requested_level = game.sim.pending_level.is_some();
    if !story_requested_level {
        game.sim.request_level(&levels.startup);
    }
    let next = game.sim.pending_level.clone().unwrap_or_default();
    deliver(game, "LevelUnloading", vec![s(&cc)]);
    deliver(game, "LeftLevel", vec![g(avatar.clone()), s(&cc)]);
    deliver(
        game,
        "EnteredLevel",
        vec![g(avatar.clone()), g(avatar_template.to_string()), s(&next)],
    );
    // The party arrives at the level's start trigger (trace: `Teleported(avatar, NULL, 0, 0,
    // 0, <S_TUT_StartPoint_001>, "RegionSwap")`, the position of the `TriggerStart` trigger).
    let start = opts
        .startup_triggers
        .iter()
        .find(|t| t.trigger_type == "TriggerStart" && t.level == next)
        .map(|t| t.position);
    if let Some(p) = start {
        game.sim.set_position(avatar_id, p);
        let args = [g(avatar.clone()), g(NULL_OBJECT.to_owned())]
            .into_iter()
            .chain([0.0, 0.0, 0.0, p[0], p[1], p[2]].map(SimValue::Real))
            .chain([s("RegionSwap")])
            .collect();
        deliver(game, "Teleported", args);
    }
    game.sim.set_triggers(opts.startup_triggers.clone());
    deliver(game, "LevelLoaded", vec![s(&next)]);
    deliver(
        game,
        "LevelGameplayStarted",
        vec![s(&next), SimValue::Int(0)],
    );
    deliver(game, "LevelGameplayReady", vec![s(&next), SimValue::Int(0)]);
    Ok(Started {
        avatar: avatar_id,
        avatar_template,
        level: next,
        story_requested_level,
        events: events + applied,
    })
}

/// The campaign story (`GustavX`, the last module of the base load order).
pub const STORY_PATH: &str = "Mods/GustavX/Story/story.div.osi";

/// Everything a new game needs from the install: game data (globals, the character-creation
/// and startup levels, character-creation data), the dialogs, the story and the startup
/// level's triggers.
///
/// Parsing the story and running it recurses deeply; call this (and drive the [`Game`]) on a
/// thread with a big stack (1 GiB, like `bg3x sim-replay`).
pub fn load_game(
    vfs: std::sync::Arc<bg3_vfs::Vfs>,
) -> Result<(Game, NewGame), Box<dyn std::error::Error + Send + Sync>> {
    let stats = bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER)?;
    let data = crate::sim::GameData::load(&vfs, &stats, &[])?;
    let startup = data.module_levels.startup.clone();
    let story = bg3_story::Story::parse(&vfs.read(STORY_PATH)?)?;
    let lib = std::sync::Arc::new(bg3_dialog::library::DialogLibrary::new(vfs.clone())?);
    let mut sim = crate::sim::Sim::new(data);
    sim.set_dialog_bank(std::sync::Arc::new(
        crate::dialog::DialogBank::from_library(lib),
    ));
    let game = Game::new(&story, sim);
    let opts = NewGame {
        startup_triggers: crate::presentation::load_triggers(&vfs, &startup),
        ..NewGame::default()
    };
    Ok((game, opts))
}
