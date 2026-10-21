//! Minimal fly camera: hold right mouse to look, WASD/QE to move, Shift to go faster.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;

pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, fly);
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

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut cameras: Query<(&FlyCamera, &mut Transform)>,
) {
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
    }
}
