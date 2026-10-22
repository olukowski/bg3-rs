//! Level (non-character) surfaces: which parts of the game's graph each material needs and the
//! uniform block of the viewer's `level.wesl`.
//!
//! Established from the compiled `DEF` (G-buffer) and `EMI` (emissive) passes of the Nautiloid's
//! graphs (`docs/research/materials.md`, "Level shading"). Every `Base_*`/`ENV_*` graph writes
//! the same G-buffer: base colour, the normal map, roughness = physical G, metalness =
//! physical R, occlusion = physical B, and `sat(Reflectance)`; the differences are the shading
//! model, the subsurface inputs, a micro detail layer and the glow.

use crate::resolve::{ResolvedMaterial, Slot};

/// Number of `vec4`s in the level uniform block (`level.wesl` `LevelUniform::v`).
pub const LEVEL_UNIFORM_LEN: usize = 10;

/// How a graph's emissive pass computes its glow (`<graph>_ST_EMI_fragmentMain`). `GM` is the
/// glow map, `c` the glow colour, `sel` = `mix(GM.r · c, GM.rgb, step(UseGMColor))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlowKind {
    /// No emissive pass.
    None,
    /// `Base_GM_VT`: `sel · GlowMultiplier`.
    Plain,
    /// `Base_GM_Pulse_VT`: `sel · GlowMultiplier · lerp(MinGlow, MaxGlow, ½ + ½cos(t · PulseSpeed))`,
    /// times the camera exposure (the only glow graph that pre-exposes).
    PulseExposed,
    /// `Base_GM_Pulse_VT_FX`: the same, not exposed.
    Pulse,
    /// `Base_GM_Pulse_AlphaTest_Translucency_VT`: `GM.rgb · c · GlowMultiplier · pulse` (cos).
    PulseColour,
    /// `Base_GM_Wind_VT`, `Base_GM_Wind_Translucency_VT`:
    /// `sel · lerp(GlowMin, GlowMax, ½ + ½sin(t · GlowPulseSpeed))`, no multiplier.
    Wind,
    /// `Base_GM_Pulse_Bouncing_AlphaTest_VT`: `GM.rgb · c · GlowMultiplier · lerp(MinGlow, MaxGlow,
    /// s)`, `s` a smoothstep triangle wave of `t · PulseSpeed + |object origin| +
    /// local y / MeshPulseSize + vertex colour B`.
    Bouncing,
    /// `Base_Glass_Glow`: `c · GM.rgb · GlowMap_Color · GlowIntensity ·
    /// mix(1, smoothstep(GlowGradientMinHeight, GlowGradientMaxHeight, local y), sat(Height_Mask))`.
    GlassGlow,
}

impl GlowKind {
    /// The shader's `GLOW_*` value.
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Where the subsurface inputs (G-buffer target 3) come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubsurfaceInput {
    /// Amount 1, thickness 0 (no texture).
    None,
    /// `Base_SSS_VT`: amount 1, thickness = `sat(SSS_Thickness.r)`.
    ThicknessMap,
    /// `Base_AlphaTest_2s_SSS_VT`: amount = `sat(SSS_Radius · SSS_MSK.r)`, thickness 0.
    RadiusMask,
    /// `Base_*Translucency_VT` (shading model 4): thickness = `sat(physical.a)`; the lighting
    /// takes amount 1.
    PhysicalAlpha,
}

impl SubsurfaceInput {
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// What a level material needs from the shader besides Bevy's standard inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelShading {
    /// G-buffer shading model (0 lit, 3 subsurface, 4 translucent).
    pub model: u32,
    pub glow: GlowKind,
    pub subsurface: SubsurfaceInput,
    /// `ENV_DetailMap*`: the micro detail layer (`MicroDetailMap`, `MicroDetailNormalMap`).
    pub detail: bool,
    /// Texture parameter feeding the subsurface input ([`SubsurfaceInput::ThicknessMap`] and
    /// [`SubsurfaceInput::RadiusMask`]).
    pub subsurface_texture: Option<&'static str>,
}

fn stem(graph_path: &str) -> &str {
    let file = graph_path.rsplit('/').next().unwrap_or(graph_path);
    file.strip_suffix(".lsf").unwrap_or(file)
}

impl LevelShading {
    /// Classify a resolved level material by its graph.
    // bg3: Shared_Base_<graph>_ST_DEF_fragmentMain, _ST_EMI_fragmentMain (Materials.metallib,
    // 4.1.1.7631656)
    pub fn classify(graph_path: &str, m: &ResolvedMaterial) -> Self {
        let stem = stem(graph_path);
        let model = u32::from(m.shading_model.unwrap_or(0));
        let has_glow_map = m.texture(Slot::Emissive).is_some();
        let glow = match stem {
            _ if !has_glow_map => GlowKind::None,
            "Base_GM_VT" => GlowKind::Plain,
            "Base_GM_Pulse_VT" => GlowKind::PulseExposed,
            "Base_GM_Pulse_VT_FX" => GlowKind::Pulse,
            "Base_GM_Pulse_AlphaTest_Translucency_VT" => GlowKind::PulseColour,
            "Base_GM_Wind_VT" | "Base_GM_Wind_Translucency_VT" => GlowKind::Wind,
            "Base_GM_Pulse_Bouncing_AlphaTest_VT" => GlowKind::Bouncing,
            "Base_Glass_Glow" => GlowKind::GlassGlow,
            // Other glow graphs (VFX models, `Base_SSS_Fresnel_GM_VT`, `ENV_GM_Pulse_Fluid_VT`)
            // are not ported.
            _ => GlowKind::None,
        };
        let (subsurface, subsurface_texture) = match (model, stem) {
            (3, _) if m.textures.iter().any(|t| t.param == "SSS_MSK") => {
                (SubsurfaceInput::RadiusMask, Some("SSS_MSK"))
            }
            (3, _) if m.textures.iter().any(|t| t.param == "SSS_Thickness") => {
                (SubsurfaceInput::ThicknessMap, Some("SSS_Thickness"))
            }
            (4, _) if m.needs_virtual_texture() => (SubsurfaceInput::PhysicalAlpha, None),
            _ => (SubsurfaceInput::None, None),
        };
        let detail = stem.starts_with("ENV_DetailMap")
            && m.textures.iter().any(|t| t.param == "MicroDetailMap");
        Self {
            model,
            glow,
            subsurface,
            detail,
            subsurface_texture,
        }
    }

