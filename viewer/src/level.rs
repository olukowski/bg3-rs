//! Level loading for the viewer.

use anyhow::Result;
use bevy::prelude::*;
use bg3_vfs::Vfs;
use bg3_world::{Level, LevelObject, ResourceBanks, RootTemplates};

use crate::fly_camera::FlyCamera;

#[derive(Resource)]
pub struct LoadedLevel {
    pub level: Level,
    pub templates: RootTemplates,
    pub banks: ResourceBanks,
}

impl LoadedLevel {
    pub fn load(vfs: &Vfs, name: &str) -> Result<Self> {
        let started = std::time::Instant::now();
        let level = Level::load(vfs, name)?;
        let templates = RootTemplates::load(vfs)?;
        let banks = ResourceBanks::load(vfs)?;
        eprintln!(
            "loaded {name}: {} objects ({} templates, {} visuals) in {:.2?}",
            level.objects.len(),
            templates.templates.len(),
            banks.visuals.len(),
            started.elapsed()
        );
        Ok(Self {
            level,
            templates,
            banks,
        })
    }

    pub fn visual_id<'a>(&'a self, obj: &'a LevelObject) -> Option<&'a str> {
        obj.visual_template
            .as_deref()
            .or_else(|| self.templates.visual_for(obj.template.as_deref()?))
    }
}

/// Convert a BG3 transform to Bevy. Both are Y-up; handedness is still to be verified.
pub fn to_bevy(t: &bg3_world::Transform) -> Transform {
    let [x, y, z] = t.position;
    let [qx, qy, qz, qw] = t.rotation;
    Transform {
        translation: Vec3::new(x, y, z),
        rotation: Quat::from_xyzw(qx, qy, qz, qw).normalize(),
        scale: Vec3::splat(t.scale),
    }
}

pub fn spawn_level(
    mut commands: Commands,
    level: Res<LoadedLevel>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let cube = meshes.add(Cuboid::new(0.5, 0.5, 0.5));
    let colors = [
        ("scenery", Color::srgb(0.6, 0.6, 0.65)),
        ("item", Color::srgb(0.9, 0.7, 0.2)),
        ("character", Color::srgb(0.9, 0.2, 0.2)),
    ];
    let mats: Vec<_> = colors
        .iter()
        .map(|(kind, c)| (*kind, materials.add(*c)))
        .collect();
    let fallback = materials.add(Color::srgb(0.3, 0.3, 0.9));
    let effect = materials.add(Color::srgb(0.8, 0.3, 0.9));

    let mut sum = Vec3::ZERO;
    for obj in &level.level.objects {
        let transform = to_bevy(&obj.transform);
        sum += transform.translation;
        let is_effect = level
            .visual_id(obj)
            .is_some_and(|v| level.banks.kind(v) == Some("EffectBank"));
        let mat = if is_effect {
            effect.clone()
        } else {
            mats.iter()
                .find(|(k, _)| *k == obj.kind)
                .map_or(fallback.clone(), |(_, m)| m.clone())
        };
        commands.spawn((
            Name::new(obj.name.clone()),
            Mesh3d(cube.clone()),
            MeshMaterial3d(mat),
            transform,
        ));
    }

    let center = sum / level.level.objects.len().max(1) as f32;
    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(center + Vec3::new(0.0, 40.0, 60.0))
            .looking_at(center, Vec3::Y),
        FlyCamera::default(),
    ));
}
