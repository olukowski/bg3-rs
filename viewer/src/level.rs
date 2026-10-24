//! Level loading for the viewer.

use anyhow::Result;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::TextureUsages;
use bg3_vfs::Vfs;
use bg3_world::{Level, LevelObject, ResourceBanks, RootTemplates};

use crate::fly_camera::FlyCamera;
use crate::meshes::VisualMeshes;

/// The level object (`MapKey`) an entity was spawned from.
#[derive(Component)]
pub struct ObjectId(pub String);

/// `--placeholders`: draw a small box for each level object without a drawn mesh (effects,
/// objects whose visual didn't resolve). Off by default: the game draws nothing there.
#[derive(Resource, Default)]
pub struct ShowPlaceholders(pub bool);

/// A visual's mesh, material and LOD distance range (`meshes::VisualMesh`).
type MeshPart = (
    Handle<Mesh>,
    Handle<StandardMaterial>,
    Option<bevy::camera::visibility::VisibilityRange>,
);

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

/// A muted colour derived from the material ID, so different materials are distinguishable.
fn tint(key: &str) -> Color {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    let v = h.finish();
    Color::hsl((v % 360) as f32, 0.25, 0.55)
}

/// Convert a BG3 transform to Bevy space.
pub fn to_bevy(t: &bg3_world::Transform) -> Transform {
    crate::space::transform(t)
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_level(
    mut commands: Commands,
    start: Res<crate::CameraStart>,
    level: Res<LoadedLevel>,
    mut visual_meshes: ResMut<VisualMeshes>,
    level_materials: Res<crate::materials::LevelMaterials>,
    characters: Option<Res<crate::characters::LevelCharacters>>,
    placeholders: Res<ShowPlaceholders>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let cube = meshes.add(Cuboid::new(0.2, 0.2, 0.2));
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

    // Upload each visual's meshes once; a stable per-material tint until real materials exist.
    let mut uploaded: std::collections::HashMap<String, Vec<MeshPart>> = Default::default();
    let mut tints: std::collections::HashMap<String, Handle<StandardMaterial>> = Default::default();
    for (id, list) in std::mem::take(&mut visual_meshes.by_visual) {
        let parts = list
            .into_iter()
            .filter_map(|m| {
                let key = m.material.unwrap_or_default();
                let mat = match level_materials.by_id.get(&key) {
                    Some(Some(mat)) => mat.clone(),
                    Some(None) => return None, // helper geometry the game doesn't draw
                    None => tints
                        .entry(key.clone())
                        .or_insert_with(|| materials.add(tint(&key)))
                        .clone(),
                };
                let range = m.range.map(|(near, far)| {
                    bevy::camera::visibility::VisibilityRange::abrupt(near, far)
                });
                Some((meshes.add(m.mesh), mat, range))
            })
            .collect();
        uploaded.insert(id, parts);
    }

    let mut sum = Vec3::ZERO;
    for obj in &level.level.objects {
        let transform = to_bevy(&obj.transform);
        sum += transform.translation;
        if characters.as_ref().is_some_and(|c| c.handles(&obj.id)) {
            continue; // drawn by `characters.rs`
        }
        // Scenery blockers get no visual in the game; an empty list means none of the
        // visual's meshes is drawn.
        let parts = level
            .visual_id(obj)
            .filter(|_| obj.draws_visual(&level.templates))
            .and_then(|v| uploaded.get(v));
        // Decals project onto objects that receive them (`RecieveDecal`).
        let receives = level
            .templates
            .receives_decals(obj, level.level.receive_decal.get(&obj.id).copied());
        let decal_tag = if receives {
            crate::decals::RECEIVES_DECALS
        } else {
            0
        };
        if parts.is_some() || !placeholders.0 {
            // Without meshes the entity still carries the object's id and position (item
            // picking, story visibility).
            commands
                .spawn((
                    Name::new(obj.name.clone()),
                    ObjectId(obj.id.clone()),
                    transform,
                    Visibility::default(),
                ))
                .with_children(|parent| {
                    for (mesh, mat, range) in parts.into_iter().flatten() {
                        let mut e = parent.spawn((
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(mat.clone()),
                            MeshTag::new(decal_tag),
                        ));
                        if let Some(range) = range {
                            e.insert(range.clone());
                        }
                    }
                });
            continue;
        }
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
            ObjectId(obj.id.clone()),
            Mesh3d(cube.clone()),
            MeshMaterial3d(mat),
            transform,
        ));
    }

    // Median position is robust against far-away backdrops.
    let mut xs: Vec<Vec3> = level
        .level
        .objects
        .iter()
        .map(|o| to_bevy(&o.transform).translation)
        .collect();
    let median = |f: fn(&Vec3) -> f32, xs: &mut Vec<Vec3>| {
        xs.sort_by(|a, b| f(a).total_cmp(&f(b)));
        xs.get(xs.len() / 2).map_or(0.0, f)
    };
    let center = Vec3::new(
        median(|v| v.x, &mut xs),
        median(|v| v.y, &mut xs),
        median(|v| v.z, &mut xs),
    );
    let _ = sum;
    let target = start.look_at.unwrap_or(center);
    let position = start
        .position
        .unwrap_or(center + Vec3::new(0.0, 60.0, 40.0));
    let mut camera = commands.spawn((
        Camera3d {
            // The subsurface pass reads the depth buffer.
            depth_texture_usages: (TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING)
                .into(),
            ..default()
        },
        // Linear HDR target: subsurface marks live in its alpha (`sss.rs`).
        bevy::camera::Hdr,
        Transform::from_translation(position).looking_at(target, Vec3::Y),
        FlyCamera::default(),
    ));
    // A/B switch: a depth-only pass first, so the main pass shades each pixel once
    // (`docs/performance.md`).
    if std::env::var_os("BG3_DEPTH_PREPASS").is_some() {
        camera.insert(bevy::core_pipeline::prepass::DepthPrepass);
    }
    eprintln!("camera at {position} looking at {target} (level median {center})");
}
