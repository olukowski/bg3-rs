//! Which character shading a material gets, and its uniform block.
//!
//! The uniform is a list of `vec4`s whose layout per kind matches the viewer's
//! `character.wesl` (documented on each builder).

use super::params::ParamSet;
use super::skin::{SkinKind, SkinUniform};
use crate::resolve::ResolvedMaterial;

/// Number of `vec4`s in the shared uniform block.
pub const UNIFORM_LEN: usize = 24;

/// Character shading kinds, in the order of the shader's `MODE_*` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharKind {
    Skin(SkinKind),
    /// 12-colour palette tint from an `MSKColor` map (the `CHAR_BASE_*` family).
    ClothPalette,
    /// 3-colour tint from an `MSKcloth` map (the `*_MSK_*` graphs).
    ClothMsk,
    /// `CHAR_Eye_Parallax`, `_FX` (`creature: false`) and `_Creature_FX` (`creature: true`);
    /// `fx`: the glowing variants.
    Eye {
        creature: bool,
        fx: bool,
    },
    Hair,
    /// `CHAR_LashesShadow`: alpha-blended lash cards and contact shadow.
    Lashes,
    /// `CHAR_Tearline`: only adds a specular highlight in the game; not drawn here.
    Tearline,
}

impl CharKind {
    /// Classify by graph file name, then by the palette-mask parameters the graph declares.
    pub fn classify(graph_path: &str, m: &ResolvedMaterial) -> Option<Self> {
        let file = graph_path.rsplit('/').next().unwrap_or(graph_path);
        let stem = file.strip_suffix(".lsf").unwrap_or(file);
        let kind = match stem {
            "CHAR_Skin_Body" => Self::Skin(SkinKind::Body),
            "CHAR_Skin_Head_v3" => Self::Skin(SkinKind::Head),
            "CHAR_Eye_Parallax" => Self::Eye {
                creature: false,
                fx: false,
            },
            "CHAR_Eye_Parallax_FX" => Self::Eye {
                creature: false,
                fx: true,
            },
            "CHAR_Eye_Parallax_Creature_FX" => Self::Eye {
                creature: true,
                fx: true,
            },
            "CHAR_Hair" => Self::Hair,
            "CHAR_LashesShadow" => Self::Lashes,
            "CHAR_Tearline" => Self::Tearline,
            _ if !stem.starts_with("CHAR_") => return None,
            _ if m.textures.iter().any(|t| t.param == "MSKColor") => Self::ClothPalette,
            _ if m.textures.iter().any(|t| t.param == "MSKcloth") => Self::ClothMsk,
            _ => return None,
        };
        Some(kind)
    }

    /// The G-buffer shading model the game lights this graph with: 3 subsurface (skin and the
    /// `SSS` clothing graphs), 5 hair, else 0 (lit).
    pub fn shading_model(self, graph_path: &str) -> u32 {
        let file = graph_path.rsplit('/').next().unwrap_or(graph_path);
        match self {
            Self::Skin(_) => 3,
            Self::Hair => 5,
            Self::ClothPalette | Self::ClothMsk if file.to_ascii_uppercase().contains("SSS") => 3,
            _ => 0,
        }
    }

    /// The shader's `MODE_*` value.
    pub fn mode(self) -> u32 {
        match self {
            Self::Skin(_) => 0,
            Self::ClothPalette => 1,
            Self::ClothMsk => 2,
            Self::Eye { .. } => 3,
            Self::Hair => 4,
            Self::Lashes => 5,
            Self::Tearline => 6,
        }
    }

