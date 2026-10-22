//! `bg3-viewer` — fly through BG3 levels.
//!
//! Current state: level objects rendered with their LOD0 GR2 meshes and textured materials
//! (`materials.rs`); objects without a drawn mesh (effects, …) are shown as small boxes with
//! `--placeholders`.

mod bloom;
mod cc_ui;
mod char_materials;
mod characters;
mod combat_ui;
mod decals;
mod dialog;
mod fly_camera;
mod fog;
mod framing;
mod level;
mod level_materials;
mod lights;
mod materials;
mod meshes;
mod nav;
mod pick;
mod play;
mod post;
mod probes;
mod space;
mod sss;
mod terrain;

use anyhow::Result;
use bevy::prelude::*;
use bg3_install::Install;
use bg3_vfs::Vfs;
use clap::Parser;

#[derive(Parser)]
#[command(about)]
struct Args {
    /// Level to load.
    #[arg(default_value = "TUT_Avernus_C")]
    level: String,
    /// Camera position `x,y,z` in BG3 world coordinates (default: above the level centre).
    #[arg(long, value_parser = parse_vec3, allow_hyphen_values = true)]
    camera: Option<Vec3>,
    /// Camera target `x,y,z` in BG3 world coordinates.
    #[arg(long, value_parser = parse_vec3, allow_hyphen_values = true)]
    look_at: Option<Vec3>,
    /// Save a screenshot to this path after a few frames, then exit.
    #[arg(long)]
    shot: Option<std::path::PathBuf>,
    /// Also render the terrain of these levels (e.g. the sub-levels of `WLD_Main_A`).
    #[arg(long, value_delimiter = ',')]
    terrain_levels: Vec<String>,
    /// Factor from BG3 local-light intensity to Bevy lumens (see docs/research/lighting.md).
    #[arg(long, default_value_t = lights::DEFAULT_LIGHT_SCALE)]
    light_scale: f32,
    /// Hide level objects, leaving terrain and lights.
    #[arg(long)]
    terrain_only: bool,
    /// Draw small boxes for level objects without a drawn mesh (effects, unresolved visuals).
    #[arg(long)]
    placeholders: bool,
    /// Run this dialog in an on-screen panel (resource name, e.g. `TUT_Lab_TrappedDevourer`).
    #[arg(long)]
    dialog: Option<String>,
    /// Draw the AI grid (walkable tiles green, blocked red/yellow).
    #[arg(long)]
    nav: bool,
    /// Character to control with left clicks (name substring; default: nearest the camera
    /// target).
    #[arg(long)]
    player: Option<String>,
    /// Walk the controlled character to `x,y,z` (BG3 world coordinates) on start.
    #[arg(long, value_parser = parse_vec3, allow_hyphen_values = true)]
    walk_to: Option<Vec3>,
    /// Play: start a new game on the simulation (the Nautiloid, character creation skipped)
    /// and drive the level from it: characters follow the story, dialogs show in a panel,
    /// left click moves the avatar or talks to a character.
    #[arg(long)]
    play: bool,
    /// With `--play`: once no dialog runs, talk to the character whose name contains this
    /// (as if clicked), or else use the item with use actions named like it (e.g.
    /// `Brinepool`), for screenshots.
    #[arg(long)]
    talk: Option<String>,
    /// With `--shot`: take it after this many seconds instead of after 60 frames (the game
    /// keeps running meanwhile).
    #[arg(long)]
    shot_after: Option<f32>,
    /// With `--play`: frame the front of a character (case-insensitive name substring).
    #[arg(long, requires = "play")]
    frame: Option<String>,
    /// Frame the whole body or a face close-up.
    #[arg(long, value_enum, default_value_t = framing::FrameMode::Body, requires = "frame")]
    frame_mode: framing::FrameMode,
    /// Orbit the framing camera around the character's vertical axis, in degrees.
    #[arg(
        long,
        default_value_t = 0.0,
        allow_hyphen_values = true,
        requires = "frame"
    )]
    frame_yaw: f32,
    /// With `--play`: jump to the deck fight's set-up (Lae'zel recruited, the party placed in
    /// the imp room) instead of walking there; the fight starts when the imps notice you.
    #[arg(long)]
    combat_test: bool,
    /// With `--play`: the AI plays the party's turns too (for demos and screenshots).
    #[arg(long)]
    combat_autoplay: bool,
    /// Keep the camera where `--camera` puts it: no combat camera follow (for close-ups).
    #[arg(long)]
    no_follow: bool,
    /// With `--play`: create the player character on a character creation screen first, then
    /// start (the viewer restarts itself with `--character`).
    #[arg(long)]
    cc: bool,
    /// With `--play`: the player character, as `bg3x cc --spec` prints it (default: the
    /// trace's, a deep gnome cleric).
    #[arg(long)]
    character: Option<String>,
}

