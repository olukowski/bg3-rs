//! Level lights → Bevy lights, plus sun, sky ambient and camera exposure from the level's
//! lighting/atmosphere resources. Unit conversions are approximations; see
//! `docs/research/lighting.md` ("Viewer mapping").

use anyhow::Result;
use bevy::asset::uuid_handle;
use bevy::camera::Exposure;
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
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
                // `BG3_METERING=<mode>` overrides the metering mode (calibration checks only).
                std::env::var("BG3_METERING")
                    .ok()
                    .and_then(|m| m.parse::<f32>().ok())
                    .unwrap_or(e.map_or(0.0, |e| e.metering_mode as f32)),
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
            vignette: discrete.post.vignette,
            vignette_color: std::array::from_fn(|i| lerp(a.vignette_color[i], b.vignette_color[i])),
            vignette_intensity: lerp(a.vignette_intensity, b.vignette_intensity),
            vignette_power: lerp(a.vignette_power, b.vignette_power),
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
/// - box lights: exact, in the game's light loop (see [`box_light`]).
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
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let mut boxes = Vec::new();
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
                // Box lights are evaluated by the game's light loop from `BOX_LIGHTS`
                // (`box_light` below), not by a Bevy light. `BG3_NO_BOX_LIGHTS=1` drops them
                // (A/B checks only).
                let only = std::env::var("BG3_BOX_ONLY").ok();
                if std::env::var_os("BG3_NO_BOX_LIGHTS").is_none()
                    && only.is_none_or(|o| l.name.contains(&o))
                {
                    boxes.push(box_light(l, scale));
                }
            }
        }
        spawned += 1;
    }

    let box_count = boxes.len();
    // A storage buffer can't be empty: an all-zero light is outside every box.
    if boxes.is_empty() {
        boxes.push(GpuBoxLight::default());
    }
    if buffers
        .insert(
            &BOX_LIGHTS,
            ShaderBuffer::new(
                boxes
                    .iter()
                    .flat_map(GpuBoxLight::words)
                    .collect::<Vec<_>>(),
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            ),
        )
        .is_err()
    {
        eprintln!("lights: box light buffer not stored");
    }

    let grid = box_light_grid(&boxes[..box_count]);
    if buffers
        .insert(
            &BOX_LIGHT_GRID,
            ShaderBuffer::new(grid, bevy::asset::RenderAssetUsages::RENDER_WORLD),
        )
        .is_err()
    {
        eprintln!("lights: box light grid not stored");
    }

    let lighting = &lights.lighting;
    spawn_box_shadow_carriers(
        &mut commands,
        &boxes[..box_count],
        lighting.sun.as_ref().map_or(100.0, |s| s.shadow_far_plane),
    );
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
            Sun,
            DirectionalLight {
                color: color(c),
                illuminance: lux,
                shadow_maps_enabled: sun.shadow && std::env::var_os("BG3_NO_SUN_SHADOWS").is_none(),
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
                // A/B switch for attributing the cascades' cost (`docs/performance.md`).
                num_cascades: std::env::var("BG3_SUN_CASCADES")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4),
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
        "spawned {spawned} lights ({box_count} box lights in the game's light loop), sun: {}, exposure EV100 {}",
        lighting.sun.is_some(),
        lights.ev100()
    );
}

/// The level's box lights, bound by every game-shaded material (`level_materials.rs`,
/// `char_materials.rs`) and evaluated in `shaders/game_lighting.wesl`.
pub const BOX_LIGHTS: Handle<ShaderBuffer> = uuid_handle!("6f1f2f4e-3c55-4c1e-9a43-0b6c1d3f7a52");

/// The box lights by world cell (`decals::xz_grid`), so the light loop only visits the boxes
/// that can contain a pixel. The grid spans the boxes under [`BOX_GRID_MAX_EXTENT`] across; the
/// larger ones (one ~10 km box on the Nautiloid) are clipped to it, and the shader visits every
/// box outside the grid.
pub const BOX_LIGHT_GRID: Handle<ShaderBuffer> =
    uuid_handle!("2d0b7c55-61a8-4f0e-9d4b-6f3c8e1a9b27");

/// Grid cell (m) of [`BOX_LIGHT_GRID`].
const BOX_GRID_CELL: f32 = 4.0;
/// Boxes wider than this (m) don't extend [`BOX_LIGHT_GRID`].
const BOX_GRID_MAX_EXTENT: f32 = 512.0;

