//! Screenshot camera policy, using posed mesh bounds rather than game camera behaviour.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

use crate::characters::{CharacterRoot, name_matches};
use crate::fly_camera::FlyCamera;

#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub enum FrameMode {
    Face,
    #[default]
    Body,
}

#[derive(Resource)]
pub struct FrameTarget {
    name: String,
    mode: FrameMode,
    yaw: f32,
    target: Option<Entity>,
    pub ready: bool,
    pub captured: bool,
}

impl FrameTarget {
    pub fn new(name: String, mode: FrameMode, yaw: f32) -> Self {
        Self {
            name,
            mode,
            yaw: yaw.to_radians(),
            target: None,
            ready: false,
            captured: false,
        }
    }
}

// Run after character positioning, animation, transform propagation and posed mesh bounds.
// The camera is a root entity: update its global transform too, before frustum calculation.
#[allow(clippy::type_complexity)]
pub fn frame_character(
    mut frame: ResMut<FrameTarget>,
    roots: Query<(Entity, &CharacterRoot, &GlobalTransform, &Visibility), Without<FlyCamera>>,
    dialogs: Option<Res<crate::play::Dialogs>>,
    children: Query<&Children>,
    meshes: Query<(&Aabb, &GlobalTransform), (With<Mesh3d>, Without<FlyCamera>)>,
    mut cameras: Query<(&mut Projection, &mut Transform, &mut GlobalTransform), With<FlyCamera>>,
) {
    if frame.captured {
        return;
    }
    let chosen = roots
        .iter()
        .filter(|(entity, root, _, vis)| {
            **vis != Visibility::Hidden
                && frame.target.is_none_or(|target| *entity == target)
                && (name_matches(&root.name, &frame.name)
                    || dialogs
                        .as_ref()
                        .is_some_and(|d| name_matches(d.character_name(root), &frame.name)))
        })
        .min_by(|a, b| a.1.id.cmp(&b.1.id));
    let Some((entity, root, world, _)) = chosen else {
        frame.ready = false;
        return;
    };
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for child in children.iter_descendants(entity) {
        if let Ok((bounds, transform)) = meshes.get(child) {
            for corner in corners(Vec3::from(bounds.min()), Vec3::from(bounds.max())) {
                let p = transform.transform_point(corner);
                min = min.min(p);
                max = max.max(p);
            }
        }
    }
    if !min.is_finite() || !max.is_finite() || max.y - min.y < 0.01 {
        // Missing/degenerate mesh bounds: use a 1.8 m character standing at its root.
        min = world.translation() + Vec3::new(-0.3, 0.0, -0.3);
        max = world.translation() + Vec3::new(0.3, 1.8, 0.3);
    }
    // Walking turns local +Z toward travel (play::sync_characters / nav::follow_path).
    let front = world.rotation() * Vec3::Z;
    let front = Vec3::new(front.x, 0.0, front.z)
        .try_normalize()
        .unwrap_or(Vec3::Z);
    let front = Quat::from_rotation_y(frame.yaw) * front;
    for (mut projection, mut camera, mut global) in &mut cameras {
        let Projection::Perspective(perspective) = &mut *projection else {
            continue;
        };
        *camera = placement(min, max, front, frame.mode, perspective);
        // Close-up cameras can sit behind level rails or inside scenery. Clip only the
        // foreground before the nearest bound, with 5 cm clearance for the character, and
        // never the floor the character stands on.
        perspective.near = foreground_near(min, max, front, camera.translation, perspective.fov);
        *global = GlobalTransform::from(*camera);
        if frame.target.is_none() {
            let at = crate::space::vec3(((min + max) * 0.5).to_array());
            eprintln!(
                "frame: {} (height {:.2} m) at {at:?} (BG3 space), camera at {:?}",
                root.name,
                max.y - min.y,
                crate::space::vec3(camera.translation.to_array()),
            );
        }
        frame.target = Some(entity);
        frame.ready = true;
    }
}

/// Keep screenshot subjects unobscured by combat and dialog panels.
pub fn hide_ui(mut nodes: Query<&mut Visibility, With<Node>>) {
    for mut visibility in &mut nodes {
        *visibility = Visibility::Hidden;
    }
}

fn corners(min: Vec3, max: Vec3) -> impl Iterator<Item = Vec3> {
    [min.x, max.x].into_iter().flat_map(move |x| {
        [min.y, max.y]
            .into_iter()
            .flat_map(move |y| [min.z, max.z].into_iter().map(move |z| Vec3::new(x, y, z)))
    })
}

/// Near plane distance: just before the subject's nearest bound, but no further than where
/// the bottom of the view meets the subject's floor level (minus a fifth for uneven ground).
/// A near plane beyond that cuts the floor in front of the subject into straight-edged
/// slabs, with whatever lies below showing through. The camera looks horizontally
/// (`placement`), so the view's lower edge drops `tan(fov / 2)` per metre of depth.
fn foreground_near(min: Vec3, max: Vec3, front: Vec3, camera: Vec3, fov: f32) -> f32 {
    let nearest = corners(min, max)
        .map(|p| (camera - p).dot(front))
        .fold(f32::INFINITY, f32::min);
    let floor = (camera.y - min.y).max(0.0) / (fov * 0.5).tan() * 0.8;
    (nearest - 0.05).min(floor).max(0.01)
}