fn parse_vec3(s: &str) -> Result<Vec3, String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|x| x.trim().parse().map_err(|e| format!("{x}: {e}")))
        .collect::<Result<_, _>>()?;
    match v[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => Err("expected x,y,z".into()),
    }
}

#[derive(Resource)]
pub struct CameraStart {
    pub position: Option<Vec3>,
    pub look_at: Option<Vec3>,
}

#[derive(Resource)]
pub(crate) struct ShotRequest(pub std::path::PathBuf, pub Option<f32>);

/// The game's files, mounted read-only.
#[derive(Resource)]
pub struct GameData {
    pub install: Install,
    pub vfs: Vfs,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let install = Install::locate()?;
    if !install.is_reference_version() {
        eprintln!(
            "warning: BG3 {} is not the reference build {}",
            install.version,
            bg3_install::REFERENCE_VERSION
        );
    }
    let vfs = Vfs::from_install(&install)?;
    eprintln!("mounted {} files from BG3 {}", vfs.len(), install.version);
    if args.cc && args.play {
        return cc_ui::run(&vfs, args.shot.clone());
    }
    let character = args
        .character
        .as_deref()
        .map(bg3_sim::cc::CharacterChoice::from_spec)
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let level = level::LoadedLevel::load(&vfs, &args.level)?;
    let terrain_levels: Vec<String> = std::iter::once(args.level.clone())
        .chain(args.terrain_levels.iter().cloned())
        .collect();
    let terrain = terrain::LevelTerrain::load(&vfs, &terrain_levels, &level.banks)?;
    let started = std::time::Instant::now();
    let meshes = meshes::VisualMeshes::load(&vfs, &level);
    eprintln!(
        "converted meshes for {} visuals ({} failed) in {:.2?}",
        meshes.by_visual.len(),
        meshes.failed.len(),
        started.elapsed()
    );
    for (id, err) in meshes.failed.iter().take(5) {
        eprintln!("  {id}: {err}");
    }
    let navigation = match nav::Navigation::load(&vfs, &args.level) {
        Ok(n) => Some(n),
        Err(e) => {
            eprintln!("nav: no AI grid ({e}); click-to-move disabled");
            None
        }
    };

    // `--play`: the game runs on its own thread; the level shows what it says.
    let play = if args.play {
        let (link, started) = play::SimLink::start(
            std::sync::Arc::new(Vfs::from_install(&install)?),
            args.combat_test,
            navigation.as_ref().map(|n| n.grid.clone()),
            character,
        )?;
        if started.level != args.level {
            eprintln!(
                "warning: the game started in {}, showing {}",
                started.level, args.level
            );
        }
        Some((link, started))
    } else {
        None
    };
    let avatar_position = play.as_ref().and_then(|(_, s)| {
        s.presences
            .iter()
            .find(|p| p.id == s.avatar)
            .map(|p| Vec3::from_array(p.position))
    });
    // The play camera starts behind the avatar (BG3 space) unless placed explicitly.
    let avatar_at = avatar_position.filter(|_| args.frame.is_none());
    let camera_start = args
        .camera
        .or(avatar_at.map(|p| p + Vec3::new(-5.0, 6.0, 4.0)));
    // Lighting and atmosphere triggers choose the level-wide lighting where the camera is
    // (`lights::follow_camera_triggers` keeps the atmosphere up to date as it moves). Without
    // a start position yet (`--frame` places the camera later), start where the player is.
    let lights = lights::LevelLights::load(
        &vfs,
        &args.level,
        &level.templates,
        args.light_scale,
        camera_start
            .or(avatar_position)
            .or(args.look_at)
            .map(|v| v.to_array()),
    )?;
    let fog = fog::LevelFog::load(
        &vfs,
        &args.level,
        &level.templates,
        lights.lighting.fog_phase,
    );
    let decals = decals::LevelDecals::load(&vfs, &args.level, &level);
    let probes = probes::LevelProbes::load(
        &vfs,
        &args.level,
        &level.templates,
        lights.lighting.lighting_id.as_deref(),
    );
    let extra = match &play {
        Some((_, started)) => play_characters(&vfs, &level, started)?,
        None => Vec::new(),
    };
    let characters = characters::LevelCharacters::load_objects(
        &vfs,
        &level,
        level.level.objects.iter().chain(&extra),
        play.as_ref().map(|(_, s)| {
            (
                s.avatar.to_string(),
                s.avatar_equipment.as_slice(),
                &s.avatar_appearance,
            )
        }),
        play.as_ref()
            .map_or(&std::collections::HashMap::new(), |(_, s)| &s.weapon_sets),
    );
    let material_ids = meshes.by_visual.values().flatten();
    let materials = materials::PreparedMaterials::load(
        &vfs,
        &level.banks,
        material_ids
            .filter_map(|(_, m)| m.as_deref())
            .chain(characters.material_ids()),
    );