    /// The uniform block for this kind from the material's (overridden) parameters.
    pub fn uniform(self, p: &ParamSet) -> [[f32; 4]; UNIFORM_LEN] {
        let mut v = [[0.0; 4]; UNIFORM_LEN];
        match self {
            Self::Skin(kind) => {
                let s = SkinUniform::new(kind, p);
                v[..SkinUniform::LEN].copy_from_slice(&s.v);
            }
            Self::ClothPalette => {
                for (i, (name, d)) in PALETTE.iter().enumerate() {
                    let [r, g, b] = p.rgb(name, *d);
                    v[i] = [r, g, b, 0.0];
                }
            }
            Self::ClothMsk => {
                for (i, name) in ["Color_01", "Color_02", "Color_03"].iter().enumerate() {
                    let [r, g, b] = p.rgb(name, [1.0; 3]);
                    v[i] = [r, g, b, 0.0];
                }
            }
            Self::Eye { creature, fx } => eye_uniform(&mut v, p, creature, fx),
            Self::Hair => super::hair::uniform(&mut v, p),
            Self::Lashes => {
                let [r, g, b] = p.rgb("Eyelashes_Color", [0.0; 3]);
                v[0] = [r, g, b, p.scalar("GradientIntensity", 1.0)];
                v[1] = [
                    p.scalar("LashesAlphaIntensity", 0.5),
                    p.scalar("ShadowAlphaIntensity", 1.0),
                    0.0,
                    0.0,
                ];
            }
            Self::Tearline => {}
        }
        v
    }
}

/// The palette parameters in shader order, with the graph defaults of `CHAR_BASE_VT`.
/// Their positions in the mask's RGB cube are fixed in the shader (`palette_point`).
pub const PALETTE: [(&str, [f32; 3]); 12] = [
    ("Cloth_Primary", [0.151, 0.293, 0.488]),
    ("Cloth_Secondary", [0.170, 0.046, 0.046]),
    ("Cloth_Tertiary", [0.072, 0.062, 0.106]),
    ("Accent_Color", [1.0; 3]),
    ("Leather_Primary", [0.239, 0.104, 0.060]),
    ("Leather_Secondary", [0.151, 0.034, 0.027]),
    ("Leather_Tertiary", [0.122, 0.060, 0.051]),
    ("Custom_1", [1.0; 3]),
    ("Metal_Primary", [0.767; 3]),
    ("Metal_Secondary", [0.687; 3]),
    ("Metal_Tertiary", [0.605; 3]),
    ("Custom_2", [1.0; 3]),
];

/// Palette point of each [`PALETTE`] entry (`MSKColor` RGB).
pub const PALETTE_POINTS: [[f32; 3]; 12] = [
    [1.0, 0.5, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 0.5, 0.5],
    [1.0, 0.0, 0.5],
    [0.5, 0.0, 1.0],
    [0.0, 0.0, 1.0],
    [0.5, 0.5, 1.0],
    [0.0, 0.5, 1.0],
    [0.0, 1.0, 0.5],
    [0.0, 1.0, 0.0],
    [0.5, 1.0, 0.5],
    [0.5, 1.0, 0.0],
];

/// Tint of a palette mask texel: each colour weighs `4·clamp(1 − |m − P|/√3 − 0.75, 0, 1)`
/// (1 on its point, 0 beyond a quarter of the cube diagonal); the weights are not normalised.
pub fn palette_tint(m: [f32; 3], colours: &[[f32; 3]; 12]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (p, c) in PALETTE_POINTS.iter().zip(colours) {
        let d = ((m[0] - p[0]).powi(2) + (m[1] - p[1]).powi(2) + (m[2] - p[2]).powi(2)).sqrt();
        let w = 4.0 * (1.0 - d / 3f32.sqrt() - 0.75).clamp(0.0, 1.0);
        for k in 0..3 {
            out[k] += w * c[k];
        }
    }
    out
}

