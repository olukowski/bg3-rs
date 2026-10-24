//! Lights: placed light objects (`Lights/`, `CombinedLights/`) and the level-wide lighting
//! (`meta.lsf` → `LightingBank` / `AtmosphereBank`). See `docs/research/lighting.md`.
//!
//! Light objects are *sparse*: a level object only stores the fields that differ from its root
//! template, which in turn only stores what differs from its `ParentTemplateId`. Resolving a
//! light therefore walks object → template → parent → … and takes the first value found.

use std::collections::HashMap;

use bg3_lsf::Node;
use bg3_vfs::Vfs;
use bg3_vfs::modules::BASE_LOAD_ORDER;

use crate::level::read_transform;
use crate::{Result, RootTemplates, Transform, parse, str_attr};

/// Per-level directories holding light objects.
pub const LIGHT_DIRS: &[&str] = &["Lights", "CombinedLights"];

/// `LightType`. Mapping inferred from template names and which fields are set
/// (`Angle` on spots, `DirectionLightDimensions` on directional lights); see the research note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    Point,
    Spot,
    /// A local, box-shaped directional light (`DirectionLightDimensions`), not a sun.
    Directional,
    Unknown(u8),
}

impl LightKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Point,
            1 => Self::Spot,
            2 => Self::Directional,
            v => Self::Unknown(v),
        }
    }
}

/// Light fields as stored on one node; `None` = not set here (inherit).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightProps {
    pub light_type: Option<u8>,
    pub enabled: Option<bool>,
    pub color: Option<[f32; 3]>,
    pub intensity: Option<f32>,
    pub kelvin: Option<f32>,
    pub use_temperature: Option<bool>,
    pub radius: Option<f32>,
    /// Spot cone angles in degrees (`x` inner, `y` outer — assumed).
    pub angle: Option<[f32; 2]>,
    pub flat_falloff: Option<bool>,
    pub gain: Option<f32>,
    pub light_channel: Option<u8>,
    pub shadow: Option<bool>,
    pub pre_expose: Option<bool>,
    pub is_flickering: Option<bool>,
    pub direction_dimensions: Option<[f32; 3]>,
    pub direction_attenuation_start: Option<f32>,
    pub direction_attenuation_end: Option<f32>,
    pub direction_attenuation_side: Option<f32>,
    pub direction_attenuation_function: Option<u8>,
}

