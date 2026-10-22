//! Level lights → Bevy lights, plus sun, sky ambient and camera exposure from the level's
//! lighting/atmosphere resources. Unit conversions are approximations; see
//! `docs/research/lighting.md` ("Viewer mapping").

use anyhow::Result;
use bevy::camera::Exposure;
use bevy::prelude::*;
use bg3_vfs::Vfs;
use bg3_world::lights::{
    ExposureSettings, LightingSources, PostProcessing, decode_gamma, kelvin_to_rgb,
    load_level_lights,
};
use bg3_world::{LevelLighting, Light, LightKind, RootTemplates};

use crate::space;

/// Exposure used when the atmosphere has none (EV100).
const DEFAULT_EV100: f32 = 14.0;

/// Extra factor on local light intensities (1 = the game's units; `--light-scale` for A/B).
pub const DEFAULT_LIGHT_SCALE: f32 = 1.0;

#[derive(Resource)]
pub struct LevelLights {
    pub lights: Vec<Light>,
    pub lighting: LevelLighting,
    pub scale: f32,
    /// What `lighting` is resolved from, to follow the camera between triggers.
    sources: LightingSources,
    /// The lighting and atmosphere triggers in effect (see [`LightingSources::zones_at`]).
    zones: [Option<usize>; 2],
}

impl LevelLights {
    pub fn load(
        vfs: &Vfs,
        level: &str,
        templates: &RootTemplates,
        scale: f32,
        at: Option<[f32; 3]>,
    ) -> Result<Self> {
        let lights = load_level_lights(vfs, level, templates)?;
        let sources = LightingSources::load(vfs, level)?;
        let zones = sources.zones_at(at);
        let lighting = sources.resolve(zones);
        let count = |k: fn(&LightKind) -> bool| lights.iter().filter(|l| k(&l.kind)).count();
        eprintln!(
            "lights: {} ({} point, {} spot, {} directional); lighting {:?}, atmosphere {:?} (triggers {:?}), exposure {:?}",
            lights.len(),
            count(|k| *k == LightKind::Point),
            count(|k| *k == LightKind::Spot),
            count(|k| *k == LightKind::Directional),
            lighting.lighting_name,
            lighting.atmosphere_name,
            lighting.zones,
            lighting.exposure_settings,
        );
        Ok(Self {
            lights,
            lighting,
            scale,
            sources,
            zones,
        })
    }

    /// Exposure and grading parameters from the atmosphere (see `post.rs`). The game's
    /// gameplay camera uses the first `ExposureSettings` entry (**assumption**: the second is
    /// for dialog/cinematic cameras).
    pub fn post_uniform(&self, screen: UVec2) -> crate::post::PostUniform {
        let e = self.lighting.exposure_settings.first();
        let (min, max, comp) = e.map_or((DEFAULT_EV100, DEFAULT_EV100, 0.0), |e| {
            (e.min, e.max, e.compensation)
        });
        let p = &self.lighting.post;
        if p.white_balance_temperature != 6500.0 || p.white_balance_tint != 0.0 {
            eprintln!(
                "post: white balance {} K / tint {} not implemented (identity used)",
                p.white_balance_temperature, p.white_balance_tint
            );
        }
        let mut ranges = [Vec4::ZERO; 6];
        for (out, r) in ranges.iter_mut().zip(&p.ranges) {
            *out = Vec4::new(
                r.xy_offset[0],
                r.xy_offset[1],
                r.saturation,
                1.35f32.powf(r.exposure_shift),
            );
        }
        let ev_size = crate::post::ev_texture_size(screen);
        crate::post::PostUniform {
            exposure: Vec4::new(min, max, comp, e.map_or(0.0, |e| e.enabled as u8 as f32)),
            metering: Vec4::new(
                e.map_or(0.0, |e| e.min_percentile),
                e.map_or(1.0, |e| e.max_percentile),
                e.map_or(0.0, |e| e.metering_mode as f32),
                min - comp,
            ),
            // The exposure buffer's histogram range: EV100 −8 to 18.
            histogram: Vec4::new(-8.0, 26.0, ev_size.x, ev_size.y),
            grading: Vec4::new(p.contrast, p.highlights, p.shadows, p.pivot),
            grading2: Vec4::new(0.0, p.saturation, p.hue, 0.0),
            ranges,
        }
    }