/// Eye layout: v0 iris, v1 iris secondary, v2 sclera, v3 blood, v4..7 the `_L` set (rgb),
/// v8 (SecondaryColourIntensity, _L, IrisEdgeStrength, _L), v9 (Redness, _L, Blindness, _L),
/// v10 (Heterochromia, Ersatz, creature, catchlight weight).
/// Eye uniform: v0–3 iris, secondary, sclera, blood colours; v4–7 their `_L` set; v8, v9 the
/// paired scalars; v10 (Heterochromia, Ersatz, creature, catchlight on); v11 (fx,
/// ParallaxWeight, Sclera_Normal_Weight); v12 (GlowColour, FxMasking); v13 (GlowColour_L,
/// FxMasking_L); v14 (GlowBrightness, GlowBrightness_L, GlowBrightnessPupil, AnimationSpeed);
/// v15 (FlameSpeed, FlameDirection).
fn eye_uniform(v: &mut [[f32; 4]; UNIFORM_LEN], p: &ParamSet, creature: bool, fx: bool) {
    let colours = [
        ("Eyes_IrisColour", [0.18, 0.325, 0.567]),
        ("Eyes_IrisSecondaryColour", [0.349, 0.5, 0.68]),
        ("Eyes_ScleraColour", [0.949, 0.805, 0.701]),
        ("Eyes_BloodColour", [0.405, 0.046, 0.053]),
    ];
    for (i, (name, d)) in colours.iter().enumerate() {
        let [r, g, b] = p.rgb(name, *d);
        v[i] = [r, g, b, 0.0];
        let [r, g, b] = p.rgb(&format!("{name}_L"), [r, g, b]);
        v[i + 4] = [r, g, b, 0.0];
    }
    let pair = |n: &str, d: f32| {
        let a = p.scalar(n, d);
        (a, p.scalar(&format!("{n}_L"), a))
    };
    let (k, kl) = pair("SecondaryColourIntensity", if creature { 1.0 } else { 2.0 });
    let (e, el) = pair("IrisEdgeStrength", if creature { 0.3 } else { 0.5 });
    let (r, rl) = pair("Redness", 1.0);
    let (b, bl) = pair("Blindness", 0.0);
    v[8] = [k, kl, e, el];
    v[9] = [r, rl, b, bl];
    v[10] = [
        p.scalar("Heterochromia", 0.0),
        p.scalar("Ersatz", 0.0),
        if creature { 1.0 } else { 0.0 },
        1.0,
    ];
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    // `_Creature_FX` mixes its parallax by ParallaxWeight; the FX graphs fix the sclera
    // detail-normal weight at 1.
    v[11] = [
        flag(fx),
        if creature {
            p.scalar("ParallaxWeight", 1.0)
        } else {
            1.0
        },
        if fx {
            1.0
        } else {
            p.scalar("Sclera_Normal_Weight", 1.0)
        },
        0.0,
    ];
    let glow = p.rgb("GlowColour", [0.489, 0.0925, 0.0]);
    let masking = p.scalar("FxMasking", 6.0);
    v[12] = [glow[0], glow[1], glow[2], masking];
    let glow_l = p.rgb("GlowColour_L", [0.0, 0.489, 0.427]);
    v[13] = [
        glow_l[0],
        glow_l[1],
        glow_l[2],
        p.scalar("FxMasking_L", masking),
    ];
    let bright = p.scalar("GlowBrightness", 30.0);
    v[14] = [
        bright,
        p.scalar("GlowBrightness_L", bright),
        p.scalar("GlowBrightnessPupil", 0.0),
        p.scalar("AnimationSpeed", 0.0),
    ];
    let [sx, sy] = p.vec2("FlameSpeed", [0.0; 2]);
    let [dx, dy] = p.vec2("FlameDirection", [0.0; 2]);
    v[15] = [sx, sy, dx, dy];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::BlendMode;
    use crate::resolve::{Slot, TextureRef};

    fn material(textures: &[&str]) -> ResolvedMaterial {
        ResolvedMaterial {
            blend: BlendMode::Opaque,
            two_sided: false,
            unlit: false,
            invisible: false,
            textures: textures
                .iter()
                .map(|t| TextureRef {
                    param: (*t).into(),
                    id: "x".into(),
                    slot: Slot::Other,
                })
                .collect(),
            virtual_textures: vec![],
            scalars: vec![],
            vectors: vec![],
            shading_model: None,
            diffusion_profile: None,
        }
    }

    #[test]
    fn classification() {
        let m = material(&[]);
        let c = |g: &str, m: &ResolvedMaterial| CharKind::classify(g, m);
        assert_eq!(
            c("a/CHAR_Skin_Body.lsf", &m),
            Some(CharKind::Skin(SkinKind::Body))
        );
        assert_eq!(
            c("CHAR_Skin_Head_v3.lsf", &m),
            Some(CharKind::Skin(SkinKind::Head))
        );
        assert_eq!(
            c("CHAR_Eye_Parallax_Creature_FX.lsf", &m),
            Some(CharKind::Eye {
                creature: true,
                fx: true
            })
        );
        assert_eq!(c("CHAR_BASE_VT.lsf", &m), None);
        assert_eq!(
            c("CHAR_BASE_VT.lsf", &material(&["MSKColor"])),
            Some(CharKind::ClothPalette)
        );
        assert_eq!(
            c(
                "CHAR_BASE_SSS_AlphaTested_2S_MSK_VT.lsf",
                &material(&["MSKcloth"])
            ),
            Some(CharKind::ClothMsk)
        );
        // Only character graphs: a level material with a mask parameter stays generic.
        assert_eq!(c("Base_VT.lsf", &material(&["MSKColor"])), None);
    }

    #[test]
    fn shading_models() {
        let skin = CharKind::Skin(SkinKind::Head);
        assert_eq!(skin.shading_model("CHAR_Skin_Head_v3.lsf"), 3);
        assert_eq!(CharKind::Hair.shading_model("CHAR_Hair.lsf"), 5);
        let msk = CharKind::ClothMsk;
        assert_eq!(
            msk.shading_model("x/CHAR_BASE_SSS_AlphaTested_2S_MSK_VT.lsf"),
            3
        );
        assert_eq!(CharKind::ClothPalette.shading_model("CHAR_BASE_VT.lsf"), 0);
        assert_eq!(
            CharKind::Eye {
                creature: false,
                fx: false
            }
            .shading_model("CHAR_Eye_Parallax.lsf"),
            0
        );
    }

    #[test]
    fn palette_weights_hit_their_point_and_fade_out() {
        let mut colours = [[0.0; 3]; 12];
        colours[0] = [0.1, 0.2, 0.3]; // Cloth_Primary at (1, .5, 0)
        colours[11] = [1.0; 3]; // Custom_2 at (.5, 1, 0)
        assert_eq!(palette_tint([1.0, 0.5, 0.0], &colours), [0.1, 0.2, 0.3]);
        assert_eq!(palette_tint([0.5, 1.0, 0.0], &colours), [1.0; 3]);
        // Farther than a quarter diagonal (√3/4) from every point: black.
        assert_eq!(palette_tint([0.5, 0.5, 0.5], &colours), [0.0; 3]);
        // Slightly off the point: a weight below 1.
        let t = palette_tint([0.95, 0.5, 0.0], &colours);
        assert!(t[0] < 0.1 && t[0] > 0.05, "{t:?}");
    }

    #[test]
    fn eye_left_set_defaults_to_the_right_one() {
        let mut p = ParamSet::default();
        p.set("Eyes_IrisColour", vec![0.5, 0.1, 0.2]);
        p.set("Redness", vec![0.8]);
        let v = CharKind::Eye {
            creature: false,
            fx: false,
        }
        .uniform(&p);
        assert_eq!(v[0][..3], [0.5, 0.1, 0.2]);
        assert_eq!(v[4][..3], [0.5, 0.1, 0.2]);
        assert_eq!(v[9][..2], [0.8, 0.8]);
    }
}