fn f32_attr(n: &Node, name: &str) -> Option<f32> {
    n.attr(name).and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn bool_attr(n: &Node, name: &str) -> Option<bool> {
    n.attr(name).and_then(|v| v.as_bool())
}

fn vec_attr<const N: usize>(n: &Node, name: &str) -> Option<[f32; N]> {
    let c = n.attr(name)?.components()?;
    let mut out = [0.0; N];
    if c.len() != N {
        return None;
    }
    for (o, v) in out.iter_mut().zip(c) {
        *o = v as f32;
    }
    Some(out)
}

impl LightProps {
    pub fn read(n: &Node) -> Self {
        Self {
            light_type: n
                .attr("LightType")
                .and_then(|v| v.as_i64())
                .map(|v| v as u8),
            enabled: bool_attr(n, "Enabled"),
            color: vec_attr(n, "Color"),
            intensity: f32_attr(n, "Intensity"),
            kelvin: f32_attr(n, "Kelvin"),
            use_temperature: bool_attr(n, "UseTemperature"),
            radius: f32_attr(n, "Radius"),
            angle: vec_attr(n, "Angle"),
            flat_falloff: bool_attr(n, "FlatFalloff"),
            gain: f32_attr(n, "Gain"),
            light_channel: n
                .attr("LightChannelFlag")
                .and_then(|v| v.as_i64())
                .map(|v| v as u8),
            shadow: bool_attr(n, "Shadow"),
            pre_expose: bool_attr(n, "PreExpose"),
            is_flickering: bool_attr(n, "IsFlickering"),
            direction_dimensions: vec_attr(n, "DirectionLightDimensions"),
            direction_attenuation_start: f32_attr(n, "DirectionLightAttenuationStart"),
            direction_attenuation_end: f32_attr(n, "DirectionLightAttenuationEnd"),
            direction_attenuation_side: f32_attr(n, "DirectionLightAttenuationSide"),
            direction_attenuation_function: n
                .attr("DirectionLightAttenuationFunction")
                .and_then(|v| v.as_i64())
                .map(|v| v as u8),
        }
    }

    /// Fill unset fields from `parent`.
    pub fn inherit(self, parent: &Self) -> Self {
        Self {
            light_type: self.light_type.or(parent.light_type),
            enabled: self.enabled.or(parent.enabled),
            color: self.color.or(parent.color),
            intensity: self.intensity.or(parent.intensity),
            kelvin: self.kelvin.or(parent.kelvin),
            use_temperature: self.use_temperature.or(parent.use_temperature),
            radius: self.radius.or(parent.radius),
            angle: self.angle.or(parent.angle),
            flat_falloff: self.flat_falloff.or(parent.flat_falloff),
            gain: self.gain.or(parent.gain),
            light_channel: self.light_channel.or(parent.light_channel),
            shadow: self.shadow.or(parent.shadow),
            pre_expose: self.pre_expose.or(parent.pre_expose),
            is_flickering: self.is_flickering.or(parent.is_flickering),
            direction_dimensions: self.direction_dimensions.or(parent.direction_dimensions),
            direction_attenuation_start: self
                .direction_attenuation_start
                .or(parent.direction_attenuation_start),
            direction_attenuation_end: self
                .direction_attenuation_end
                .or(parent.direction_attenuation_end),
            direction_attenuation_side: self
                .direction_attenuation_side
                .or(parent.direction_attenuation_side),
            direction_attenuation_function: self
                .direction_attenuation_function
                .or(parent.direction_attenuation_function),
        }
    }
}

/// A placed light with all fields resolved through its template chain.
#[derive(Debug, Clone)]
pub struct Light {
    pub id: String,
    pub name: String,
    /// `light` or `CombinedLight` (a render light bundled with a gameplay light).
    pub object_type: String,
    pub template: Option<String>,
    pub transform: Transform,
    pub kind: LightKind,
    pub enabled: bool,
    /// Linear RGB filter colour.
    pub color: [f32; 3],
    pub intensity: f32,
    /// Colour temperature, applied on top of `color` when `use_temperature` is set.
    pub kelvin: f32,
    pub use_temperature: bool,
    pub radius: f32,
    /// Degrees.
    pub angle: [f32; 2],
    /// The engine's "fill light": constant `4/R²` in place of the inverse square.
    pub flat_falloff: bool,
    /// Added to the inverse square (1/m²).
    pub gain: f32,
    /// Light channels the light reaches (bit per G-buffer channel).
    pub light_channel: u8,
    pub shadow: bool,
    pub pre_expose: bool,
    pub is_flickering: bool,
    pub direction_dimensions: [f32; 3],
    /// Box light fade lengths (m): start (from the source face), end, sides.
    pub direction_attenuation: [f32; 3],
    /// Box light fade curve: 0 linear, 1 square, 2 smoothstep, 3 smootherstep.
    pub direction_attenuation_function: u8,
    pub module: String,
}

impl Light {
    fn resolve(
        id: String,
        name: String,
        object_type: String,
        template: Option<String>,
        transform: Transform,
        props: LightProps,
        module: String,
    ) -> Self {
        Self {
            kind: LightKind::from_u8(props.light_type.unwrap_or(0)),
            enabled: props.enabled.unwrap_or(true),
            color: props.color.unwrap_or([1.0; 3]),
            intensity: props.intensity.unwrap_or(0.0),
            kelvin: props.kelvin.unwrap_or(6445.8965),
            use_temperature: props.use_temperature.unwrap_or(false),
            radius: props.radius.unwrap_or(1.0),
            angle: props.angle.unwrap_or([35.0, 45.0]),
            flat_falloff: props.flat_falloff.unwrap_or(false),
            gain: props.gain.unwrap_or(0.0),
            light_channel: props.light_channel.unwrap_or(0xff),
            shadow: props.shadow.unwrap_or(false),
            pre_expose: props.pre_expose.unwrap_or(false),
            is_flickering: props.is_flickering.unwrap_or(false),
            direction_dimensions: props.direction_dimensions.unwrap_or([1.0; 3]),
            direction_attenuation: [
                props.direction_attenuation_start.unwrap_or(0.0),
                props.direction_attenuation_end.unwrap_or(0.0),
                props.direction_attenuation_side.unwrap_or(0.0),
            ],
            direction_attenuation_function: props.direction_attenuation_function.unwrap_or(0),
            id,
            name,
            object_type,
            template,
            transform,
            module,
        }
    }

    /// Effective linear colour: `color`, tinted by the black-body colour of `kelvin` if enabled.
    pub fn effective_color(&self) -> [f32; 3] {
        if self.use_temperature {
            let k = kelvin_to_rgb(self.kelvin);
            [
                self.color[0] * k[0],
                self.color[1] * k[1],
                self.color[2] * k[2],
            ]
        } else {
            self.color
        }
    }
}

/// Template-chain lookup for light fields.
pub fn template_light_props(templates: &RootTemplates, id: &str) -> LightProps {
    let mut acc = LightProps::default();
    let mut current = templates.get(id);
    for _ in 0..32 {
        let Some(t) = current else { break };
        if let Some(p) = &t.light {
            acc = acc.inherit(p);
        }
        current = t.parent.as_deref().and_then(|p| templates.get(p));
    }
    acc
}

/// Read the light objects of one `_merged.lsf` resource.
pub fn read_lights(res: &bg3_lsf::Resource, module: &str, templates: &RootTemplates) -> Vec<Light> {
    res.region("Templates")
        .into_iter()
        .flat_map(|n| n.children.iter().filter(|c| c.name == "GameObjects"))
        .filter_map(|n| {
            let id = str_attr(n, "MapKey")?;
            let template = str_attr(n, "TemplateName");
            let inherited = template
                .as_deref()
                .map(|t| template_light_props(templates, t))
                .unwrap_or_default();
            Some(Light::resolve(
                id,
                str_attr(n, "Name").unwrap_or_default(),
                str_attr(n, "Type").unwrap_or_default(),
                template,
                n.child("Transform").map(read_transform).unwrap_or_default(),
                LightProps::read(n).inherit(&inherited),
                module.to_owned(),
            ))
        })
        .collect()
}

/// All lights of a level, merged across modules (later modules replace by `MapKey`).
pub fn load_level_lights(vfs: &Vfs, level: &str, templates: &RootTemplates) -> Result<Vec<Light>> {
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Light> = Vec::new();
    for module in BASE_LOAD_ORDER {
        for dir in LIGHT_DIRS {
            let path = format!("Mods/{module}/Levels/{level}/{dir}/_merged.lsf");
            if !vfs.exists(&path) {
                continue;
            }
            let res = parse(vfs, &path)?;
            for l in read_lights(&res, module, templates) {
                match by_id.get(&l.id) {
                    Some(&i) => out[i] = l,
                    None => {
                        by_id.insert(l.id.clone(), out.len());
                        out.push(l);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The engine's colour for a colour temperature: linear Rec.709 RGB with unit luminance (so it
/// tints without changing brightness; components can exceed 1).
///
/// The chromaticity is the Planckian locus (rational fits in CIE 1960 uv) blended into the CIE
/// daylight locus around 13000 K by a logistic weight, then converted from xyY (Y = 1) to linear
/// Rec.709 and divided by its luminance.
// bg3: ls::blackbody (4.1.1.7631656)
pub fn kelvin_to_rgb(kelvin: f32) -> [f32; 3] {
    let t = f64::from(kelvin.max(1.0));
    let u = (0.860117757 + 1.54118254e-4 * t + 1.28641212e-7 * t * t)
        / (1.0 + 8.42420235e-4 * t + 7.08145163e-7 * t * t);
    let v = (0.317398726 + 4.22806245e-5 * t + 4.20481691e-8 * t * t)
        / (1.0 - 2.89741816e-5 * t + 1.61456053e-7 * t * t);
    let d = 2.0 * u - 8.0 * v + 4.0;
    let (xp, yp) = (3.0 * u / d, 2.0 * v / d);
    let xd = 0.24039 + 222.6347 / t + 2107038.0 / (t * t) - 3025846784.0 / (t * t * t);
    let yd = 3.081758 * xd * xd * xd - 5.873387 * xd * xd + 3.751130 * xd - 0.370015;
    let s = (13000.0 - t) / 256.0;
    let w = if s < -18.0 {
        0.0
    } else if s > 18.0 {
        1.0
    } else {
        0.5 + 0.5 * (s.exp() - 1.0) / (s.exp() + 1.0)
    };
    let x = w * xp + (1.0 - w) * xd;
    let y = w * yp + (1.0 - w) * yd;
    let (cx, cz) = (x / y, (1.0 - x - y) / y);
    let r = (3.2404542 * cx - 1.5371385 - 0.4985314 * cz).max(0.0);
    let g = (-0.9692660 * cx + 1.8760108 + 0.0415560 * cz).max(0.0);
    let b = (0.0556434 * cx - 0.2040259 + 1.0572252 * cz).max(0.0);
    let lum = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    [(r / lum) as f32, (g / lum) as f32, (b / lum) as f32]
}

/// Gamma-encoded colours of the lighting bank (sun, moon, sky light): the engine converts them
/// with a plain 2.2 power.
// bg3: ls::Lighting::Visit (4.1.1.7631656)
pub fn decode_gamma(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| v.max(0.0).powf(2.2))
}

/// The level's sun (from `LightingBank` → `Lighting/Sun`).
#[derive(Debug, Clone, PartialEq)]
pub struct Sun {
    /// Degrees.
    pub yaw: f32,
    /// Degrees above the horizon.
    pub pitch: f32,
    pub color: [f32; 3],
    /// Illuminance (assumed lux).
    pub intensity: f32,
    pub kelvin: f32,
    pub use_temperature: bool,
    pub shadow: bool,
    /// Distance covered by the sun's shadow cascades (m).
    pub shadow_far_plane: f32,
    /// Nearest distance of the first cascade (m).
    pub shadow_near_plane: f32,
    /// Number of cascades, 1–4.
    pub cascade_count: u8,
    /// Blend of the cascade splits between uniform (0) and logarithmic (1).
    pub cascade_spread: f32,
    /// How far towards the sun each cascade's shadow camera sits (m).
    pub light_distance: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub kelvin: f32,
    pub use_temperature: bool,
    /// Atmospheric scattering on: the sun then lights surfaces with `scattering_sun_color ·
    /// scattering_sun_intensity` (× the atmosphere's transmittance) instead of its own colour.
    pub scattering: bool,
    pub scattering_intensity: f32,
    pub scattering_sun_color: [f32; 3],
    pub scattering_sun_intensity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FogLayer {
    pub enabled: bool,
    pub albedo: [f32; 3],
    /// Densities at `height0` and `height1`.
    pub density: [f32; 2],
    pub height: [f32; 2],
}

/// Level-wide lighting, from the `LevelLighting` and `LevelAtmosphere` resources named in the
/// level's `meta.lsf`.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelLighting {
    pub lighting_id: Option<String>,
    pub lighting_name: String,
    pub atmosphere_id: Option<String>,
    pub atmosphere_name: String,
    pub sun: Option<Sun>,
    pub sky: Option<SkyLight>,
    pub fog: Vec<FogLayer>,
    pub fog_render_distance: Option<f32>,
    /// Henyey-Greenstein asymmetry of the volumetric fog (`Fog/Phase`).
    pub fog_phase: Option<f32>,
    /// `(ExposureMin, ExposureMax, ExposureCompensation)` of the first `ExposureSettings` (EV100).
    pub exposure: Option<[f32; 3]>,
    /// Every `ExposureSettings` entry of the atmosphere, in file order.
    pub exposure_settings: Vec<ExposureSettings>,
    /// The atmosphere's colour grading and bloom (`PostProcessing`).
    pub post: PostProcessing,
    /// `LevelBounds` from `meta.lsf` (min, max).
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// Names of the lighting / atmosphere triggers that chose the resources (see
    /// [`LevelLighting::load_at`]), if any.
    pub zones: [Option<String>; 2],
}

/// One `PostProcessing/ExposureSettings/Object` of an atmosphere.
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureSettings {
    /// Auto-exposure on.
    pub enabled: bool,
    pub min: f32,
    pub max: f32,
    pub compensation: f32,
    /// Histogram percentiles kept for metering.
    pub min_percentile: f32,
    pub max_percentile: f32,
    pub metering_mode: i64,
}

/// One range of `NewColorCorrection` (`GlobalRange`, `ShadowRange`, …).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradingRange {
    pub exposure_shift: f32,
    pub saturation: f32,
    pub xy_offset: [f32; 2],
}

impl Default for GradingRange {
    fn default() -> Self {
        Self {
            exposure_shift: 0.0,
            saturation: 1.0,
            xy_offset: [0.0; 2],
        }
    }
}

/// Colour grading and bloom settings of an atmosphere (`PostProcessing`).
#[derive(Debug, Clone, PartialEq)]
pub struct PostProcessing {
    pub bloom: bool,
    /// Bloom `Threshold` and `Amount`: the threshold pass weights each texel by
    /// `sat(Amount · (luma − Threshold))` (`docs/research/lighting.md`, "Bloom").
    pub bloom_threshold: f32,
    pub bloom_amount: f32,
    /// `Vignette`: the display output is mixed towards `VignetteColor` by
    /// `sat(VignetteIntensity · ‖|2(uv − ½)|^VignettePower‖)` (`docs/research/lighting.md`).
    pub vignette: bool,
    pub vignette_color: [f32; 3],
    pub vignette_intensity: f32,
    pub vignette_power: f32,
    pub white_balance_temperature: f32,
    pub white_balance_tint: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub hue: f32,
    pub pivot: f32,
    /// Ranges in the order global, dark, shadow, midtones, highlight, specular.
    pub ranges: [GradingRange; 6],
}

/// `NewColorCorrection` range node names, in [`PostProcessing::ranges`] order.
pub const GRADING_RANGES: [&str; 6] = [
    "GlobalRange",
    "DarkRange",
    "ShadowRange",
    "MidtonesRange",
    "HighlightRange",
    "SpecularRange",
];

impl Default for PostProcessing {
    fn default() -> Self {
        Self {
            // The engine's defaults when an atmosphere omits them.
            // bg3: ls::Atmosphere::Visit (4.1.1.7631656)
            bloom: false,
            bloom_threshold: 0.4,
            bloom_amount: 1.5,
            vignette: false,
            vignette_color: [0.0; 3],
            vignette_intensity: 0.5,
            vignette_power: 7.0,
            white_balance_temperature: 6500.0,
            white_balance_tint: 0.0,
            contrast: 1.0,
            highlights: 0.0,
            shadows: 0.0,
            saturation: 1.0,
            hue: 0.0,
            pivot: 0.0,
            ranges: [GradingRange::default(); 6],
        }
    }
}

impl PostProcessing {
    fn read(p: &Node) -> Self {
        let d = Self::default();
        let f = |n: &Node, name: &str, default: f32| f32_attr(n, name).unwrap_or(default);
        let mut out = Self {
            bloom: bool_attr(p, "Bloom").unwrap_or(false),
            bloom_threshold: f(p, "Threshold", d.bloom_threshold),
            bloom_amount: f(p, "Amount", d.bloom_amount),
            vignette: bool_attr(p, "Vignette").unwrap_or(false),
            vignette_color: vec_attr(p, "VignetteColor").unwrap_or(d.vignette_color),
            vignette_intensity: f(p, "VignetteIntensity", d.vignette_intensity),
            vignette_power: f(p, "VignettePower", d.vignette_power),
            white_balance_temperature: f(p, "WhiteBalanceTemperature", 6500.0),
            white_balance_tint: f(p, "WhiteBalanceTint", 0.0),
            ..d
        };
        if let Some(c) = p.child("NewColorCorrection") {
            out.contrast = f(c, "Contrast", 1.0);
            out.highlights = f(c, "Highlights", 0.0);
            out.shadows = f(c, "Shadows", 0.0);
            out.saturation = f(c, "Saturation", 1.0);
            out.hue = f(c, "Hue", 0.0);
            out.pivot = f(c, "Pivot", 0.0);
            for (range, name) in out.ranges.iter_mut().zip(GRADING_RANGES) {
                if let Some(r) = c.child(name) {
                    *range = GradingRange {
                        exposure_shift: f(r, "ExposureShift", 0.0),
                        saturation: f(r, "Saturation", 1.0),
                        xy_offset: vec_attr(r, "XYOffset").unwrap_or([0.0; 2]),
                    };
                }
            }
        }
        out
    }
}

impl ExposureSettings {
    fn read(o: &Node) -> Option<Self> {
        Some(Self {
            enabled: bool_attr(o, "Exposure").unwrap_or(true),
            min: f32_attr(o, "ExposureMin")?,
            max: f32_attr(o, "ExposureMax")?,
            compensation: f32_attr(o, "ExposureCompensation").unwrap_or(0.0),
            min_percentile: f32_attr(o, "ExposureMinPercentile").unwrap_or(0.0),
            max_percentile: f32_attr(o, "ExposureMaxPercentile").unwrap_or(1.0),
            metering_mode: o
                .attr("ExposureMeteringMode")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
        })
    }
}

/// Everything [`LevelLighting`] is resolved from: the level's default resources, the
/// lighting/atmosphere banks and the level's lighting and atmosphere triggers. Kept so a
/// front end can re-resolve the lighting as its camera moves between triggers.
#[derive(Debug, Clone)]
pub struct LightingSources {
    lighting_id: Option<String>,
    atmosphere_id: Option<String>,
    bounds: Option<([f32; 3], [f32; 3])>,
    banks: Vec<bg3_lsf::Resource>,
    /// The level's lighting and atmosphere triggers.
    pub zones: Vec<crate::atmosphere::ResourceZone>,
    /// Per zone: its first resource is a lighting (`[0]`) / atmosphere (`[1]`) resource.
    kinds: Vec<[bool; 2]>,
}

impl LightingSources {
    /// Read `meta.lsf` (last module wins), the banks and the level's triggers.
    pub fn load(vfs: &Vfs, level: &str) -> Result<Self> {
        let mut meta = None;
        for module in BASE_LOAD_ORDER {
            let path = format!("Mods/{module}/Levels/{level}/meta.lsf");
            if vfs.exists(&path) {
                meta = Some(parse(vfs, &path)?);
            }
        }
        let data = meta.as_ref().and_then(|m| m.region("LevelData"));
        let banks = atmosphere_banks(vfs)?;
        let zones = crate::atmosphere::load_resource_zones(vfs, level)?;
        let has = |region: &str, id: &str| {
            banks.iter().any(|b| {
                b.region(region).is_some_and(|r| {
                    r.children_named("Resource")
                        .any(|x| x.attr_str("ID") == Some(id))
                })
            })
        };
        let kinds = zones
            .iter()
            .map(|z| {
                let first = z.resources.first().map(String::as_str);
                ["LightingBank", "AtmosphereBank"].map(|b| first.is_some_and(|r| has(b, r)))
            })
            .collect();
        Ok(Self {
            lighting_id: data.and_then(|d| str_attr(d, "LevelLighting")),
            atmosphere_id: data.and_then(|d| str_attr(d, "LevelAtmosphere")),
            bounds: data
                .and_then(|d| d.child("LevelBounds"))
                .and_then(|b| Some((vec_attr::<3>(b, "Min")?, vec_attr::<3>(b, "Max")?))),
            banks,
            zones,
            kinds,
        })
    }

    /// The lighting (`[0]`) and atmosphere (`[1]`) triggers in effect with the camera at
    /// `at` (indices into [`zones`](Self::zones)); `None` where the level's own resource
    /// applies. The engine tests the camera's eye position, not the player's (both for the
    /// game camera and for timeline cameras).
    // bg3: ecl::CameraSystem::CheckTriggers, ecl::TimelineCameraSystem::UpdateCameras (4.1.1.7631656)
    pub fn zones_at(&self, at: Option<[f32; 3]>) -> [Option<usize>; 2] {
        let Some(p) = at else {
            return [None, None];
        };
        [0, 1].map(|k| crate::atmosphere::zone_index_at(&self.zones, p, |i, _| self.kinds[i][k]))
    }

    /// The level lighting with the given triggers' first resources (see
    /// [`zones_at`](Self::zones_at)).
    pub fn resolve(&self, zones: [Option<usize>; 2]) -> LevelLighting {
        let zone = |k: usize| zones[k].map(|i| &self.zones[i]);
        let mut out = LevelLighting {
            lighting_id: zone(0)
                .map(|z| z.resources[0].clone())
                .or_else(|| self.lighting_id.clone()),
            lighting_name: String::new(),
            atmosphere_id: zone(1)
                .map(|z| z.resources[0].clone())
                .or_else(|| self.atmosphere_id.clone()),
            atmosphere_name: String::new(),
            sun: None,
            sky: None,
            fog: Vec::new(),
            fog_render_distance: None,
            fog_phase: None,
            exposure: None,
            exposure_settings: Vec::new(),
            post: PostProcessing::default(),
            bounds: self.bounds,
            zones: [
                zone(0).map(|z| z.name.clone()),
                zone(1).map(|z| z.name.clone()),
            ],
        };
        for res in &self.banks {
            out.apply_bank(res);
        }
        out
    }
}

impl LevelLighting {
    /// Read `meta.lsf` (last module wins) and resolve its lighting/atmosphere resources.
    pub fn load(vfs: &Vfs, level: &str) -> Result<Self> {
        Self::load_at(vfs, level, None)
    }

    /// Like [`load`](Self::load), with the lighting and atmosphere triggers containing the
    /// camera position `at` (BG3 world space) replacing the level's resources (see
    /// [`LightingSources::zones_at`]). Triggers fade between resources as the camera moves;
    /// this picks one place.
    pub fn load_at(vfs: &Vfs, level: &str, at: Option<[f32; 3]>) -> Result<Self> {
        let sources = LightingSources::load(vfs, level)?;
        Ok(sources.resolve(sources.zones_at(at)))
    }

    /// Take the lighting and atmosphere resources this level refers to from a bank file.
    pub fn apply_bank(&mut self, res: &bg3_lsf::Resource) {
        let find = |region: &str, id: &Option<String>| -> Option<Node> {
            let id = id.as_deref()?;
            res.region(region)?
                .children_named("Resource")
                .find(|r| r.attr_str("ID") == Some(id))
                .cloned()
        };
        if let Some(r) = find("LightingBank", &self.lighting_id) {
            self.lighting_name = str_attr(&r, "Name").unwrap_or_default();
            if let Some(l) = r.child("Lighting") {
                self.read_lighting(l);
            }
        }
        if let Some(r) = find("AtmosphereBank", &self.atmosphere_id) {
            self.atmosphere_name = str_attr(&r, "Name").unwrap_or_default();
            self.exposure_settings = r
                .child("Atmosphere")
                .and_then(|a| a.child("PostProcessing"))
                .into_iter()
                .flat_map(|p| p.children_named("ExposureSettings"))
                .filter_map(|e| e.child("Object"))
                .filter_map(ExposureSettings::read)
                .collect();
            self.exposure = self
                .exposure_settings
                .first()
                .map(|e| [e.min, e.max, e.compensation]);
            self.post = r
                .child("Atmosphere")
                .and_then(|a| a.child("PostProcessing"))
                .map(PostProcessing::read)
                .unwrap_or_default();
        }
    }

    fn read_lighting(&mut self, l: &Node) {
        self.sun = l.child("Sun").map(|s| Sun {
            yaw: f32_attr(s, "Yaw").unwrap_or(0.0),
            pitch: f32_attr(s, "Pitch").unwrap_or(45.0),
            color: vec_attr(s, "SunColor").unwrap_or([1.0; 3]),
            intensity: f32_attr(s, "SunIntensity").unwrap_or(0.0),
            kelvin: f32_attr(s, "Kelvin").unwrap_or(6445.8965),
            use_temperature: bool_attr(s, "UseTemperature").unwrap_or(false),
            shadow: bool_attr(s, "ShadowEnabled").unwrap_or(false),
            shadow_far_plane: f32_attr(s, "ShadowFarPlane").unwrap_or(100.0),
            shadow_near_plane: f32_attr(s, "ShadowNearPlane").unwrap_or(0.1),
            cascade_count: f32_attr(s, "CascadeCount").map_or(4, |v| v as u8),
            cascade_spread: f32_attr(s, "CascadeSpread").unwrap_or(0.0),
            light_distance: f32_attr(s, "LightDistance").unwrap_or(250.0),
        });
        self.sky = l.child("SkyLight").map(|s| SkyLight {
            color: vec_attr(s, "Color").unwrap_or([1.0; 3]),
            intensity: f32_attr(s, "Intensity").unwrap_or(0.0),
            kelvin: f32_attr(s, "Kelvin").unwrap_or(6445.8965),
            use_temperature: bool_attr(s, "UseTemperature").unwrap_or(false),
            scattering: bool_attr(s, "ScatteringEnabled").unwrap_or(false),
            scattering_intensity: f32_attr(s, "ScatteringIntensity").unwrap_or(0.0),
            scattering_sun_color: vec_attr(s, "ScatteringSunColor").unwrap_or([1.0; 3]),
            scattering_sun_intensity: f32_attr(s, "ScatteringSunIntensity").unwrap_or(0.0),
        });
        if let Some(fog) = l.child("Fog") {
            self.fog_render_distance = f32_attr(fog, "RenderDistance");
            self.fog_phase = f32_attr(fog, "Phase");
            self.fog = ["FogLayer0", "FogLayer1"]
                .iter()
                .filter_map(|name| fog.child(name))
                .map(|f| FogLayer {
                    enabled: bool_attr(f, "Enabled").unwrap_or(false),
                    albedo: vec_attr(f, "Albedo").unwrap_or([1.0; 3]),
                    density: [
                        f32_attr(f, "Density0").unwrap_or(0.0),
                        f32_attr(f, "Density1").unwrap_or(0.0),
                    ],
                    height: [
                        f32_attr(f, "Height0").unwrap_or(0.0),
                        f32_attr(f, "Height1").unwrap_or(0.0),
                    ],
                })
                .collect();
        }
    }
}

/// Bank files with lighting/atmosphere resources (`Public/<Module>/Content/**/Atmospheres`).
fn atmosphere_banks(vfs: &Vfs) -> Result<Vec<bg3_lsf::Resource>> {
    let mut out = Vec::new();
    for module in BASE_LOAD_ORDER {
        let prefix = format!("Public/{module}/Content/");
        let mut paths: Vec<String> = vfs
            .files_under(&prefix)
            .map(|f| f.entry.name.clone())
            .filter(|n| n.contains("Atmospheres") && n.ends_with("_merged.lsf"))
            .collect();
        paths.sort();
        for p in paths {
            out.push(parse(vfs, &p)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootTemplate;
    use bg3_lsf::Value;

    fn template(id: &str, parent: Option<&str>, light: Node) -> RootTemplate {
        RootTemplate {
            id: id.into(),
            name: id.into(),
            kind: "light".into(),
            parent: parent.map(Into::into),
            visual_template: None,
            physics_template: None,
            is_blocker: None,
            equipment_type: None,
            equipment: None,
            light: Some(LightProps::read(&light)),
            character: None,
            probe: None,
            fog: None,
            decal: None,
            receive_decal: None,
            surface: None,
        }
    }

    #[test]
    fn resolves_sparse_fields_through_template_chain() {
        let mut templates = RootTemplates::default();
        let base = Node::new("GameObjects")
            .with_attr("LightType", Value::Byte(1))
            .with_attr("Intensity", Value::Float(6000.0))
            .with_attr("Radius", Value::Float(10.0))
            .with_attr("Color", Value::Vec3([1.0, 0.5, 0.25]))
            .with_attr("Angle", Value::Vec2([20.0, 30.0]));
        let child = Node::new("GameObjects").with_attr("Radius", Value::Float(7.0));
        templates
            .templates
            .insert("base".into(), template("base", None, base));
        templates
            .templates
            .insert("child".into(), template("child", Some("base"), child));

        let obj = Node::new("GameObjects")
            .with_attr("MapKey", Value::FixedString("obj".into()))
            .with_attr("Type", Value::FixedString("light".into()))
            .with_attr("TemplateName", Value::FixedString("child".into()))
            .with_attr("Intensity", Value::Float(3000.0))
            .with_child(Node::new("Transform").with_attr("Position", Value::Vec3([1.0, 2.0, 3.0])));
        let res = bg3_lsf::Resource {
            regions: vec![bg3_lsf::Region {
                name: "Templates".into(),
                node: Node::new("Templates").with_child(obj),
            }],
            ..Default::default()
        };
        let lights = read_lights(&res, "M", &templates);
        assert_eq!(lights.len(), 1);
        let l = &lights[0];
        assert_eq!(l.kind, LightKind::Spot);
        assert_eq!(l.intensity, 3000.0); // object
        assert_eq!(l.radius, 7.0); // child template
        assert_eq!(l.color, [1.0, 0.5, 0.25]); // base template
        assert_eq!(l.angle, [20.0, 30.0]);
        assert_eq!(l.transform.position, [1.0, 2.0, 3.0]);
        assert!(l.enabled);
    }

    #[test]
    fn kelvin_colours_have_unit_luminance_and_the_expected_tint() {
        // Reference values: our evaluation of the engine's formula (research notes).
        for (t, rgb) in [
            (2500.0, [2.066, 0.770, 0.138]),
            (3500.0, [1.564, 0.892, 0.405]),
            (6445.9, [1.049, 0.983, 1.028]),
            (15000.0, [0.785, 0.993, 1.706]),
        ] {
            let c = kelvin_to_rgb(t);
            for (a, b) in c.iter().zip(rgb) {
                assert!((a - b).abs() < 0.01, "{t} K: {c:?}");
            }
            let lum = 0.2126729 * c[0] + 0.7151522 * c[1] + 0.072175 * c[2];
            assert!((lum - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn reads_lighting_and_atmosphere_resources() {
        let sun = Node::new("Sun")
            .with_attr("Yaw", Value::Float(300.0))
            .with_attr("Pitch", Value::Float(45.0))
            .with_attr("SunIntensity", Value::Float(100000.0))
            .with_attr("SunColor", Value::Vec3([1.0, 0.9, 0.8]));
        let lighting = Node::new("Resource")
            .with_attr("ID", Value::FixedString("L".into()))
            .with_attr("Name", Value::LSString("LTN_Test".into()))
            .with_child(Node::new("Lighting").with_child(sun));
        let exposure = Node::new("ExposureSettings").with_child(
            Node::new("Object")
                .with_attr("ExposureMin", Value::Float(14.0))
                .with_attr("ExposureMax", Value::Float(16.0)),
        );
        let atmosphere = Node::new("Resource")
            .with_attr("ID", Value::FixedString("A".into()))
            .with_child(
                Node::new("Atmosphere").with_child(
                    Node::new("PostProcessing")
                        .with_attr("Bloom", Value::Bool(true))
                        .with_attr("Amount", Value::Float(0.25))
                        .with_attr("Vignette", Value::Bool(true))
                        .with_attr("VignetteColor", Value::Vec3([0.1, 0.0, 0.0]))
                        .with_attr("VignettePower", Value::Float(4.03))
                        .with_child(exposure)
                        .with_child(
                            Node::new("NewColorCorrection")
                                .with_attr("Contrast", Value::Float(1.12))
                                .with_child(
                                    Node::new("ShadowRange")
                                        .with_attr("XYOffset", Value::Vec2([0.1, 0.01])),
                                ),
                        ),
                ),
            );
        let res = bg3_lsf::Resource {
            regions: vec![
                bg3_lsf::Region {
                    name: "LightingBank".into(),
                    node: Node::new("LightingBank").with_child(lighting),
                },
                bg3_lsf::Region {
                    name: "AtmosphereBank".into(),
                    node: Node::new("AtmosphereBank").with_child(atmosphere),
                },
            ],
            ..Default::default()
        };
        let mut l = LevelLighting {
            lighting_id: Some("L".into()),
            lighting_name: String::new(),
            atmosphere_id: Some("A".into()),
            atmosphere_name: String::new(),
            sun: None,
            sky: None,
            fog: vec![],
            fog_render_distance: None,
            fog_phase: None,
            exposure: None,
            exposure_settings: vec![],
            post: PostProcessing::default(),
            bounds: None,
            zones: [None, None],
        };
        l.apply_bank(&res);
        assert_eq!(l.lighting_name, "LTN_Test");
        let sun = l.sun.unwrap();
        assert_eq!(sun.intensity, 100000.0);
        assert_eq!(sun.color, [1.0, 0.9, 0.8]);
        assert_eq!(l.exposure, Some([14.0, 16.0, 0.0]));
        assert!(l.post.bloom);
        assert_eq!(l.post.bloom_amount, 0.25);
        assert_eq!(l.post.bloom_threshold, 0.4, "engine default");
        assert!(l.post.vignette);
        assert_eq!(l.post.vignette_color, [0.1, 0.0, 0.0]);
        assert_eq!(
            (l.post.vignette_intensity, l.post.vignette_power),
            (0.5, 4.03)
        );
        assert_eq!(l.post.contrast, 1.12);
        assert_eq!(l.post.ranges[2].xy_offset, [0.1, 0.01]);
        assert_eq!(l.post.ranges[0], GradingRange::default());
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn level_lights_load() {
        let install = match bg3_install::Install::locate() {
            Ok(i) => i,
            Err(e) => return eprintln!("skipping: {e}"),
        };
        let vfs = Vfs::from_install(&install).unwrap();
        let templates = RootTemplates::load(&vfs).unwrap();
        for (level, min, has_lighting) in [
            ("TUT_Avernus_C", 200, true),
            ("WLD_Main_A", 5, true),
            // A sub-level of WLD_Main_A; its LevelLighting ID resolves to nothing.
            ("WLD_Crashsite_D", 20, false),
        ] {
            let lights = load_level_lights(&vfs, level, &templates).unwrap();
            let mut kinds: HashMap<String, usize> = HashMap::new();
            for l in &lights {
                *kinds.entry(format!("{:?}", l.kind)).or_default() += 1;
                assert!(l.intensity.is_finite() && l.intensity >= 0.0, "{l:?}");
                assert!(l.radius.is_finite() && l.radius >= 0.0, "{l:?}");
                assert!(l.transform.position.iter().all(|v| v.is_finite()));
            }
            let lighting = LevelLighting::load(&vfs, level).unwrap();
            eprintln!(
                "{level}: {} lights {kinds:?}; lighting {:?} sun {:?} sky {:?} exposure {:?}",
                lights.len(),
                lighting.lighting_name,
                lighting.sun,
                lighting.sky,
                lighting.exposure
            );
            assert!(lights.len() >= min, "{level}: {}", lights.len());
            assert!(
                lights
                    .iter()
                    .all(|l| !matches!(l.kind, LightKind::Unknown(_)))
            );
            assert_eq!(lighting.sun.is_some(), has_lighting);
            assert!(lighting.exposure.is_some());
        }
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn nautiloid_triggers_choose_lighting_probes_and_fog() {
        let install = match bg3_install::Install::locate() {
            Ok(i) => i,
            Err(e) => return eprintln!("skipping: {e}"),
        };
        let vfs = Vfs::from_install(&install).unwrap();
        let level = "TUT_Avernus_C";
        // A camera on the deck of the imp fight.
        let at = [-41.0, 4.0, -399.0];
        let lighting = LevelLighting::load_at(&vfs, level, Some(at)).unwrap();
        eprintln!(
            "{level} at {at:?}: {} / {} via {:?}; exposure {:?}; post {:?}",
            lighting.lighting_name,
            lighting.atmosphere_name,
            lighting.zones,
            lighting.exposure_settings,
            lighting.post
        );
        assert_eq!(lighting.lighting_name, "LTN_Default_EV10_Avernus_Ext");
        // A camera on the deck is below `…_Interior` (y 14.1–61.8) and outside the
        // footprint of `…_UnderDeck`: the exterior atmosphere.
        assert_eq!(lighting.atmosphere_name, "ATM_Default_EV10_Avernus_Ext");
        assert_eq!(
            lighting.zones[1].as_deref(),
            Some("ATM_TUT_Avernus_C_Exterior")
        );
        // A camera 12 m above the deck is in both atmosphere prisms; the interior's
        // smaller horizontal bound wins.
        let above = LevelLighting::load_at(&vfs, level, Some([-46.6, 15.0, -402.8])).unwrap();
        assert_eq!(above.atmosphere_name, "ATM_Default_EV08_Avernus_Int");
        assert_eq!(
            above.zones[1].as_deref(),
            Some("ATM_TUT_Avernus_C_Interior")
        );
        assert_eq!(above.lighting_name, "LTN_Default_EV10_Avernus_Ext");
        assert!(lighting.exposure_settings.len() >= 2);
        let e = &lighting.exposure_settings[0];
        assert!(e.min < 10.0 && e.max >= e.min);
        assert!(lighting.sky.as_ref().is_some_and(|s| s.scattering));
        // Without a point the level default applies.
        let default = LevelLighting::load(&vfs, level).unwrap();
        assert_eq!(default.lighting_name, "LTN_Default");

        let templates = RootTemplates::load(&vfs).unwrap();
        let probes = crate::probes::load_level_probes(&vfs, level, &templates).unwrap();
        let id = lighting.lighting_id.as_deref().unwrap();
        let baked = probes.iter().filter(|p| p.capture(id).is_some()).count();
        eprintln!("{} probes, {baked} baked with {id}", probes.len());
        assert!(baked >= 20);
        for p in probes.iter().filter_map(|p| p.capture(id)) {
            for path in [&p.diffuse, &p.specular, &p.sh].into_iter().flatten() {
                assert!(vfs.exists(path), "{path}");
            }
        }
        let fog = crate::fog::load_fog_volumes(&vfs, level, &templates).unwrap();
        let enabled = fog.iter().filter(|f| f.enabled).count();
        eprintln!("{} fog volumes, {enabled} enabled", fog.len());
        assert!(enabled >= 20);
        assert!(
            fog.iter()
                .all(|f| f.density >= 0.0 && f.transition_distance >= 0.0)
        );
    }
}