fn box_light_grid(boxes: &[GpuBoxLight]) -> Vec<u32> {
    let bounds: Vec<([f32; 2], [f32; 2])> = boxes
        .iter()
        .map(|b| {
            let (lo, hi) = bounds(ShadowedBox::new(b).corners.into_iter());
            ([lo.x, lo.z], [hi.x, hi.z])
        })
        .collect();
    let small = |(lo, hi): &&([f32; 2], [f32; 2])| {
        (hi[0] - lo[0]).max(hi[1] - lo[1]) <= BOX_GRID_MAX_EXTENT
    };
    let region = bounds.iter().filter(small).fold(None, |r, (lo, hi)| {
        let (rlo, rhi): ([f32; 2], [f32; 2]) = r.unwrap_or((*lo, *hi));
        Some((
            [rlo[0].min(lo[0]), rlo[1].min(lo[1])],
            [rhi[0].max(hi[0]), rhi[1].max(hi[1])],
        ))
    });
    crate::decals::xz_grid(&bounds, BOX_GRID_CELL, region)
}

/// One box light as the shader reads it (`BoxLight` in `game_lighting.wesl`).
#[derive(Clone, Copy, Default, Debug)]
pub struct GpuBoxLight {
    /// Rows of the affine map from Bevy world space to box coordinates (inside = all in 0–1).
    to_box: [Vec4; 3],
    /// Linear colour × intensity, and the fade curve (0 linear, 1 square, 2 smoothstep,
    /// 3 smootherstep).
    color_function: Vec4,
    /// Fade along the axis: start, end (box coordinates), 1/start, 1/(1 − end).
    attenuation_z: Vec4,
    /// Towards the light (Bevy space), and 1 if the game shadows the light.
    direction_shadow: Vec4,
    /// Side fades: width / side length, height / side length.
    attenuation_xy: Vec4,
    /// Box size (w, h, depth); CPU only.
    size: Vec3,
}

impl GpuBoxLight {
    /// The seven `vec4<f32>` of the shader's struct, in order.
    fn words(&self) -> [[f32; 4]; 7] {
        [
            self.to_box[0].to_array(),
            self.to_box[1].to_array(),
            self.to_box[2].to_array(),
            self.color_function.to_array(),
            self.attenuation_z.to_array(),
            self.direction_shadow.to_array(),
            self.attenuation_xy.to_array(),
        ]
    }
}

/// A box light (`LightType` 2) packed like the engine does: the box spans `±w/2`, `±h/2` and
/// `0..depth` along the light's local +Z from its origin, which maps to box coordinates
/// `(x/w + ½, y/h + ½, z/depth)` (rotation and position only, no scale); the light shines along
/// +Z with the constant illuminance `Color · Intensity` (no distance falloff, no π), times the
/// fades: over `AttenuationStart` from the source face (`z < start/depth`), over
/// `AttenuationEnd` before the far face and over `AttenuationSide` from each side, each with
/// the light's curve.
///
/// bg3: ls::ClusteredAssignmentStage::SetDirectionLightData, Illuminance_Direction
/// (ClusteredLighting_*, Core.metallib) (4.1.1.7631656)
fn box_light(l: &Light, scale: f32) -> GpuBoxLight {
    let [w, h, depth] = l.direction_dimensions.map(|d| d.max(1e-5));
    let [start, end, side] = l.direction_attenuation;
    // bg3: SetDirectionLightData clamps start/depth to [1e-5, 1], 1 − end/depth to [0, 0.99999]
    // and the side length to ≥ 1e-5.
    let z_start = (start / depth).clamp(1e-5, 1.0);
    let z_end = (1.0 - end / depth).clamp(0.0, 0.99999);
    let side = side.max(1e-5);
    let [x, y, z, qw] = l.transform.rotation;
    let q = Quat::from_xyzw(x, y, z, qw).normalize();
    let pos = Vec3::from_array(l.transform.position);
    // BG3 world → box coordinates, after mirroring Bevy's z back into BG3 space.
    let to_box = Mat4::from_translation(Vec3::new(0.5, 0.5, 0.0))
        * Mat4::from_scale(Vec3::new(1.0 / w, 1.0 / h, 1.0 / depth))
        * Mat4::from_quat(q.inverse())
        * Mat4::from_translation(-pos)
        * Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
    let towards = Vec3::from_array(space::vec3((-(q * Vec3::Z)).to_array()));
    let [r, g, b] = l.effective_color().map(|c| c * l.intensity * scale);
    GpuBoxLight {
        to_box: [to_box.row(0), to_box.row(1), to_box.row(2)],
        color_function: Vec4::new(r, g, b, f32::from(l.direction_attenuation_function.min(3))),
        attenuation_z: Vec4::new(z_start, z_end, 1.0 / z_start, 1.0 / (1.0 - z_end)),
        direction_shadow: towards.extend(if l.shadow { 1.0 } else { 0.0 }),
        attenuation_xy: Vec4::new(w / side, h / side, 0.0, 0.0),
        size: Vec3::new(w, h, depth),
    }
}