    let dialog = match &args.dialog {
        Some(name) => Some(dialog::DialogPlugin::load(&install, name, &args.level)?),
        None => None,
    };

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("bg3-viewer — {}", args.level),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(fly_camera::FlyCameraPlugin)
    .add_plugins(characters::CharactersPlugin)
    .add_plugins(char_materials::CharacterMaterialsPlugin)
    .add_plugins(level_materials::LevelMaterialsPlugin)
    .add_plugins(terrain::TerrainPlugin)
    .add_plugins(decals::DecalsPlugin)
    .add_plugins(sss::SssPlugin)
    .add_plugins(post::GamePostPlugin)
    .add_plugins(bloom::GameBloomPlugin)
    .add_plugins(lights::GameLightPlugin)
    .add_plugins(fog::FogAmbientPlugin)
    .insert_resource(GameData { install, vfs })
    .insert_resource(level)
    .insert_resource(meshes)
    .insert_resource(terrain)
    .insert_resource(lights)
    .insert_resource(probes)
    .insert_resource(fog)
    .insert_resource(decals)
    .insert_resource(materials)
    .insert_resource(characters)
    .insert_resource(level::ShowPlaceholders(args.placeholders))
    .insert_resource(CameraStart {
        position: camera_start.map(|v| Vec3::from_array(space::vec3(v.to_array()))),
        look_at: args
            .look_at
            .or(avatar_at.map(|p| p + Vec3::Y))
            .map(|v| Vec3::from_array(space::vec3(v.to_array()))),
    })
    .add_systems(
        Startup,
        (
            decals::upload,
            materials::upload.after(decals::upload),
            level::spawn_level.after(materials::upload),
            terrain::spawn_terrain,
            lights::spawn_lights,
            probes::spawn_probes,
            fog::spawn_fog,
        ),
    )
    .add_systems(
        PostStartup,
        (lights::apply_exposure, probes::apply_sky, fog::apply_fog),
    )
    .add_systems(Last, lights::follow_camera_triggers)
    .add_systems(
        PostUpdate,
        lights::assign_box_shadow_carriers.before(TransformSystems::Propagate),
    );
    if args.terrain_only {
        app.add_systems(PostStartup, terrain::hide_objects);
    }
    if let Some(navigation) = navigation {
        app.add_plugins(nav::NavPlugin {
            navigation,
            options: nav::NavOptions {
                overlay: args.nav,
                player: args.player.clone(),
                walk_to: args.walk_to.map(|v| v.to_array()),
                player_id: play.as_ref().map(|(_, s)| s.avatar.to_string()),
            },
        });
    }
    if let Some((link, started)) = play {
        let loca = std::sync::Arc::new(dialog::load_loca(
            &app.world().resource::<GameData>().vfs,
            "English",
        )?);
        let names = started.names.clone();
        app.add_plugins(play::PlayPlugin {
            link: std::sync::Mutex::new(Some(link)),
            loca: loca.clone(),
            names: names.clone(),
            talk: args.talk.clone(),
        });
        app.add_plugins(combat_ui::CombatPlugin {
            loca,
            names,
            autoplay: args.combat_autoplay,
        });
    }
    if let Some(dialog) = dialog {
        app.add_plugins(dialog);
    }
    if let Some(name) = args.frame {
        app.insert_resource(framing::FrameTarget::new(
            name,
            args.frame_mode,
            args.frame_yaw,
        ))
        .add_systems(
            PostUpdate,
            framing::hide_ui
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        )
        .add_systems(
            PostUpdate,
            framing::frame_character
                .after(bevy::camera::visibility::VisibilitySystems::CalculateBounds)
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::UpdateFrusta),
        );
    }
    if args.no_follow || app.world().contains_resource::<framing::FrameTarget>() {
        app.insert_resource(fly_camera::CombatCamera::disabled());
    }
    if let Some(points) = pick::requested() {
        app.insert_resource(pick::PickPoints(points))
            .add_systems(PostUpdate, pick::mark_backfaces)
            .add_systems(Last, pick::pick_at_shot);
    }
    if let Some(path) = args.shot {
        app.insert_resource(ShotRequest(path, args.shot_after))
            .add_systems(Last, take_screenshot);
    }
    app.run();
    Ok(())
}

