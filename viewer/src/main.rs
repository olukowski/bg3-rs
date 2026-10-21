//! `bg3-viewer` — fly through BG3 levels.
//!
//! Current state: level objects rendered with their LOD0 GR2 meshes and textured materials
//! (`materials.rs`); objects without a mesh (effects, …) are shown as small boxes.

mod fly_camera;
mod level;
mod materials;
mod meshes;
mod space;

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
    let material_ids = meshes.by_visual.values().flatten();
    let materials = materials::PreparedMaterials::load(
        &vfs,
        &level.banks,
        material_ids.filter_map(|(_, m)| m.as_deref()),
    );

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
    .insert_resource(materials)
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
            setup,
            materials::upload,
            level::spawn_level.after(materials::upload),
        ),
    );
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

fn setup(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: false,
            illuminance: 8000.0,
            ..default()
        },
        Transform::from_xyz(4.0, 10.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 400.0,
        ..default()
    });
}
