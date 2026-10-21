//! `bg3-viewer` — fly through BG3 levels.
//!
//! Current state: level objects rendered with their LOD0 GR2 meshes and textured materials
//! (`materials.rs`); objects without a mesh (effects, …) are shown as small boxes.

mod characters;
mod dialog;
mod fly_camera;
mod level;
mod lights;
mod materials;
mod meshes;
mod nav;
mod play;
mod space;
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
    /// (as if clicked), for screenshots.
    #[arg(long)]
    talk: Option<String>,
    /// With `--shot`: take it after this many seconds instead of after 60 frames (the game
    /// keeps running meanwhile).
    #[arg(long)]
    shot_after: Option<f32>,
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
struct ShotRequest(std::path::PathBuf, Option<f32>);

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
    let level = level::LoadedLevel::load(&vfs, &args.level)?;
    let terrain_levels: Vec<String> = std::iter::once(args.level.clone())
        .chain(args.terrain_levels.iter().cloned())
        .collect();
    let terrain = terrain::LevelTerrain::load(&vfs, &terrain_levels)?;
    let lights = lights::LevelLights::load(&vfs, &args.level, &level.templates, args.light_scale)?;
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
    // `--play`: the game runs on its own thread; the level shows what it says.
    let play = if args.play {
        let (link, started) =
            play::SimLink::start(std::sync::Arc::new(Vfs::from_install(&install)?))?;
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
    // The play camera starts behind the avatar (BG3 space) unless placed explicitly.
    let avatar_at = play.as_ref().and_then(|(_, s)| {
        s.presences
            .iter()
            .find(|p| p.id == s.avatar)
            .map(|p| Vec3::from_array(p.position))
    });
    let extra = match &play {
        Some((_, started)) => play_characters(&vfs, &level, started)?,
        None => Vec::new(),
    };
    let characters = characters::LevelCharacters::load_objects(
        &vfs,
        &level,
        level.level.objects.iter().chain(&extra),
    );
    let material_ids = meshes.by_visual.values().flatten();
    let materials = materials::PreparedMaterials::load(
        &vfs,
        &level.banks,
        material_ids
            .filter_map(|(_, m)| m.as_deref())
            .chain(characters.material_ids()),
    );

    let navigation = match nav::Navigation::load(&vfs, &args.level) {
        Ok(n) => Some(n),
        Err(e) => {
            eprintln!("nav: no AI grid ({e}); click-to-move disabled");
            None
        }
    };

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
    .insert_resource(GameData { install, vfs })
    .insert_resource(level)
    .insert_resource(meshes)
    .insert_resource(terrain)
    .insert_resource(lights)
    .insert_resource(materials)
    .insert_resource(characters)
    .insert_resource(CameraStart {
        position: args
            .camera
            .or(avatar_at.map(|p| p + Vec3::new(-5.0, 6.0, 4.0)))
            .map(|v| Vec3::from_array(space::vec3(v.to_array()))),
        look_at: args
            .look_at
            .or(avatar_at.map(|p| p + Vec3::Y))
            .map(|v| Vec3::from_array(space::vec3(v.to_array()))),
    })
    .add_systems(
        Startup,
        (
            materials::upload,
            level::spawn_level.after(materials::upload),
            terrain::spawn_terrain,
            lights::spawn_lights,
        ),
    )
    .add_systems(PostStartup, lights::apply_exposure);
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
        let loca = dialog::load_loca(&app.world().resource::<GameData>().vfs, "English")?;
        let names = play::speaker_names(
            &app.world().resource::<GameData>().vfs,
            &args.level,
            &loca,
            started.avatar,
        );
        app.add_plugins(play::PlayPlugin {
            link: std::sync::Mutex::new(Some(link)),
            loca: std::sync::Mutex::new(Some(loca)),
            names,
            talk: args.talk.clone(),
        });
    }
    if let Some(dialog) = dialog {
        app.add_plugins(dialog);
    }
    if let Some(path) = args.shot {
        app.insert_resource(ShotRequest(path, args.shot_after))
            .add_systems(Update, take_screenshot);
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
fn take_screenshot(
    mut commands: Commands,
    request: Res<ShotRequest>,
    time: Res<Time>,
    mut frame: Local<u32>,
    mut taken: Local<Option<u32>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    *frame += 1;
    let due = match request.1 {
        Some(secs) => *frame >= 60 && time.elapsed_secs() >= secs,
        None => *frame == 60,
    };
    if due && taken.is_none() {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(request.0.clone()));
        *taken = Some(*frame);
    }
    if taken.is_some_and(|t| *frame == t + 30) {
        exit.write(AppExit::Success);
    }
}