/// Level objects for the characters the game placed in the level that the level file doesn't
/// have: global characters (origins and companions, from `Globals`) and the created avatar
/// (made from its root template), at the game's positions.
fn play_characters(
    vfs: &Vfs,
    level: &level::LoadedLevel,
    started: &play::Started,
) -> Result<Vec<bg3_world::LevelObject>> {
    let have: std::collections::HashSet<&str> =
        level.level.objects.iter().map(|o| o.id.as_str()).collect();
    let globals: std::collections::HashMap<String, bg3_world::LevelObject> =
        bg3_world::level::load_global_characters(vfs)?
            .into_iter()
            .map(|o| (o.id.clone(), o))
            .collect();
    let mut out = Vec::new();
    for p in &started.presences {
        let id = p.id.to_string();
        if p.kind != bg3_sim::EntityKind::Character || have.contains(id.as_str()) {
            continue;
        }
        let mut obj = match globals.get(&id) {
            Some(o) => o.clone(),
            None if p.id == started.avatar => bg3_world::LevelObject {
                id: id.clone(),
                name: p.name.clone(),
                kind: "character".into(),
                template: Some(started.avatar_template.to_string()),
                visual_template: None,
                is_blocker: None,
                transform: bg3_world::Transform::default(),
                character: None,
                module: String::new(),
            },
            None => continue,
        };
        obj.transform.position = p.position;
        out.push(obj);
    }
    eprintln!(
        "play: {} characters placed by the game ({})",
        out.len(),
        out.iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(out)
}

/// Wait for assets to settle, save one screenshot, then exit once it's written.
pub(crate) fn take_screenshot(
    mut commands: Commands,
    request: Res<ShotRequest>,
    time: Res<Time>,
    mut framing: Option<ResMut<framing::FrameTarget>>,
    mut frame: Local<u32>,
    mut taken: Local<Option<u32>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    *frame += 1;
    let due = match request.1 {
        Some(secs) => *frame >= 60 && time.elapsed_secs() >= secs,
        None => *frame >= 60,
    };
    if due && taken.is_none() && framing.as_ref().is_none_or(|f| f.ready) {
        if let Some(f) = framing.as_mut() {
            f.captured = true;
        }
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(request.0.clone()));
        *taken = Some(*frame);
    }
    if taken.is_some_and(|t| *frame == t + 30) {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_arguments_require_play_and_accept_modes_and_negative_yaw() {
        assert!(Args::try_parse_from(["viewer", "--frame", "Lae"]).is_err());
        assert!(Args::try_parse_from(["viewer", "--frame-mode", "face"]).is_err());
        assert!(Args::try_parse_from(["viewer", "--frame-yaw", "45"]).is_err());
        assert!(Args::try_parse_from(["viewer", "--shot", "synthetic.png"]).is_ok());
        assert!(
            Args::try_parse_from([
                "viewer",
                "--play",
                "--frame",
                "Lae",
                "--frame-mode",
                "face",
                "--frame-yaw",
                "-45",
            ])
            .is_ok()
        );
    }
}
