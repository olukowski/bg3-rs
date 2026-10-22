//! Minimal fly camera: hold right mouse to look, WASD/QE to move, Shift to go faster.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;

pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CombatCamera>().add_systems(
            Update,
            fly.after(crate::combat_ui::animate)
                .run_if(not(resource_exists::<crate::framing::FrameTarget>)),
        );
    }
}

#[derive(Component)]
pub struct FlyCamera {
    pub speed: f32,
    pub sensitivity: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            speed: 10.0,
            sensitivity: 0.003,
        }
    }
}

/// Viewer camera policy; not a port of BG3's camera controller.
#[derive(Resource)]
pub struct CombatCamera {
    enabled: bool,
    requested: Option<bg3_sim::Guid>,
    active: Option<bg3_sim::Guid>,
    pan: Option<Pan>,
}

struct Pan {
    target: bg3_sim::Guid,
    start: Vec3,
    distance: f32,
    elapsed: f32,
}

impl Default for CombatCamera {
    fn default() -> Self {
        Self {
            enabled: true,
            requested: None,
            active: None,
            pan: None,
        }
    }
}

impl CombatCamera {
    /// Follow off from the start (as after pressing `F`).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..default()
        }
    }

    pub fn turn_started(&mut self, who: bg3_sim::Guid) {
        self.active = Some(who);
        self.requested = Some(who);
        self.pan = None;
    }

    /// Members of a shared turn can begin acting at different times. A delayed AI reply for
    /// the same actor must not undo a manual camera interruption.
    pub fn actor_started(&mut self, who: bg3_sim::Guid) {
        if self.active != Some(who) {
            self.turn_started(who);
        }
    }

    pub fn stop(&mut self) {
        self.requested = None;
        self.pan = None;
    }
}

fn pan_position(start: Vec3, end: Vec3, elapsed: f32) -> Vec3 {
    let t = (elapsed / 0.5).clamp(0.0, 1.0);
    start.lerp(end, t * t * (3.0 - 2.0 * t))
}

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut follow: ResMut<CombatCamera>,
    roots: Query<(&crate::characters::CharacterRoot, &Transform), Without<FlyCamera>>,
    mut cameras: Query<(&FlyCamera, &mut Transform)>,
) {
    if keys.just_pressed(KeyCode::KeyF) {
        follow.enabled = !follow.enabled;
        follow.stop();
        info!(
            "Combat camera follow: {}",
            if follow.enabled { "on" } else { "off" }
        );
    }
    let manual = [
        KeyCode::KeyW,
        KeyCode::KeyS,
        KeyCode::KeyA,
        KeyCode::KeyD,
        KeyCode::KeyQ,
        KeyCode::KeyE,
    ]
    .iter()
    .any(|k| keys.pressed(*k))
        || (buttons.pressed(MouseButton::Right) && motion.delta != Vec2::ZERO);
    if manual || !follow.enabled {
        follow.stop();
    }
    let position = |id| {
        roots
            .iter()
            .find(|(r, _)| bg3_sim::Guid::parse(&r.id) == Some(id))
            .map(|(_, t)| t.translation + Vec3::Y)
    };
    for (cam, mut transform) in &mut cameras {
        if buttons.pressed(MouseButton::Right) && motion.delta != Vec2::ZERO {
            let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
            yaw -= motion.delta.x * cam.sensitivity;
            pitch = (pitch - motion.delta.y * cam.sensitivity).clamp(-1.54, 1.54);
            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        }

        let mut dir = Vec3::ZERO;
        let bindings = [
            (KeyCode::KeyW, *transform.forward()),
            (KeyCode::KeyS, *transform.back()),
            (KeyCode::KeyA, *transform.left()),
            (KeyCode::KeyD, *transform.right()),
            (KeyCode::KeyE, Vec3::Y),
            (KeyCode::KeyQ, Vec3::NEG_Y),
        ];
        for (key, v) in bindings {
            if keys.pressed(key) {
                dir += v;
            }
        }
        let boost = if keys.pressed(KeyCode::ShiftLeft) {
            5.0
        } else {
            1.0
        };
        transform.translation += dir.normalize_or_zero() * cam.speed * boost * time.delta_secs();
        if let Some(target) = follow.requested.take()
            && let Some(at) = position(target)
        {
            let forward = *transform.forward();
            // Keep the distance to the current look-at plane at the actor's chest height.
            let plane_distance = (at.y - transform.translation.y) / forward.y;
            let distance = if forward.y.abs() > 0.05 && plane_distance > 0.0 {
                plane_distance
            } else {
                (at - transform.translation).dot(forward).max(2.0)
            };
            follow.pan = Some(Pan {
                target,
                start: transform.translation,
                distance,
                elapsed: 0.0,
            });
        }
        if let Some(pan) = &mut follow.pan {
            pan.elapsed += time.delta_secs();
            if let Some(at) = position(pan.target) {
                let end = at - *transform.forward() * pan.distance;
                transform.translation = pan_position(pan.start, end, pan.elapsed);
            }
            pan.elapsed = pan.elapsed.min(0.5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_preserves_camera_offset_and_finishes_in_half_a_second() {
        let start = Vec3::new(2.0, 8.0, 12.0);
        let end = start + Vec3::new(20.0, 0.0, 5.0);
        assert_eq!(pan_position(start, end, 0.0), start);
        assert_eq!(pan_position(start, end, 0.25), start.lerp(end, 0.5));
        assert_eq!(pan_position(start, end, 0.5), end);
        assert_eq!(pan_position(start, end, 5.0), end);
    }

    #[test]
    fn interruption_lasts_until_a_new_turn() {
        let mut camera = CombatCamera::default();
        let who = bg3_sim::Guid::parse("00000000-0000-0000-0000-000000000001").unwrap();
        camera.turn_started(who);
        camera.stop();
        assert!(camera.requested.is_none());
        assert!(camera.pan.is_none());
        camera.actor_started(who);
        assert!(camera.requested.is_none());
        camera.turn_started(who);
        assert_eq!(camera.requested, Some(who));
    }
}
