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
    pub shadow: Option<bool>,
    pub pre_expose: Option<bool>,
    pub is_flickering: Option<bool>,
    pub direction_dimensions: Option<[f32; 3]>,
    pub direction_attenuation_start: Option<f32>,
    pub direction_attenuation_end: Option<f32>,
    pub direction_attenuation_side: Option<f32>,
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
            shadow: bool_attr(n, "Shadow"),
            pre_expose: bool_attr(n, "PreExpose"),
            is_flickering: bool_attr(n, "IsFlickering"),
            direction_dimensions: vec_attr(n, "DirectionLightDimensions"),
            direction_attenuation_start: f32_attr(n, "DirectionLightAttenuationStart"),
            direction_attenuation_end: f32_attr(n, "DirectionLightAttenuationEnd"),
            direction_attenuation_side: f32_attr(n, "DirectionLightAttenuationSide"),
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
    pub flat_falloff: bool,
    pub shadow: bool,
    pub pre_expose: bool,
    pub is_flickering: bool,
    pub direction_dimensions: [f32; 3],
    pub direction_attenuation: [f32; 3],
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
            kelvin: props.kelvin.unwrap_or(6500.0),
            use_temperature: props.use_temperature.unwrap_or(false),
            radius: props.radius.unwrap_or(0.0),
            angle: props.angle.unwrap_or([35.0, 45.0]),
            flat_falloff: props.flat_falloff.unwrap_or(false),
            shadow: props.shadow.unwrap_or(false),
            pre_expose: props.pre_expose.unwrap_or(false),
            is_flickering: props.is_flickering.unwrap_or(false),
            direction_dimensions: props.direction_dimensions.unwrap_or([1.0; 3]),
            direction_attenuation: [
                props.direction_attenuation_start.unwrap_or(0.0),
                props.direction_attenuation_end.unwrap_or(0.0),
                props.direction_attenuation_side.unwrap_or(0.0),
            ],
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

/// Approximate black-body colour (linear RGB, max component 1) for a colour temperature.
///
/// Our own fit, not the game's: Tanner Helland's sRGB approximation, converted to linear.
pub fn kelvin_to_rgb(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_16 * (t - 60.0).powf(-0.075_514_846)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    let lin = |c: f32| {
        let c = (c / 255.0).clamp(0.0, 1.0);
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [lin(r), lin(g), lin(b)]
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub kelvin: f32,
    pub use_temperature: bool,
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
    /// `(ExposureMin, ExposureMax, ExposureCompensation)` of the first `ExposureSettings` (EV100).
    pub exposure: Option<[f32; 3]>,
    /// `LevelBounds` from `meta.lsf` (min, max).
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

impl LevelLighting {
    /// Read `meta.lsf` (last module wins) and resolve its lighting/atmosphere resources.
    pub fn load(vfs: &Vfs, level: &str) -> Result<Self> {
        let mut meta = None;
        for module in BASE_LOAD_ORDER {
            let path = format!("Mods/{module}/Levels/{level}/meta.lsf");
            if vfs.exists(&path) {
                meta = Some(parse(vfs, &path)?);
            }
        }
        let data = meta.as_ref().and_then(|m| m.region("LevelData"));
        let lighting_id = data.and_then(|d| str_attr(d, "LevelLighting"));
        let atmosphere_id = data.and_then(|d| str_attr(d, "LevelAtmosphere"));
        let bounds = data
            .and_then(|d| d.child("LevelBounds"))
            .and_then(|b| Some((vec_attr::<3>(b, "Min")?, vec_attr::<3>(b, "Max")?)));
        let mut out = Self {
            lighting_id,
            lighting_name: String::new(),
            atmosphere_id,
            atmosphere_name: String::new(),
            sun: None,
            sky: None,
            fog: Vec::new(),
            fog_render_distance: None,
            exposure: None,
            bounds,
        };
        for res in atmosphere_banks(vfs)? {
            out.apply_bank(&res);
        }
        Ok(out)
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
            self.exposure = r
                .child("Atmosphere")
                .and_then(|a| a.child("PostProcessing"))
                .and_then(|p| p.child("ExposureSettings"))
                .and_then(|e| e.child("Object"))
                .and_then(|o| {
                    Some([
                        f32_attr(o, "ExposureMin")?,
                        f32_attr(o, "ExposureMax")?,
                        f32_attr(o, "ExposureCompensation").unwrap_or(0.0),
                    ])
                });
        }
    }

    fn read_lighting(&mut self, l: &Node) {
        self.sun = l.child("Sun").map(|s| Sun {
            yaw: f32_attr(s, "Yaw").unwrap_or(0.0),
            pitch: f32_attr(s, "Pitch").unwrap_or(45.0),
            color: vec_attr(s, "SunColor").unwrap_or([1.0; 3]),
            intensity: f32_attr(s, "SunIntensity").unwrap_or(0.0),
            kelvin: f32_attr(s, "Kelvin").unwrap_or(6500.0),
            use_temperature: bool_attr(s, "UseTemperature").unwrap_or(false),
            shadow: bool_attr(s, "ShadowEnabled").unwrap_or(false),
        });
        self.sky = l.child("SkyLight").map(|s| SkyLight {
            color: vec_attr(s, "Color").unwrap_or([1.0; 3]),
            intensity: f32_attr(s, "Intensity").unwrap_or(0.0),
            kelvin: f32_attr(s, "Kelvin").unwrap_or(6500.0),
            use_temperature: bool_attr(s, "UseTemperature").unwrap_or(false),
        });
        if let Some(fog) = l.child("Fog") {
            self.fog_render_distance = f32_attr(fog, "RenderDistance");
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
            equipment: None,
            light: Some(LightProps::read(&light)),
            character: None,
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
    fn kelvin_colours_are_plausible() {
        let warm = kelvin_to_rgb(2700.0);
        let neutral = kelvin_to_rgb(6600.0);
        let cold = kelvin_to_rgb(15000.0);
        assert!(warm[0] > warm[2] * 3.0);
        assert!((neutral[0] - 1.0).abs() < 0.05 && neutral[2] > 0.9);
        assert!(cold[2] > cold[0]);
        for c in [warm, neutral, cold] {
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)));
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
                Node::new("Atmosphere")
                    .with_child(Node::new("PostProcessing").with_child(exposure)),
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
            exposure: None,
            bounds: None,
        };
        l.apply_bank(&res);
        assert_eq!(l.lighting_name, "LTN_Test");
        let sun = l.sun.unwrap();
        assert_eq!(sun.intensity, 100000.0);
        assert_eq!(sun.color, [1.0, 0.9, 0.8]);
        assert_eq!(l.exposure, Some([14.0, 16.0, 0.0]));
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
}