    pub fn ev100(&self) -> f32 {
        // Auto-exposure isn't implemented; use the bottom of the allowed range (brightest image).
        self.lighting
            .exposure
            .map_or(DEFAULT_EV100, |[min, _max, comp]| min - comp)
    }
}

/// The gameplay camera's atmosphere parameters, before conversion to shader uniforms.
#[derive(Clone, Debug, PartialEq)]
struct AtmosphereValues {
    exposure: ExposureSettings,
    post: PostProcessing,
}

impl AtmosphereValues {
    fn read(lighting: &LevelLighting) -> Self {
        Self {
            exposure: lighting
                .exposure_settings
                .first()
                .cloned()
                .unwrap_or(ExposureSettings {
                    enabled: false,
                    min: DEFAULT_EV100,
                    max: DEFAULT_EV100,
                    compensation: 0.0,
                    min_percentile: 0.0,
                    max_percentile: 1.0,
                    metering_mode: 0,
                }),
            post: lighting.post.clone(),
        }
    }

    fn apply(&self, lighting: &mut LevelLighting) {
        let e = &self.exposure;
        lighting.exposure = Some([e.min, e.max, e.compensation]);
        if let Some(first) = lighting.exposure_settings.first_mut() {
            *first = e.clone();
        } else {
            lighting.exposure_settings.push(e.clone());
        }
        lighting.post = self.post.clone();
    }

    /// Assumption: linear in stored parameter space; discrete settings switch at halfway.
    // bg3: AtmosphereTrigger/FadeTime, PostProcessing (4.1.1.7631656).
    fn blend(&self, target: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        if t == 0.0 {
            return self.clone();
        }
        if t == 1.0 {
            return target.clone();
        }
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let discrete = if t < 0.5 { self } else { target };
        let a = &self.exposure;
        let b = &target.exposure;
        let exposure = ExposureSettings {
            enabled: discrete.exposure.enabled,
            min: lerp(a.min, b.min),
            max: lerp(a.max, b.max),
            compensation: lerp(a.compensation, b.compensation),
            min_percentile: lerp(a.min_percentile, b.min_percentile),
            max_percentile: lerp(a.max_percentile, b.max_percentile),
            metering_mode: discrete.exposure.metering_mode,
        };
        let a = &self.post;
        let b = &target.post;
        let post = PostProcessing {
            bloom: discrete.post.bloom,
            bloom_threshold: lerp(a.bloom_threshold, b.bloom_threshold),
            bloom_amount: lerp(a.bloom_amount, b.bloom_amount),
            white_balance_temperature: lerp(
                a.white_balance_temperature,
                b.white_balance_temperature,
            ),
            white_balance_tint: lerp(a.white_balance_tint, b.white_balance_tint),
            contrast: lerp(a.contrast, b.contrast),
            highlights: lerp(a.highlights, b.highlights),
            shadows: lerp(a.shadows, b.shadows),
            saturation: lerp(a.saturation, b.saturation),
            hue: lerp(a.hue, b.hue),
            pivot: lerp(a.pivot, b.pivot),
            ranges: std::array::from_fn(|i| bg3_world::lights::GradingRange {
                exposure_shift: lerp(a.ranges[i].exposure_shift, b.ranges[i].exposure_shift),
                saturation: lerp(a.ranges[i].saturation, b.ranges[i].saturation),
                xy_offset: std::array::from_fn(|j| {
                    lerp(a.ranges[i].xy_offset[j], b.ranges[i].xy_offset[j])
                }),
            }),
        };
        Self { exposure, post }
    }
}

pub(crate) struct AtmosphereFade {
    from: AtmosphereValues,
    target: AtmosphereValues,
    elapsed: f32,
    duration: f32,
}

impl AtmosphereFade {
    fn new(from: AtmosphereValues, target: AtmosphereValues, duration: f32) -> Self {
        Self {
            from,
            target,
            elapsed: 0.0,
            duration: duration.max(0.0),
        }
    }

    fn advance(&mut self, seconds: f32) -> AtmosphereValues {
        self.elapsed += seconds.max(0.0);
        let t = if self.duration > 0.0 {
            self.elapsed / self.duration
        } else {
            1.0
        };
        self.from.blend(&self.target, t)
    }

