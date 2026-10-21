//! Level lights → Bevy lights, plus sun, sky ambient and camera exposure from the level's
//! lighting/atmosphere resources. Unit conversions are approximations; see
//! `docs/research/lighting.md` ("Viewer mapping").

use anyhow::Result;
use bevy::camera::Exposure;
use bevy::prelude::*;
use bg3_vfs::Vfs;
use bg3_world::lights::{kelvin_to_rgb, load_level_lights};
use bg3_world::{LevelLighting, Light, LightKind, RootTemplates};

use crate::space;

/// Exposure used when the atmosphere has none (EV100).
const DEFAULT_EV100: f32 = 14.0;

/// Viewer-tuned factor from a local light's `Intensity` to Bevy lumens (see the research note:
/// most Nautiloid lights are `PreExpose`, i.e. their intensity is relative to the exposure).
pub const DEFAULT_LIGHT_SCALE: f32 = 200.0;

#[derive(Resource)]
pub struct LevelLights {
    pub lights: Vec<Light>,
    pub lighting: LevelLighting,
    pub scale: f32,
}

impl LevelLights {
    pub fn load(vfs: &Vfs, level: &str, templates: &RootTemplates, scale: f32) -> Result<Self> {
        let lights = load_level_lights(vfs, level, templates)?;
        let lighting = LevelLighting::load(vfs, level)?;
        let count = |k: fn(&LightKind) -> bool| lights.iter().filter(|l| k(&l.kind)).count();
        eprintln!(
            "lights: {} ({} point, {} spot, {} directional); lighting {:?}, atmosphere {:?}, exposure {:?}",
            lights.len(),
            count(|k| *k == LightKind::Point),
            count(|k| *k == LightKind::Spot),
            count(|k| *k == LightKind::Directional),
            lighting.lighting_name,
            lighting.atmosphere_name,
            lighting.exposure,
        );
        Ok(Self {
            lights,
            lighting,
            scale,
        })
    }

    pub fn ev100(&self) -> f32 {
        // Auto-exposure isn't implemented; use the bottom of the allowed range (brightest image).
        self.lighting
            .exposure
            .map_or(DEFAULT_EV100, |[min, _max, comp]| min - comp)
    }
}

fn color(c: [f32; 3]) -> Color {
    Color::linear_rgb(c[0], c[1], c[2])
}

fn tinted(c: [f32; 3], kelvin: f32, use_temperature: bool) -> [f32; 3] {
    if !use_temperature {
        return c;
    }
    let k = kelvin_to_rgb(kelvin);
    [c[0] * k[0], c[1] * k[1], c[2] * k[2]]
}

/// Bevy-space transform of a light, with Bevy's forward (−Z) along the BG3 light's forward.
///
/// Assumption: BG3 spot/directional lights shine along their local +Z (see the research note).
fn light_transform(l: &Light) -> Transform {
    let t = space::transform(&l.transform);
    let [x, y, z, w] = l.transform.rotation;
    let q = Quat::from_xyzw(x, y, z, w).normalize();
    let dir = Vec3::from_array(space::vec3((q * Vec3::Z).to_array()));
    let up = if dir.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    Transform::from_translation(t.translation).looking_to(dir, up)
}

pub fn spawn_lights(
    mut commands: Commands,
    lights: Res<LevelLights>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    let scale = lights.scale;
    let mut spawned = 0;
    for l in lights
        .lights
        .iter()
        .filter(|l| l.enabled && l.intensity > 0.0)
    {
        let c = color(l.effective_color());
        let range = l.radius.max(0.1);
        let name = Name::new(format!("light {}", l.name));
        // A local light's intensity is treated as candela-like: lumens = I · 4π · scale.
        let lumens = l.intensity * 4.0 * std::f32::consts::PI * scale;
        match l.kind {
            LightKind::Point | LightKind::Unknown(_) => {
                commands.spawn((
                    name,
                    PointLight {
                        color: c,
                        intensity: lumens,
                        range,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    space::transform(&l.transform),
                ));
            }
            LightKind::Spot => {
                // Assumed: `Angle` = (inner, outer) half-angles in degrees.
                let outer = l.angle[1].clamp(1.0, 89.0).to_radians();
                let inner = l.angle[0].clamp(0.0, l.angle[1]).to_radians().min(outer);
                commands.spawn((
                    name,
                    SpotLight {
                        color: c,
                        intensity: lumens,
                        range,
                        inner_angle: inner,
                        outer_angle: outer,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    light_transform(l),
                ));
            }
            LightKind::Directional => {
                // A box light (`DirectionLightDimensions` = width, height, depth) approximated
                // by a spot light at the box origin covering the box cross-section.
                let [w, h, depth] = l.direction_dimensions;
                let depth = depth.max(0.5);
                let outer = ((w.max(h) * 0.5) / depth).atan().clamp(0.2, 1.4);
                commands.spawn((
                    name,
                    SpotLight {
                        color: c,
                        intensity: lumens,
                        range: depth.max(range),
                        inner_angle: outer * 0.7,
                        outer_angle: outer,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    light_transform(l),
                ));
            }
        }
        spawned += 1;
    }

    let lighting = &lights.lighting;
    if let Some(sun) = &lighting.sun {
        // Yaw around +Y, pitch above the horizon; the light travels from the sun towards the
        // ground. The exact BG3 angle convention is unverified.
        let (yaw, pitch) = (sun.yaw.to_radians(), sun.pitch.to_radians());
        let to_sun = [
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        ];
        let to_sun = Vec3::from_array(space::vec3(to_sun));
        commands.spawn((
            Name::new("sun"),
            DirectionalLight {
                color: color(tinted(sun.color, sun.kelvin, sun.use_temperature)),
                illuminance: sun.intensity,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::default().looking_to(-to_sun, Vec3::Y),
        ));
    }
    if let Some(sky) = &lighting.sky {
        // Sky light intensity treated as illuminance from a uniform sky: luminance = E / π.
        let c = tinted(sky.color, sky.kelvin, sky.use_temperature);
        ambient.color = color(c);
        ambient.brightness = sky.intensity / std::f32::consts::PI;
    }
    eprintln!(
        "spawned {spawned} lights, sun: {}, exposure EV100 {}",
        lighting.sun.is_some(),
        lights.ev100()
    );
}

/// Give cameras the level's exposure (runs after the camera is spawned).
pub fn apply_exposure(
    mut commands: Commands,
    lights: Res<LevelLights>,
    cameras: Query<Entity, Added<Camera3d>>,
) {
    for e in &cameras {
        commands.entity(e).insert(Exposure {
            ev100: lights.ev100(),
        });
    }
}