/// Box lights with `Shadow` are shadowed in the game with an orthographic map along the
/// light's axis that covers the box (`DoClusteredLight` input: the shadow matrix's first three
/// rows, no perspective divide). A Bevy directional light renders the same kind of map, so
/// shadowed box lights get black directional lights that only render shadow maps; the light
/// loop finds a box light's carrier by direction (`game_lighting.wesl`), so boxes along one
/// axis share it. The game shadows every shadowed box light; Bevy allows ten directional lights,
/// one of which is the sun, so [`assign_box_shadow_carriers`] gives the carriers to the
/// directions of the shadowed boxes in view nearest the camera (further ones go unshadowed).
///
/// Each carrier renders one map fitted to its boxes ([`fit_box_shadow_cascades`]), as the game
/// does: shadow culling then draws only what lies in (or towards the light from) the boxes.
/// Carriers stay shadow-enabled, an unused one with an empty map, so the shadow map array never
/// changes size as the camera moves. `BG3_BOX_SHADOW_CASCADES=1` gives the carriers Bevy's four
/// cascades over the camera instead, with casters culled to the boxes
/// ([`cull_box_shadow_casters`]): sharper near the camera, 10–25 % slower (A/B checks).
/// Approximations: the map resolution is Bevy's directional one (the game's local light shadow
/// tiles, `ls::LocalLightShadowStage`, are not read); biases and filtering are Bevy's.
///
/// bg3: ls::ClusteredAssignmentStage::SetDirectionLightData (ShadowDataIndex_VolShadow_Shadow),
/// ClusteredLighting_* (4.1.1.7631656)
fn spawn_box_shadow_carriers(commands: &mut Commands, boxes: &[GpuBoxLight], far_plane: f32) {
    // `BG3_NO_BOX_SHADOWS=1`: box lights unshadowed (A/B checks only).
    if std::env::var_os("BG3_NO_BOX_SHADOWS").is_some() {
        return;
    }
    let shadowed: Vec<ShadowedBox> = boxes
        .iter()
        .filter(|b| b.direction_shadow.w > 0.5)
        .map(ShadowedBox::new)
        .collect();
    if shadowed.is_empty() {
        return;
    }
    let mut directions: Vec<Vec3> = Vec::new();
    for b in &shadowed {
        if !directions
            .iter()
            .any(|d| d.dot(b.direction) > BOX_SHADOW_MATCH)
        {
            directions.push(b.direction);
        }
    }
    let carriers = directions.len().min(MAX_BOX_SHADOW_CARRIERS);
    let unused = unused_direction(&directions);
    let camera_cascades = std::env::var_os("BG3_BOX_SHADOW_CASCADES").is_some();
    eprintln!(
        "lights: {} shadowed box lights along {} directions, {carriers} shadow carriers ({})",
        shadowed.len(),
        directions.len(),
        if camera_cascades {
            "camera cascades"
        } else {
            "one map per box"
        }
    );
    for _ in 0..carriers {
        commands.spawn((
            Name::new("box light shadow"),
            BoxShadowCarrier::default(),
            DirectionalLight {
                color: Color::BLACK,
                illuminance: 0.0,
                shadow_maps_enabled: true,
                ..default()
            },
            // Shadows end at the sun's shadow distance from the camera, as the sun's do.
            bevy::light::CascadeShadowConfigBuilder {
                num_cascades: if camera_cascades { 4 } else { 1 },
                maximum_distance: far_plane,
                ..default()
            }
            .build(),
            carrier_transform(unused, None),
        ));
    }
    commands.insert_resource(BoxShadows {
        boxes: shadowed,
        far_plane,
        unused,
        camera_cascades,
    });
}

/// A carrier shining along −`towards`, its map's up along `up` (default: world up).
fn carrier_transform(towards: Vec3, up: Option<Vec3>) -> Transform {
    let up = up.unwrap_or(if towards.y.abs() > 0.99 {
        Vec3::X
    } else {
        Vec3::Y
    });
    Transform::default().looking_to(-towards, up)
}