    fn finished(&self) -> bool {
        self.elapsed >= self.duration
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

/// The game's point/spot falloff for every surface: registers `shaders/game_light.wesl` (the
/// character shader imports it) and patches Bevy's point light to call it in place of
/// `getDistanceAttenuation`.
pub struct GameLightPlugin;

const PBR_LIGHTING: &str = "embedded://bevy_pbr/render/pbr_lighting.wesl";
const GAME_LIGHT: &str = include_str!("shaders/game_light.wesl");
const BEVY_FALLOFF: &str =
    "getDistanceAttenuation(distance_square, (*light).color_inverse_square_range.w)";
const GAME_FALLOFF: &str = "bg3_light_falloff(distance_square, (*light).color_inverse_square_range.w, (*light).shadow_depth_bias)";

#[derive(Resource)]
struct PbrLightingPatch(Handle<Shader>, bool);

impl Plugin for GameLightPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "shaders/game_light.wesl");
        let library: Handle<Shader> =
            bevy::asset::load_embedded_asset!(app, "shaders/game_light.wesl");
        std::mem::forget(library);
        let handle = app.world().resource::<AssetServer>().load(PBR_LIGHTING);
        app.insert_resource(PbrLightingPatch(handle, false))
            .add_systems(First, patch_pbr_lighting);
    }
}

fn patch_pbr_lighting(mut patch: ResMut<PbrLightingPatch>, mut shaders: ResMut<Assets<Shader>>) {
    if patch.1 {
        return;
    }
    let Some(shader) = shaders.get(&patch.0) else {
        return;
    };
    patch.1 = true;
    let source = shader.source.as_str();
    if !source.contains(BEVY_FALLOFF) {
        eprintln!("lights: Bevy's point light changed; the game's falloff is not patched in");
        return;
    }
    let patched = format!(
        "{}\n{GAME_LIGHT}",
        source.replacen(BEVY_FALLOFF, GAME_FALLOFF, 1)
    );
    if let Some(mut shader) = shaders.get_mut(&patch.0) {
        shader.source = bevy::shader::Source::Wesl(patched.into());
    }
}

/// Bevy-space transform of a light, with Bevy's forward (−Z) along the BG3 light's forward.
///
/// Spot and box lights shine along their local +Z (`UpdateBounds`, the packed direction).
fn light_transform(l: &Light) -> Transform {
    let t = space::transform(&l.transform);
    let [x, y, z, w] = l.transform.rotation;
    let q = Quat::from_xyzw(x, y, z, w).normalize();
    let dir = Vec3::from_array(space::vec3((q * Vec3::Z).to_array()));
    let up = if dir.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    Transform::from_translation(t.translation).looking_to(dir, up)
}

/// The game's packed `Gain`/fill-light value (`Gain_FillLight`), carried to the shader in the
/// light's `shadow_depth_bias` (unused: the viewer renders no local shadow maps) and read by
/// `bg3_light_falloff` (`shaders/game_light.wesl`).
fn gain_fill(gain: f32, fill: bool) -> f32 {
    if fill {
        -gain.max(1.19e-7)
    } else {
        gain.max(0.0)
    }
}

