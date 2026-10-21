//! Starting a new game on the sim the way the engine starts the campaign, up to the player
//! waking up on the Nautiloid (`TUT_Avernus_C`).
//!
//! The order is the runtime trace's (`docs/research/osiris-runtime-log.md`, "Startup
//! sequence"): `InitGame`, `GameModeStarted("Campaign")`, the character-creation level, the
//! player's choices, `CharacterCreationFinished`, then the story's
//! `TeleportPartiesToLevelWithMovie` takes the party to the startup level.
//!
//! **Character creation is skipped**: the avatar is a placeholder made from a character
//! creation preset's root template (by default the one the trace's player picked, a male deep
//! gnome), with a fixed UUID, announced with the same events character creation raises for a
//! created avatar. Its race, class and appearance choices are not modelled (its sheet comes
//! from the template's stats, see `docs/research/osiris-engine-api.md`, open question 9).

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
/// Placeholder UUIDs of the created avatar and guardian (the engine makes random ones).
pub const AVATAR_ID: &str = "a7a7a7a7-0000-4000-8000-000000000001";
pub const GUARDIAN_ID: &str = "a7a7a7a7-0000-4000-8000-000000000002";
/// The first local user (`CharacterReservedUserIDChanged(…, -65536, 65537)` in the trace).
pub const USER_ID: i32 = 65537;
const NO_USER: i32 = -65536;

/// How to start.
#[derive(Clone, Debug)]
pub struct NewGame {
    pub avatar_template: Guid,
    pub guardian_template: Guid,
    /// Triggers of the startup level ([`crate::presentation::load_triggers`]); the party is
    /// placed at its start trigger (`TriggerType` `TriggerStart`).
    pub startup_triggers: Vec<TriggerDef>,
}

impl Default for NewGame {
    fn default() -> Self {
        Self {
            avatar_template: Guid::parse(TRACE_AVATAR_TEMPLATE).expect("uuid"),
            guardian_template: Guid::parse(TRACE_GUARDIAN_TEMPLATE).expect("uuid"),
            startup_triggers: Vec::new(),
        }
    }
}

/// What [`new_game`] did.
#[derive(Clone, Debug)]
pub struct Started {
    pub avatar: Guid,
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
/// set a dialog bank first for the dialogs to run).
pub fn new_game(game: &mut Game, opts: &NewGame) -> Started {
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

    // Character creation creates the avatar and the guardian in the CC level.
    let template_name = |t: Guid| {
        game.sim
            .data
            .templates
            .get(&t)
            .map_or("Player".to_owned(), |t| t.name.clone())
    };
    let avatar_id = Guid::parse(AVATAR_ID).expect("uuid");
    let guardian_id = Guid::parse(GUARDIAN_ID).expect("uuid");
    let avatar = format!("{}_{avatar_id}", template_name(opts.avatar_template));
    let guardian = format!("{}_{guardian_id}", template_name(opts.guardian_template));
    for (who, template) in [
        (&avatar, opts.avatar_template),
        (&guardian, opts.guardian_template),
    ] {
        game.sim.observe_entity(who, EntityKind::Character);
        deliver(
            game,
            "EnteredLevel",
            vec![g(who.clone()), g(template.to_string()), s(&cc)],
        );
    }
    deliver(game, "LevelLoaded", vec![s(&cc)]);
    deliver(game, "CharacterCreationStarted", vec![]);
    deliver(game, "LevelGameplayStarted", vec![s(&cc), SimValue::Int(0)]);
    deliver(game, "LevelGameplayReady", vec![s(&cc), SimValue::Int(0)]);
    deliver(game, "MovieFinished", vec![s("GUS_CGI01_Part1")]);
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
        vec![
            g(avatar.clone()),
            g(opts.avatar_template.to_string()),
            s(&next),
        ],
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
    Started {
        avatar: avatar_id,
        level: next,
        story_requested_level,
        events,
    }
}

/// The campaign story (`GustavX`, the last module of the base load order).
pub const STORY_PATH: &str = "Mods/GustavX/Story/story.div.osi";

/// Everything a new game needs from the install: game data (globals, the character-creation
/// and startup levels), the dialogs, the story and the startup level's triggers.
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