/// Directional lights left to box light shadows next to the sun.
const MAX_BOX_SHADOW_CARRIERS: usize = 9;

/// A carrier's boxes (indices into [`BoxShadows`]); empty when unused.
#[derive(Component, Default, PartialEq)]
pub struct BoxShadowCarrier {
    boxes: Vec<usize>,
}

impl BoxShadowCarrier {
    /// Whether the carrier shadows any box this frame.
    pub fn in_use(&self) -> bool {
        !self.boxes.is_empty()
    }
}

struct ShadowedBox {
    to_box: [Vec4; 3],
    size: Vec3,
    direction: Vec3,
    /// The box's local +Y (Bevy space), to align its map with the box.
    up: Vec3,
    /// The eight corners (Bevy space).
    corners: [Vec3; 8],
    /// Box coordinates (0–1) to Bevy space.
    world_from_box: bevy::math::Affine3A,
}

impl ShadowedBox {
    fn new(b: &GpuBoxLight) -> Self {
        let to_box = Mat4::from_cols(b.to_box[0], b.to_box[1], b.to_box[2], Vec4::W).transpose();
        let from_box = to_box.inverse();
        let corners = std::array::from_fn(|i| {
            from_box.transform_point3(Vec3::new(
                (i & 1) as f32,
                ((i >> 1) & 1) as f32,
                ((i >> 2) & 1) as f32,
            ))
        });
        Self {
            to_box: b.to_box,
            size: b.size,
            direction: b.direction_shadow.truncate(),
            up: from_box.transform_vector3(Vec3::Y).normalize_or(Vec3::Y),
            corners,
            world_from_box: bevy::math::Affine3A::from_mat4(from_box),
        }
    }

    /// Distance (m) from a Bevy-space point to the box (0 inside).
    fn distance(&self, p: Vec3) -> f32 {
        let p = p.extend(1.0);
        let u = Vec3::new(
            self.to_box[0].dot(p),
            self.to_box[1].dot(p),
            self.to_box[2].dot(p),
        );
        ((u - u.clamp(Vec3::ZERO, Vec3::ONE)) * self.size).length()
    }
}

#[derive(Resource)]
pub struct BoxShadows {
    boxes: Vec<ShadowedBox>,
    far_plane: f32,
    /// A direction no shadowed box has, for unused carriers.
    unused: Vec3,
    /// `BG3_BOX_SHADOW_CASCADES`: Bevy's camera cascades instead of one map per box.
    camera_cascades: bool,
}

/// A direction further than [`BOX_SHADOW_MATCH`] from all of `directions`.
fn unused_direction(directions: &[Vec3]) -> Vec3 {
    (0..64)
        .map(|i| {
            let (s, c) = (i as f32 * 2.399_963).sin_cos();
            let y = -1.0 + (i as f32 + 0.5) / 32.0;
            let r = (1.0 - y * y).sqrt();
            Vec3::new(r * c, y, r * s)
        })
        .find(|u| directions.iter().all(|d| d.dot(*u) < 0.999))
        .unwrap_or(Vec3::NEG_Y)
}

/// The viewer's camera, not a carrier.
type ViewCamera = (
    With<crate::fly_camera::FlyCamera>,
    Without<BoxShadowCarrier>,
);