/// Spawn the level's lights in the game's units. Bevy's exposure is the game's
/// (`1/(1.2·2^EV100)`), and Bevy divides a point/spot light's lumens by 4π:
///
/// - colour `Color · blackbody(Kelvin) · Intensity` (`Color` linear, as stored);
/// - point lights: the game divides by 4π too, so lumens = `Intensity`;
/// - spot lights: the game divides by π, so lumens = `4 · Intensity`; `Angle` holds full cone
///   angles, Bevy takes half angles; the cone curve is the same;
/// - falloff, `Gain` and `FlatFalloff`: `bg3_light_falloff`, patched into Bevy's lighting;
/// - box lights: approximated (see below).
///
/// Every Nautiloid light is `PreExpose` (exposed like everything else); lights without it are
/// in display units in the game, which Bevy can't express (not handled).
///
/// bg3: ls::ClusteredAssignmentStage::SetLightData, SetDirectionLightData, ls::LightComponent
/// (4.1.1.7631656)
pub fn spawn_lights(
    mut commands: Commands,
    lights: Res<LevelLights>,
    fog: Res<crate::fog::LevelFog>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    // Every light scatters in the game's fog (`ScatteringScale`, 1 by default).
    let volumetric = fog.active();
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
        let bias = gain_fill(l.gain, l.flat_falloff);
        match l.kind {
            LightKind::Point | LightKind::Unknown(_) => {
                let mut e = commands.spawn((
                    name,
                    PointLight {
                        color: c,
                        intensity: l.intensity * scale,
                        range,
                        shadow_maps_enabled: false,
                        shadow_depth_bias: bias,
                        shadow_normal_bias: game_shadow_marker(l.shadow),
                        ..default()
                    },
                    space::transform(&l.transform),
                ));
                if volumetric {
                    e.insert(bevy::light::VolumetricLight);
                }
            }
            LightKind::Spot => {
                let outer = (l.angle[1] * 0.5).clamp(0.5, 89.0).to_radians();
                let inner = (l.angle[0] * 0.5)
                    .clamp(0.0, l.angle[1] * 0.5)
                    .to_radians()
                    .min(outer);
                let mut e = commands.spawn((
                    name,
                    SpotLight {
                        color: c,
                        intensity: 4.0 * l.intensity * scale,
                        range,
                        inner_angle: inner,
                        outer_angle: outer,
                        shadow_maps_enabled: false,
                        shadow_depth_bias: bias,
                        shadow_normal_bias: game_shadow_marker(l.shadow),
                        ..default()
                    },
                    light_transform(l),
                ));
                if volumetric {
                    e.insert(bevy::light::VolumetricLight);
                }
            }
            LightKind::Directional => {
                // A box light: `DirectionLightDimensions` (w, h, depth) from the source face (the
                // object origin) along +Z, a constant direction and illuminance `Color ·
                // Intensity` inside, fading over the attenuation lengths. Approximated by a fill
                // spot light from the source face: its falloff is the constant 4/R², so lumens
                // = π · Intensity · R² gives the box's illuminance on the axis; the range window
                // stands in for the far fade, the cone (covering the box's cross-section at
                // mid-depth) for the sides.
                let [w, h, depth] = l.direction_dimensions;
                let depth = depth.max(0.5);
                let outer = ((w.max(h) * 0.5) / (depth * 0.5)).atan().clamp(0.1, 1.4);
                let mut e = commands.spawn((
                    name,
                    SpotLight {
                        color: c,
                        intensity: std::f32::consts::PI * l.intensity * depth * depth * scale,
                        range: depth,
                        inner_angle: outer * 0.7,
                        outer_angle: outer,
                        shadow_maps_enabled: false,
                        shadow_depth_bias: gain_fill(0.0, true),
                        shadow_normal_bias: game_shadow_marker(l.shadow),
                        ..default()
                    },
                    light_transform(l),
                ));
                if volumetric {
                    e.insert(bevy::light::VolumetricLight);
                }
            }
        }
        spawned += 1;
    }

    let lighting = &lights.lighting;
    if let Some(sun) = &lighting.sun {
        // Towards the sun, BG3 space: yaw 0 = +X, 90° = +Z; pitch = elevation.
        // bg3: ls::Lighting::Visit (SunDir) (4.1.1.7631656)
        let (yaw, pitch) = (sun.yaw.to_radians(), sun.pitch.to_radians());
        let to_sun = [
            pitch.cos() * yaw.cos(),
            pitch.sin(),
            pitch.cos() * yaw.sin(),
        ];
        let to_sun = Vec3::from_array(space::vec3(to_sun));
        // With atmospheric scattering the sun lights surfaces with the scattering sun colour
        // (raw) × intensity × the atmosphere's transmittance (not ported: taken as 1);
        // otherwise with pow(SunColor, 2.2) × SunIntensity × blackbody(Kelvin).
        let (c, lux) = match &lighting.sky {
            Some(sky) if sky.scattering => (sky.scattering_sun_color, sky.scattering_sun_intensity),
            _ => (
                tinted(decode_gamma(sun.color), sun.kelvin, sun.use_temperature),
                sun.intensity,
            ),
        };
        let mut e = commands.spawn((
            Name::new("sun"),
            DirectionalLight {
                color: color(c),
                illuminance: lux,
                shadow_maps_enabled: sun.shadow,
                // With shadow maps the character shader uses them; otherwise it reads the marker
                // as "shadowed in the game".
                shadow_normal_bias: if sun.shadow {
                    DirectionalLight::default().shadow_normal_bias
                } else {
                    game_shadow_marker(true)
                },
                ..default()
            },
            bevy::light::CascadeShadowConfigBuilder {
                maximum_distance: sun.shadow_far_plane,
                ..default()
            }
            .build(),
            Transform::default().looking_to(-to_sun, Vec3::Y),
        ));
        if volumetric && sun.shadow {
            e.insert(bevy::light::VolumetricLight);
        }
    }
    // Ambient comes from the light probes (`probes.rs`); the sky light only reaches surfaces
    // through them (its intensity is not applied at run time).
    ambient.brightness = 0.0;
    eprintln!(
        "spawned {spawned} lights, sun: {}, exposure EV100 {}",
        lighting.sun.is_some(),
        lights.ev100()
    );
}