fn placement(
    min: Vec3,
    max: Vec3,
    front: Vec3,
    mode: FrameMode,
    lens: &PerspectiveProjection,
) -> Transform {
    let height = max.y - min.y;
    let mut target = (min + max) * 0.5;
    let distance = match mode {
        FrameMode::Face => {
            target.y = min.y + height * 0.92;
            (height * 0.6).max(0.5)
        }
        FrameMode::Body => {
            let tan_v = (lens.fov * 0.5).tan();
            let tan_h = tan_v * lens.aspect_ratio;
            let right = Vec3::Y.cross(front);
            // Fit height and width (including wings), accounting for box depth and leaving
            // a margin for idle motion at the edges of the image.
            corners(min, max)
                .map(|p| {
                    let offset = p - target;
                    let extent = (offset.y.abs() / tan_v).max(offset.dot(right).abs() / tan_h);
                    offset.dot(front) + extent * 1.25
                })
                .fold((height * 0.5).max(0.1), f32::max)
        }
    };
    Transform::from_translation(target + front * distance).looking_at(target, Vec3::Y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_spawn_tracks_position_and_freezes_at_capture() {
        let mut app = App::new();
        app.insert_resource(FrameTarget::new("test".into(), FrameMode::Body, 0.0))
            .add_systems(Update, frame_character);
        let camera = app
            .world_mut()
            .spawn((
                FlyCamera::default(),
                Projection::Perspective(default()),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.update();
        assert!(!app.world().resource::<FrameTarget>().ready);
        let root = app
            .world_mut()
            .spawn((
                CharacterRoot {
                    id: "synthetic".into(),
                    name: "Test character".into(),
                    run_speed: 0.0,
                    step_height: 0.0,
                    idle: None,
                    run: None,
                    combat: default(),
                },
                GlobalTransform::default(),
                Visibility::Inherited,
            ))
            .id();
        let mesh = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                Aabb::from_min_max(Vec3::ZERO, Vec3::Y * 2.0),
                GlobalTransform::default(),
                ChildOf(root),
            ))
            .id();
        app.update();
        assert!(app.world().resource::<FrameTarget>().ready);
        let initial = *app.world().get::<Transform>(camera).unwrap();
        app.world_mut()
            .entity_mut(root)
            .insert(GlobalTransform::from_translation(Vec3::X));
        app.world_mut()
            .entity_mut(mesh)
            .insert(GlobalTransform::from_translation(Vec3::X));
        app.update();
        let moved = *app.world().get::<Transform>(camera).unwrap();
        assert!((moved.translation - initial.translation - Vec3::X).length() < 1e-6);
        app.world_mut().resource_mut::<FrameTarget>().captured = true;
        app.world_mut()
            .entity_mut(mesh)
            .insert(GlobalTransform::from_translation(Vec3::X * 10.0));
        app.update();
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), moved);
    }

    #[test]
    fn body_fits_all_corners_from_front_and_after_orbit() {
        let min = Vec3::new(-1.5, 0.0, -0.3);
        let max = Vec3::new(1.5, 1.8, 0.3);
        let lens = PerspectiveProjection {
            aspect_ratio: 0.8,
            ..default()
        };
        for yaw in [0.0, 90.0_f32.to_radians()] {
            let front = Quat::from_rotation_y(yaw) * Vec3::Z;
            let camera = placement(min, max, front, FrameMode::Body, &lens);
            assert!(camera.translation.dot(front) > 0.0);
            let near = foreground_near(min, max, front, camera.translation, lens.fov);
            assert!(near > lens.near);
            let view = camera.to_matrix().inverse();
            for p in corners(min, max) {
                let p = view.transform_point3(p);
                assert!(p.z < -near);
                assert!(p.y.abs() < -p.z * (lens.fov * 0.5).tan());
                assert!(p.x.abs() < -p.z * (lens.fov * 0.5).tan() * lens.aspect_ratio);
            }
        }
    }

    #[test]
    fn near_plane_stops_short_of_the_floor_in_view() {
        let (min, max) = (Vec3::new(-0.3, 0.0, -0.3), Vec3::new(0.3, 2.0, 0.3));
        let lens = PerspectiveProjection::default();
        let camera = placement(min, max, Vec3::Z, FrameMode::Body, &lens);
        let near = foreground_near(min, max, Vec3::Z, camera.translation, lens.fov);
        // Where the lower edge of the view reaches the floor (y = 0) at that depth.
        let drop = near * (lens.fov * 0.5).tan();
        assert!(camera.translation.y - drop > min.y + 0.15, "near {near}");
        // Still clips scenery well in front of the subject.
        assert!(near > 1.0, "near {near}");
    }

    #[test]
    fn face_aims_at_ninety_two_percent_with_minimum_distance() {
        for height in [0.4, 1.8] {
            let camera = placement(
                Vec3::ZERO,
                Vec3::Y * height,
                Vec3::Z,
                FrameMode::Face,
                &default(),
            );
            assert!((camera.translation.y - height * 0.92).abs() < 1e-6);
            assert!((camera.translation.z - (height * 0.6).max(0.5)).abs() < 1e-6);
        }
        assert!(name_matches("LaE'zel", "lae"));
    }
}