/// Point the shadow carriers along the shadowed box lights nearest the camera (one per
/// direction) and give each the boxes along its direction; unused carriers point along
/// [`BoxShadows::unused`]. Boxes the camera can't see (outside its frustum, or beyond the shadow
/// distance) are skipped: none of their pixels is drawn. Runs after the camera's frustum is
/// updated and sets the carriers' global transforms itself (they have no parent), so the
/// cascades built next follow this frame's view. Writes only on change.
pub fn assign_box_shadow_carriers(
    shadows: Option<Res<BoxShadows>>,
    cameras: Query<(&GlobalTransform, &bevy::camera::primitives::Frustum), ViewCamera>,
    mut carriers: Query<(&mut BoxShadowCarrier, &mut Transform, &mut GlobalTransform)>,
) {
    let (Some(shadows), Some((camera, frustum))) = (shadows, cameras.iter().next()) else {
        return;
    };
    let eye = camera.translation();
    let unit = bevy::camera::primitives::Aabb {
        center: Vec3A::splat(0.5),
        half_extents: Vec3A::splat(0.5),
    };
    let mut near: Vec<(f32, usize)> = shadows
        .boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.distance(eye), i))
        .filter(|&(d, i)| {
            d <= shadows.far_plane
                && frustum.intersects_obb(&unit, &shadows.boxes[i].world_from_box, true, true)
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    // (direction, boxes along it, nearest first)
    let mut groups: Vec<(Vec3, Vec<usize>)> = Vec::new();
    for (_, i) in near {
        let b = &shadows.boxes[i];
        match groups
            .iter()
            .position(|g| g.0.dot(b.direction) > BOX_SHADOW_MATCH)
        {
            Some(g) => groups[g].1.push(i),
            None if groups.len() < MAX_BOX_SHADOW_CARRIERS => groups.push((b.direction, vec![i])),
            None => {}
        }
    }
    for (i, (mut carrier, mut transform, mut global)) in carriers.iter_mut().enumerate() {
        let (target, mut boxes) = match groups.get_mut(i) {
            Some((d, boxes)) => {
                // One map per box: aligned with the nearest box, for the most texels on it.
                let up = (!shadows.camera_cascades).then(|| shadows.boxes[boxes[0]].up);
                (carrier_transform(*d, up), std::mem::take(boxes))
            }
            None => (carrier_transform(shadows.unused, None), Vec::new()),
        };
        boxes.sort_unstable();
        if transform.rotation != target.rotation {
            *transform = target;
            *global = GlobalTransform::from(target);
        }
        if carrier.boxes != boxes {
            carrier.boxes = boxes;
        }
    }
}

/// Replace each carrier's cascade (Bevy fits it to the camera) with an orthographic projection
/// over its boxes in the carrier's frame; an unused carrier gets a 1 m map 100 km below the
/// level, which nothing falls in. Runs after Bevy builds the cascades and before it derives
/// the culling frusta from them.
pub fn fit_box_shadow_cascades(
    shadows: Option<Res<BoxShadows>>,
    map: Res<bevy::light::DirectionalLightShadowMap>,
    mut carriers: Query<(
        &GlobalTransform,
        &BoxShadowCarrier,
        &mut bevy::light::Cascades,
    )>,
) {
    let Some(shadows) = shadows.filter(|s| !s.camera_cascades) else {
        return;
    };
    let far_away = [Vec3::splat(-0.5), Vec3::splat(0.5)].map(|c| c - Vec3::Y * 1.0e5);
    for (transform, carrier, mut cascades) in &mut carriers {
        let points: Vec<Vec3> = if carrier.boxes.is_empty() {
            far_away.to_vec()
        } else {
            carrier
                .boxes
                .iter()
                .flat_map(|&i| shadows.boxes[i].corners)
                .collect()
        };
        let cascade = box_cascade(transform.rotation(), &points, map.size as f32);
        for view in cascades.cascades.values_mut() {
            view.clear();
            view.push(cascade.clone());
        }
    }
}

/// An orthographic, reverse-Z shadow cascade along the light (`rotation`'s −Z) that covers
/// `points`, with `size` texels a side, texel-snapped.
fn box_cascade(rotation: Quat, points: &[Vec3], size: f32) -> bevy::light::cascade::Cascade {
    let world_from_light = Mat4::from_quat(rotation);
    let light_from_world = world_from_light.transpose();
    let (lo, hi) = bounds(points.iter().map(|p| light_from_world.transform_point3(*p)));
    // Two texels of margin around the box; whole texels keep the projection stable.
    let extent = (hi - lo).truncate().max(Vec2::splat(0.01));
    let texel = extent / (size - 4.0);
    let centre = ((lo + hi).truncate() * 0.5 / texel).floor() * texel;
    let half = (extent * 0.5 + 2.0 * texel).extend(0.0);
    let (lo, hi) = (
        (centre.extend(lo.z - 0.01)) - half,
        (centre.extend(hi.z + 0.01)) + half,
    );
    let clip_from_cascade = ortho_clip_from_light(lo - hi.with_xy(centre), hi - hi.with_xy(centre));
    let cascade_from_world = Mat4::from_translation(-centre.extend(hi.z)) * light_from_world;
    bevy::light::cascade::Cascade {
        world_from_cascade: world_from_light * Mat4::from_translation(centre.extend(hi.z)),
        clip_from_cascade,
        clip_from_world: clip_from_cascade * cascade_from_world,
        texel_size: texel.max_element(),
    }
}

/// Limit each carrier cascade's shadow casters to its boxes (`BG3_BOX_SHADOW_CASCADES`): the
/// culling frustum becomes the cascade's, narrowed (in the light's frame) to the boxes'
/// footprint across the light and cut at their far face; casters towards the light stay (Bevy
/// doesn't cull at the near plane and clamps their depth). The projection is untouched, so the
/// map is the same where it matters. Cascades that miss every box, and unused carriers, cull
/// everything. Runs after Bevy derives the frusta from the cascades and before it culls.
pub fn cull_box_shadow_casters(
    shadows: Option<Res<BoxShadows>>,
    mut carriers: Query<(
        &GlobalTransform,
        &BoxShadowCarrier,
        &bevy::light::Cascades,
        &mut bevy::camera::primitives::CascadesFrusta,
    )>,
) {
    let Some(shadows) = shadows.filter(|s| s.camera_cascades) else {
        return;
    };
    for (transform, carrier, cascades, mut frusta) in &mut carriers {
        let light_from_world = Mat4::from_quat(transform.rotation()).transpose();
        // The boxes' bounds in the light's frame (+Z towards the light).
        let boxes = (!carrier.boxes.is_empty()).then(|| {
            bounds(
                carrier
                    .boxes
                    .iter()
                    .flat_map(|&i| shadows.boxes[i].corners)
                    .map(|p| light_from_world.transform_point3(p)),
            )
        });
        for (view, view_frusta) in frusta.frusta.iter_mut() {
            let Some(view_cascades) = cascades.cascades.get(view) else {
                continue;
            };
            for (frustum, cascade) in view_frusta.iter_mut().zip(view_cascades) {
                let world_from_clip = cascade.clip_from_world.inverse();
                let (lo, hi) = bounds((0..8).map(|i| {
                    let ndc = Vec3::new(
                        if i & 1 == 0 { -1.0 } else { 1.0 },
                        if i & 2 == 0 { -1.0 } else { 1.0 },
                        (i >> 2) as f32,
                    );
                    light_from_world.transform_point3(world_from_clip.project_point3(ndc))
                }));
                let culled = boxes.and_then(|(blo, bhi)| {
                    let lo = lo.max(blo);
                    let hi = hi.min(bhi).with_z(hi.z);
                    (lo.x < hi.x && lo.y < hi.y && lo.z < hi.z).then_some((lo, hi))
                });
                // Nothing to draw: a 1 m volume 100 km below the level.
                let (lo, hi) = culled.unwrap_or_else(|| {
                    let p = light_from_world.transform_point3(Vec3::NEG_Y * 1.0e5);
                    (p - 0.5, p + 0.5)
                });
                let clip = ortho_clip_from_light(lo, hi) * light_from_world;
                frustum.0 = bevy::shape::ViewFrustum::from_clip_from_world(&clip);
            }
        }
    }
}

/// Component-wise bounds of `points`.
fn bounds(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    points.fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    )
}