/// Shadow maps are off in the viewer; a light that casts shadows in the game carries a negative
/// normal bias instead, which the character shader reads (subsurface transmission through thick
/// objects measures the occluder depth in the light's shadow map, `character.wesl`).
fn game_shadow_marker(shadow: bool) -> f32 {
    if shadow { -1.0 } else { 0.0 }
}

/// Give cameras the level's exposure and post-processing (runs after the camera is spawned).
pub fn apply_exposure(
    mut commands: Commands,
    lights: Res<LevelLights>,
    cameras: Query<Entity, Added<Camera3d>>,
    windows: Query<&Window>,
) {
    let screen = windows
        .iter()
        .next()
        .map_or(UVec2::new(1280, 720), |w| w.physical_size());
    for e in &cameras {
        if crate::post::enabled() {
            commands
                .entity(e)
                .insert(crate::post::camera_bundle(lights.post_uniform(screen)));
        } else {
            commands.entity(e).insert(Exposure {
                ev100: lights.ev100(),
            });
        }
    }
}

/// The engine picks the lighting and atmosphere triggers at the camera's eye position as the
/// camera moves (`docs/research/lighting.md`). Follow the camera for the atmosphere (exposure
/// and grading, read by the post passes every frame); a different lighting trigger would need
/// the sun, probes and fog respawned, which is not implemented, so the lighting chosen at start
/// stays. Assumptions: the NEW trigger supplies FadeTime, blending is linear, and
/// discrete settings switch halfway. The first camera update always selects instantly.
// bg3: ecl::CameraSystem::CheckTriggers (4.1.1.7631656)
pub fn follow_camera_triggers(
    mut lights: ResMut<LevelLights>,
    mut cameras: Query<
        (
            &GlobalTransform,
            Option<&mut crate::post::GamePost>,
            &mut Exposure,
        ),
        With<crate::fly_camera::FlyCamera>,
    >,
    windows: Query<&Window>,
    mut warned: Local<bool>,
    mut initialized: Local<bool>,
    mut fade: Local<Option<AtmosphereFade>>,
    time: Res<Time>,
) {
    let Some((transform, post, mut exposure)) = cameras.iter_mut().next() else {
        return;
    };
    let eye = space::vec3(transform.translation().to_array());
    let zones = lights.sources.zones_at(Some(eye));
    let name = |z: Option<usize>| z.map(|i| lights.sources.zones[i].name.clone());
    if zones[0] != lights.zones[0] && !*warned {
        *warned = true;
        eprintln!(
            "lighting: camera in lighting trigger {:?}, keeping {:?} (switching lighting at runtime is not implemented)",
            name(zones[0]),
            name(lights.zones[0]),
        );
    }
    let atmosphere_trigger = name(zones[1]);
    // Advance the old transition before capturing a retarget's starting values.
    let mut changed = false;
    if let Some(active) = fade.as_mut() {
        active
            .advance(time.delta_secs())
            .apply(&mut lights.lighting);
        changed = true;
        if active.finished() {
            *fade = None;
        }
    }
    if zones[1] != lights.zones[1] {
        let from = AtmosphereValues::read(&lights.lighting);
        let lighting = lights.sources.resolve([lights.zones[0], zones[1]]);
        eprintln!(
            "atmosphere: camera at {eye:?} in {:?}: {:?}, exposure {:?}",
            atmosphere_trigger, lighting.atmosphere_name, lighting.exposure,
        );
        let duration = if *initialized {
            zones[1].map_or(0.0, |i| lights.sources.zones[i].fade_time)
        } else {
            0.0
        };
        let mut transition = AtmosphereFade::new(from, AtmosphereValues::read(&lighting), duration);
        lights.zones[1] = zones[1];
        lights.lighting = lighting;
        transition.advance(0.0).apply(&mut lights.lighting);
        *fade = (!transition.finished()).then_some(transition);
        changed = true;
    }
    *initialized = true;
    if !changed {
        return;
    }
    match post {
        Some(mut post) => {
            let screen = windows
                .iter()
                .next()
                .map_or(UVec2::new(1280, 720), |w| w.physical_size());
            post.uniform = lights.post_uniform(screen);
        }
        None => exposure.ev100 = lights.ev100(),
    }
}