    /// The uniform block (`level.wesl` `LevelUniform::v`); the profile (`v[6..10]`) is filled
    /// in by the caller, which owns the kernel slots.
    ///
    /// - `v[0]`: Reflectance, MicroDetailTiling, DetailNormalStrength, DetailAOStrength
    /// - `v[1]`: DetailRoughnessStrength, SSS_Radius, 0, 0
    /// - `v[2]`: glow colour, glow multiplier (Glass: `GlowMap_Color · GlowIntensity`)
    /// - `v[3]`: UseGMColor, pulse speed, min glow, max glow
    /// - `v[4]`: MeshPulseSize, Height_Mask, GlowGradientMinHeight, GlowGradientMaxHeight
    pub fn uniform(&self, m: &ResolvedMaterial) -> [[f32; 4]; LEVEL_UNIFORM_LEN] {
        let s = |name: &str, default: f32| m.scalar(name).unwrap_or(default);
        let mut v = [[0.0; 4]; LEVEL_UNIFORM_LEN];
        // Graph defaults apply through `resolve`; these fallbacks only matter for graphs
        // without the parameter (the G-buffer's default reflectance is 0.5).
        v[0] = [
            s("Reflectance", 0.5),
            s("MicroDetailTiling", 1.0),
            s("DetailNormalStrength", 0.0),
            s("DetailAOStrength", 0.0),
        ];
        v[1] = [
            s("DetailRoughnessStrength", 0.0),
            s("SSS_Radius", 0.0),
            0.0,
            0.0,
        ];
        let colour = ["GlowColour", "GlowColor"]
            .iter()
            .find_map(|n| m.vector(n))
            .and_then(|c| Some([*c.first()?, *c.get(1)?, *c.get(2)?]))
            .unwrap_or([1.0; 3]);
        let (multiplier, speed, min, max) = match self.glow {
            GlowKind::None => (0.0, 0.0, 0.0, 0.0),
            GlowKind::Plain => (s("GlowMultiplier", 1.0), 0.0, 1.0, 1.0),
            GlowKind::PulseExposed
            | GlowKind::Pulse
            | GlowKind::PulseColour
            | GlowKind::Bouncing => (
                s("GlowMultiplier", 1.0),
                s("PulseSpeed", 0.0),
                s("MinGlow", 1.0),
                s("MaxGlow", 1.0),
            ),
            GlowKind::Wind => (
                1.0,
                s("GlowPulseSpeed", 0.0),
                s("GlowMin", 1.0),
                s("GlowMax", 1.0),
            ),
            GlowKind::GlassGlow => (
                s("GlowMap_Color", 1.0) * s("GlowIntensity", 1.0),
                0.0,
                1.0,
                1.0,
            ),
        };
        v[2] = [colour[0], colour[1], colour[2], multiplier];
        v[3] = [s("UseGMColor", 0.0), speed, min, max];
        v[4] = [
            s("MeshPulseSize", 1.0),
            s("Height_Mask", 0.0),
            s("GlowGradientMinHeight", 0.0),
            s("GlowGradientMaxHeight", 1.0),
        ];
        v
    }
}

/// `pulse` of the glow graphs at time `t` (s): `lerp(min, max, ½ + ½cos(t · speed))`
/// (`Wind`: `sin`). Reference for the shader.
pub fn pulse(kind: GlowKind, t: f32, speed: f32, min: f32, max: f32) -> f32 {
    let w = match kind {
        GlowKind::Wind => 0.5 + 0.5 * (t * speed).sin(),
        _ => 0.5 + 0.5 * (t * speed).cos(),
    };
    min + (max - min) * w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::BlendMode;
    use crate::resolve::TextureRef;

    fn material(model: u8, textures: &[(&str, Slot)], scalars: &[(&str, f32)]) -> ResolvedMaterial {
        ResolvedMaterial {
            blend: BlendMode::Opaque,
            two_sided: false,
            unlit: false,
            invisible: false,
            textures: textures
                .iter()
                .map(|(p, slot)| TextureRef {
                    param: (*p).into(),
                    id: format!("{p}_tex"),
                    slot: *slot,
                })
                .collect(),
            virtual_textures: vec!["vt".into()],
            scalars: scalars.iter().map(|(n, v)| ((*n).into(), *v)).collect(),
            vectors: vec![("GlowColour".into(), vec![1.0, 0.04, 0.02])],
            shading_model: Some(model),
            diffusion_profile: None,
        }
    }

    #[test]
    fn graphs_classify_by_name_and_inputs() {
        let gm = material(
            0,
            &[("Glowmap", Slot::Emissive)],
            &[("GlowMultiplier", 12.0)],
        );
        let s = LevelShading::classify("Public/Shared/Assets/Materials/Base/Base_GM_VT.lsf", &gm);
        assert_eq!((s.model, s.glow), (0, GlowKind::Plain));
        assert_eq!(s.uniform(&gm)[2], [1.0, 0.04, 0.02, 12.0]);

        let sss = material(3, &[("SSS_Thickness", Slot::Other)], &[]);
        let s = LevelShading::classify("Base/Base_SSS_VT.lsf", &sss);
        assert_eq!(s.subsurface, SubsurfaceInput::ThicknessMap);
        assert_eq!(s.subsurface_texture, Some("SSS_Thickness"));
        assert_eq!(s.uniform(&sss)[0][0], 0.5, "default reflectance");

        let mask = material(3, &[("SSS_MSK", Slot::Other)], &[("SSS_Radius", 0.0)]);
        let s = LevelShading::classify("Base/Base_AlphaTest_2s_SSS_VT.lsf", &mask);
        assert_eq!(s.subsurface, SubsurfaceInput::RadiusMask);

        let tr = material(4, &[], &[]);
        let s = LevelShading::classify("Base/Base_Translucency_VT.lsf", &tr);
        assert_eq!((s.model, s.subsurface), (4, SubsurfaceInput::PhysicalAlpha));

        let detail = material(
            0,
            &[
                ("MicroDetailMap", Slot::Other),
                ("MicroDetailNormalMap", Slot::Other),
            ],
            &[
                ("MicroDetailTiling", 10.0),
                ("DetailRoughnessStrength", 0.45),
            ],
        );
        let s = LevelShading::classify("Environment/ENV_DetailMap_VT.lsf", &detail);
        assert!(s.detail);
        let v = s.uniform(&detail);
        assert_eq!((v[0][1], v[1][0]), (10.0, 0.45));

        // A glow graph without a glow map, and an unported glow graph, have no glow.
        let s = LevelShading::classify("Base/Base_GM_VT.lsf", &material(0, &[], &[]));
        assert_eq!(s.glow, GlowKind::None);
        let s = LevelShading::classify("Base/Base_SSS_Fresnel_GM_VT.lsf", &gm);
        assert_eq!(s.glow, GlowKind::None);
    }

    #[test]
    fn pulses_follow_the_emissive_passes() {
        // cos for the pulse graphs, sin for wind; ½ + ½·wave between min and max.
        assert_eq!(pulse(GlowKind::PulseExposed, 0.0, 1.0, 10.0, 30.0), 30.0);
        assert!(
            (pulse(GlowKind::Pulse, std::f32::consts::PI, 1.0, 10.0, 30.0) - 10.0).abs() < 1e-4
        );
        assert_eq!(pulse(GlowKind::Wind, 0.0, 4.0, 2.0, 4.0), 3.0);
        let glass = material(
            0,
            &[("Glowmap", Slot::Emissive)],
            &[
                ("GlowMap_Color", 1.0),
                ("GlowIntensity", 0.2),
                ("Height_Mask", 1.0),
            ],
        );
        let s = LevelShading::classify("Base/Base_Glass_Glow.lsf", &glass);
        assert_eq!(s.glow, GlowKind::GlassGlow);
        let v = s.uniform(&glass);
        assert!((v[2][3] - 0.2).abs() < 1e-6);
        assert_eq!(v[4][1], 1.0);
    }
}