/// An orthographic, reverse-Z projection of the light-frame box `lo..hi` (looking along −Z).
fn ortho_clip_from_light(lo: Vec3, hi: Vec3) -> Mat4 {
    let size = (hi - lo).max(Vec3::splat(1e-3));
    let centre = (lo + hi) * 0.5;
    Mat4::from_cols(
        Vec4::new(2.0 / size.x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / size.y, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / size.z, 0.0),
        Vec4::new(
            -2.0 * centre.x / size.x,
            -2.0 * centre.y / size.y,
            1.0 - hi.z / size.z,
            1.0,
        ),
    )
}

/// Directions closer than this (cosine) share a shadow carrier; the shader uses the same test.
const BOX_SHADOW_MATCH: f32 = 0.99999;

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

/// The level's sun (its cascaded shadows: [`sun_shadow_stats`]).
#[derive(Component)]
pub struct Sun;

/// `BG3_SHADOW_STATS=1`: log, once a second, each sun cascade's texel size, its shadow casters
/// and how many of them cover less than 1, 2, 4 and 8 texels (bounding sphere diameter over the
/// texel size). Culling those small casters was tried and doesn't pay (`docs/performance.md`).
pub fn sun_shadow_stats(
    sun: Query<
        (
            &bevy::light::Cascades,
            &bevy::camera::visibility::CascadesVisibleEntities,
        ),
        With<Sun>,
    >,
    casters: Query<(&bevy::camera::primitives::Aabb, &GlobalTransform)>,
    time: Res<Time>,
    mut last_log: Local<f32>,
) {
    if std::env::var_os("BG3_SHADOW_STATS").is_none() || time.elapsed_secs() - *last_log < 1.0 {
        return;
    }
    *last_log = time.elapsed_secs();
    let diameter = |e: Entity| {
        casters.get(e).ok().map(|(aabb, t)| {
            let (scale, _, _) = t.to_scale_rotation_translation();
            2.0 * (Vec3::from(aabb.half_extents) * scale.abs()).length()
        })
    };
    for (cascades, visible) in &sun {
        for (view, lists) in &visible.entities {
            let Some(view_cascades) = cascades.cascades.get(view) else {
                continue;
            };
            for (i, (cascade, list)) in view_cascades.iter().zip(lists).enumerate() {
                let texel = cascade.texel_size;
                let mut under = [0usize; 4];
                for &e in &list.entities {
                    let d = diameter(e).unwrap_or(f32::INFINITY) / texel;
                    for (n, limit) in under.iter_mut().zip([1.0, 2.0, 4.0, 8.0]) {
                        *n += usize::from(d < limit);
                    }
                }
                eprintln!(
                    "sun cascade {i}: texel {texel:.3} m, {} casters, under 1/2/4/8 texels {under:?}",
                    list.entities.len(),
                );
            }
        }
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
                vignette: enabled,
                vignette_color: [x; 3],
                vignette_intensity: x,
                vignette_power: x,
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

#[cfg(test)]
mod box_light_tests {
    use super::*;

    fn light(rotation: [f32; 4]) -> Light {
        Light {
            id: "id".into(),
            name: "box".into(),
            object_type: "light".into(),
            template: None,
            transform: bg3_world::Transform {
                position: [10.0, 2.0, -5.0],
                rotation,
                scale: 3.0,
            },
            kind: LightKind::Directional,
            enabled: true,
            color: [0.5, 1.0, 0.25],
            intensity: 300.0,
            kelvin: 6500.0,
            use_temperature: false,
            radius: 15.0,
            angle: [35.0, 45.0],
            flat_falloff: false,
            gain: 1.0,
            light_channel: 191,
            shadow: true,
            pre_expose: true,
            is_flickering: false,
            direction_dimensions: [2.0, 4.0, 10.0],
            direction_attenuation: [3.0, 1.0, 0.5],
            direction_attenuation_function: 2,
            module: "Test".into(),
        }
    }

    /// Box coordinates of a BG3-space point.
    fn coords(b: &GpuBoxLight, bg3: [f32; 3]) -> Vec3 {
        let p = Vec3::from_array(space::vec3(bg3)).extend(1.0);
        Vec3::new(b.to_box[0].dot(p), b.to_box[1].dot(p), b.to_box[2].dot(p))
    }

    #[test]
    fn box_spans_half_sizes_and_depth_along_local_z_without_scale() {
        let b = box_light(&light([0.0, 0.0, 0.0, 1.0]), 1.0);
        // Origin = centre of the source face; the far face is `depth` along +Z.
        assert!((coords(&b, [10.0, 2.0, -5.0]) - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-5);
        assert!((coords(&b, [11.0, 4.0, 5.0]) - Vec3::new(1.0, 1.0, 1.0)).length() < 1e-5);
        assert!((coords(&b, [9.5, 2.0, 0.0]) - Vec3::new(0.25, 0.5, 0.5)).length() < 1e-5);
        // It shines along +Z: towards the light is BG3 −Z, Bevy +Z.
        assert!((b.direction_shadow.truncate() - Vec3::Z).length() < 1e-5);
        assert_eq!(b.direction_shadow.w, 1.0);
        assert_eq!(b.color_function, Vec4::new(150.0, 300.0, 75.0, 2.0));
        // Fades: start 3 of 10, end 1 of 10, sides 0.5.
        let z = b.attenuation_z;
        assert!((z - Vec4::new(0.3, 0.9, 1.0 / 0.3, 10.0)).length() < 1e-4);
        assert_eq!(b.attenuation_xy, Vec4::new(4.0, 8.0, 0.0, 0.0));
    }

    #[test]
    fn ortho_clip_maps_the_light_box_to_reverse_z_ndc() {
        let (lo, hi) = (Vec3::new(-2.0, 1.0, -30.0), Vec3::new(4.0, 3.0, -10.0));
        let clip = ortho_clip_from_light(lo, hi);
        assert!((clip.project_point3(lo) - Vec3::new(-1.0, -1.0, 0.0)).length() < 1e-5);
        assert!((clip.project_point3(hi) - Vec3::new(1.0, 1.0, 1.0)).length() < 1e-5);
        // A frustum from it keeps points inside and rejects those beside or beyond the box.
        let f = bevy::shape::ViewFrustum::from_clip_from_world(&clip);
        let inside = bevy::camera::primitives::Aabb {
            center: Vec3A::new(1.0, 2.0, -20.0),
            half_extents: Vec3A::splat(0.1),
        };
        let beside = bevy::camera::primitives::Aabb {
            center: Vec3A::new(6.0, 2.0, -20.0),
            ..inside
        };
        let beyond = bevy::camera::primitives::Aabb {
            center: Vec3A::new(1.0, 2.0, -40.0),
            ..inside
        };
        let id = bevy::math::Affine3A::IDENTITY;
        let frustum = bevy::camera::primitives::Frustum(f);
        assert!(frustum.intersects_obb(&inside, &id, false, true));
        assert!(!frustum.intersects_obb(&beside, &id, false, true));
        assert!(!frustum.intersects_obb(&beyond, &id, false, true));
    }

    #[test]
    fn box_cascade_covers_the_box_in_reverse_z() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let b = ShadowedBox::new(&box_light(&light([0.0, h, 0.0, h]), 1.0));
        let rotation = carrier_transform(b.direction, Some(b.up)).rotation;
        let c = box_cascade(rotation, &b.corners, 2048.0);
        for p in b.corners {
            let ndc = c.clip_from_world.project_point3(p);
            assert!(ndc.x.abs() < 1.0 && ndc.y.abs() < 1.0, "{ndc}");
            assert!((0.0..=1.0).contains(&ndc.z), "{ndc}");
        }
        // Aligned with the box: 2 × 4 m across, 2048 texels with two of margin each side.
        assert!((c.texel_size - 4.0 / 2044.0).abs() < 1e-6);
        assert!((2.0 / c.clip_from_cascade.y_axis.y - 2048.0 * c.texel_size).abs() < 1e-4);
        // The source face (towards the light) is near (z ≈ 1), the far face far (z ≈ 0).
        let source = Vec3::from_array(space::vec3([10.0, 2.0, -5.0]));
        let far = Vec3::from_array(space::vec3([20.0, 2.0, -5.0]));
        assert!(c.clip_from_world.project_point3(source).z > 0.99);
        assert!(c.clip_from_world.project_point3(far).z < 0.01);
        // `world_from_cascade` and `clip_from_cascade` compose to `clip_from_world`.
        let composed = c.clip_from_cascade * c.world_from_cascade.inverse();
        assert!(composed.abs_diff_eq(c.clip_from_world, 1e-4));
    }

    #[test]
    fn unused_direction_matches_no_box() {
        let dirs = [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::new(0.0, -0.6, 0.8)];
        let u = unused_direction(&dirs);
        assert!((u.length() - 1.0).abs() < 1e-4);
        assert!(dirs.iter().all(|d| d.dot(u) < BOX_SHADOW_MATCH));
    }

    #[test]
    fn rotated_box_follows_the_light_axis() {
        // 90° about Y: local +Z is BG3 +X.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let b = box_light(&light([0.0, h, 0.0, h]), 1.0);
        assert!((coords(&b, [15.0, 2.0, -5.0]) - Vec3::new(0.5, 0.5, 0.5)).length() < 1e-4);
        assert!((b.direction_shadow.truncate() - Vec3::NEG_X).length() < 1e-4);
        let shadowed = ShadowedBox::new(&b);
        assert!((shadowed.direction - Vec3::NEG_X).length() < 1e-4);
        // The corners span the box: the source face at BG3 x = 10, the far face at x = 20.
        let x = shadowed.corners.map(|c| c.x);
        assert!((x.iter().copied().fold(f32::MAX, f32::min) - 10.0).abs() < 1e-3);
        assert!((x.iter().copied().fold(f32::MIN, f32::max) - 20.0).abs() < 1e-3);
        let inside = Vec3::from_array(space::vec3([15.0, 2.0, -5.0]));
        let beyond = Vec3::from_array(space::vec3([23.0, 2.0, -5.0]));
        assert!(shadowed.distance(inside) < 1e-4);
        assert!((shadowed.distance(beyond) - 3.0).abs() < 1e-3);
    }
}