#[cfg(test)]
mod atmosphere_tests {
    use super::*;

    fn values(x: f32, enabled: bool) -> AtmosphereValues {
        AtmosphereValues {
            exposure: ExposureSettings {
                enabled,
                min: x,
                max: x + 2.0,
                compensation: x / 2.0,
                min_percentile: x / 100.0,
                max_percentile: 1.0 - x / 100.0,
                metering_mode: enabled as i64,
            },
            post: PostProcessing {
                contrast: x,
                hue: x,
                bloom: enabled,
                bloom_threshold: x + 1.0,
                bloom_amount: x,
                white_balance_temperature: 6500.0 + x,
                white_balance_tint: x,
                highlights: x,
                shadows: x,
                saturation: x,
                pivot: x,
                ranges: [bg3_world::lights::GradingRange {
                    exposure_shift: x,
                    saturation: x,
                    xy_offset: [x, -x],
                }; 6],
            },
        }
    }

    fn assert_intermediate(actual: AtmosphereValues, expected: AtmosphereValues) {
        let a = actual.exposure;
        let b = expected.exposure;
        for (a, b) in [
            a.min,
            a.max,
            a.compensation,
            a.min_percentile,
            a.max_percentile,
        ]
        .into_iter()
        .zip([
            b.min,
            b.max,
            b.compensation,
            b.min_percentile,
            b.max_percentile,
        ]) {
            assert!((a - b).abs() < 1e-6, "{a} != {b}");
        }
        assert_eq!(a.enabled, b.enabled);
        assert_eq!(a.metering_mode, b.metering_mode);
        assert_eq!(actual.post, expected.post);
    }

    #[test]
    fn atmosphere_fade_start_mid_end_and_discrete_midpoint() {
        let a = values(2.0, false);
        let b = values(10.0, true);
        let mut fade = AtmosphereFade::new(a.clone(), b.clone(), 4.0);
        assert_eq!(fade.advance(0.0), a);
        let early = fade.advance(1.0);
        assert_eq!(early.exposure.min, 4.0);
        assert!(!early.exposure.enabled);
        assert!(!early.post.bloom);
        let mid = fade.advance(1.0);
        assert_intermediate(mid, values(6.0, true));
        assert!(!fade.finished());
        assert_eq!(fade.advance(2.0), b);
        assert!(fade.finished());
        assert_eq!(fade.advance(10.0), b);
    }

    #[test]
    fn retarget_starts_from_current_values_and_zero_fade_is_instant() {
        let a = values(2.0, false);
        let b = values(10.0, true);
        let mut fade = AtmosphereFade::new(a.clone(), b.clone(), 4.0);
        let current = fade.advance(2.0);
        let mut retarget = AtmosphereFade::new(current.clone(), a.clone(), 2.0);
        assert_eq!(retarget.advance(0.0), current);
        assert_intermediate(retarget.advance(1.0), values(4.0, false));
        assert_eq!(retarget.advance(1.0), a);
        let mut instant = AtmosphereFade::new(a, b.clone(), 0.0);
        assert_eq!(instant.advance(0.0), b);
        assert!(instant.finished());
    }
}
