//! `bg3-viewer` — fly through BG3 levels.
//!
//! Current state: level objects rendered with their LOD0 GR2 meshes and flat placeholder
//! materials; objects without a mesh (effects, …) are shown as small boxes.

mod fly_camera;
mod level;
mod lights;
mod meshes;
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
struct ShotRequest(std::path::PathBuf);

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

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("bg3-viewer — {}", args.level),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(fly_camera::FlyCameraPlugin)
    .insert_resource(GameData { install, vfs })
    .insert_resource(level)
    .insert_resource(meshes)
    .insert_resource(terrain)
    .insert_resource(lights)
    .insert_resource(CameraStart {
        position: args
            .camera
            .map(|v| Vec3::from_array(space::vec3(v.to_array()))),
        look_at: args
            .look_at
            .map(|v| Vec3::from_array(space::vec3(v.to_array()))),
    })
    .add_systems(
        Startup,
        (
            level::spawn_level,
            terrain::spawn_terrain,
            lights::spawn_lights,
        ),
    )
    .add_systems(PostStartup, lights::apply_exposure);
    if args.terrain_only {
        app.add_systems(PostStartup, terrain::hide_objects);
    }
    if let Some(path) = args.shot {
        app.insert_resource(ShotRequest(path))
            .add_systems(Update, take_screenshot);
    }
    app.run();
    Ok(())
}

/// Wait for assets to settle, save one screenshot, then exit once it's written.
fn take_screenshot(
    mut commands: Commands,
    request: Res<ShotRequest>,
    mut frame: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    *frame += 1;
    if *frame == 60 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(request.0.clone()));
    }
    if *frame == 90 {
        exit.write(AppExit::Success);
    }
}
