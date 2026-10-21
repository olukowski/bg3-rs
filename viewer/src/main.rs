//! `bg3-viewer` — fly through BG3 levels.
//!
//! Current state: places every level object at its transform as a placeholder box (meshes land
//! with `bg3-gr2`).

mod fly_camera;
mod level;

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
}

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

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("bg3-viewer — {}", args.level),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(fly_camera::FlyCameraPlugin)
        .insert_resource(GameData { install, vfs })
        .insert_resource(level)
        .add_systems(Startup, (setup, level::spawn_level))
        .run();
    Ok(())
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
