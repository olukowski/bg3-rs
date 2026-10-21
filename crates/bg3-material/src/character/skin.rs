//! Skin: `CHAR_Skin_Body` and `CHAR_Skin_Head_v3`.
//!
//! The skin colour is not painted: it is built from per-texel concentrations of hemoglobin,
//! melanin, veins and yellowing (the `HMVY` map, which for bodies is the virtual texture's
//! physical layer) and the character's pigment colours, then face/body layers (eyebrows,
//! body hair, lips, make-up, tattoos, non-skin parts) are mixed on top. Formulas and sources:
//! `docs/research/character-shaders.md`. [`shade`] is the reference implementation the
//! viewer's `skin.wesl` mirrors; its layout constants ([`SkinUniform`]) are shared.
//!
//! bg3: Shared_Characters_CHAR_Skin_Body_ST_DEF_fragmentMain,
//! Shared_Characters_CHAR_Skin_Head_v3_ST_DEF_fragmentMain (Materials.metallib, 4.1.1.7631656)

use super::params::ParamSet;

/// Constant tint applied to the pigment product (same in body and head shaders).
pub const SKIN_TINT: [f32; 3] = [0.95222, 0.744_097_9, 0.595_242_5];
/// Rec. 709 luminance weights used by the desaturation steps.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Which skin graph: they share the pigment model but read different maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkinKind {
    Body,
    Head,
}

/// Uniform block of the skin material: 20 `vec4`s, laid out as documented per field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinUniform {
    pub v: [[f32; 4]; Self::LEN],
}

impl SkinUniform {
    pub const LEN: usize = 20;

    /// Gather the uniform from a material's (overridden) parameters.
    pub fn new(kind: SkinKind, p: &ParamSet) -> Self {
        let rgb = |n: &str, d: [f32; 3], w: f32| {
            let [r, g, b] = p.rgb(n, d);
            [r, g, b, w]
        };
        let s = |n: &str, d: f32| p.scalar(n, d);
        let body = kind == SkinKind::Body;
        let skin_weight = if body {
            1.0 - s("isGenitals", 0.0)
        } else {
            1.0
        };
        let tattoo = if body {
            let i = p.vec4("BodyTattooIntensity", [0.0; 4]);
            let k = s("doDrawTattoo", 1.0) * skin_weight;
            i.map(|x| x * k)
        } else {
            p.vec4("TattooIntensity", [0.0; 4])
        };
        let (tc, tci) = if body {
            ("BodyTattooColor", "BodyTattooIndex")
        } else {
            ("TattooColor", "TattooIndex")
        };
        let mut v = [[0.0; 4]; Self::LEN];
        v[0] = rgb(
            "HemoglobinColor",
            [0.65, 0.009, 0.009],
            s("HemoglobinAmount", 1.0),
        );
        v[1] = rgb("VeinColor", [0.0, 0.421, 0.73], s("VeinAmount", 1.0));
        v[2] = rgb(
            "MelaninColor",
            [0.0077, 0.0039, 0.0019],
            s("MelaninAmount", 0.6),
        );
        v[3] = rgb(
            "YellowingColor",
            [1.0, 0.789, 0.184],
            s("YellowingAmount", 0.3),
        );
        v[4] = [
            s("MelaninDarkThreshold", 0.9),
            s("MelaninDarkMultiplier", 2.0),
            s("Vitiligo", 0.0),
            s("VitiligoWeight", 0.5),
        ];
        v[5] = [
            s("Freckles", 0.0),
            s("FrecklesWeight", 0.1),
            s("MelaninRemovalAmount", 0.0),
            skin_weight,
        ];
        v[6] = rgb("NonSkinColor", [1.0; 3], s("NonSkin_Weight", 1.0));
        v[7] = if body {
            rgb("Body_Hair_Color", [0.0; 3], s("NonSkinMetalness", 0.0))
        } else {
            rgb(
                "Eyebrow_Color",
                [0.074, 0.038, 0.022],
                s("NonSkinMetalness", 0.0),
            )
        };
        v[8] = if body {
            rgb("CLEA_Makeup_Color", [0.0; 3], s("CLEA_Makeup_Weight", 0.0))
        } else {
            rgb("MakeupColor", [0.0; 3], s("MakeupIntensity", 0.0))
        };
        v[9] = [
            if body {
                s("CLEA_Makeup_Roughness", 0.2)
            } else {
                s("MakeupRoughness", 0.5)
            },
            s(tci, 0.0),
            s("TattooCurvatureInfluence", 1.0),
            s("TattooRoughnessOffset", 0.0),
        ];
        v[10] = tattoo;
        let zero = [0.0; 3];
        v[11] = rgb(tc, zero, s("TattooMetalness", 0.0));
        v[12] = rgb(&format!("{tc}G"), zero, s("TMP_BODY_ROUGH", 0.56));
        v[13] = rgb(&format!("{tc}B"), zero, s("MakeUpIndex", 0.0));
        v[14] = rgb(&format!("{tc}A"), zero, if body { 0.0 } else { 1.0 });
        v[15] = rgb(
            "Lips_Makeup_Color",
            [0.448, 0.147, 0.14],
            s("LipsMakeupIntensity", 0.0),
        );
        v[16] = rgb("CustomColor", [0.027; 3], s("CustomIntensity", 0.0));
        v[17] = [
            s("LipsMakeupRoughness", 1.0),
            s("CustomIndex", 0.0),
            s("EyesMakeupMetalness", 0.0),
            s("LipsMakeupMetalness", 0.0),
        ];
        v[18] = [
            s("ScalpDesaturation", 0.0),
            s("ScalpDarken", 0.0),
            s("BeardDesaturation", 0.0),
            s("BeardDarken", 0.0),
        ];
        // Atlas grids: body tattoos 4×5, head tattoos/make-up 4×4.
        v[19] = [
            s("MakeupThicknessExclusion", 0.1),
            if body { 1.0 } else { 0.0 },
            4.0,
            if body { 5.0 } else { 4.0 },
        ];
        Self { v }
    }

    pub fn kind(&self) -> SkinKind {
        if self.v[14][3] > 0.5 {
            SkinKind::Head
        } else {
            SkinKind::Body
        }
    }
}

/// Texture samples at one texel, as the shader sees them (linear values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinSample {
    /// Hemoglobin, melanin (inverted: 1 = none), vein, yellowing.
    pub hmvy: [f32; 4],
    /// Curvature, body hair / eyebrows, make-up (body) / lips (head), cavity (alpha out).
    /// Bodies read this from the albedo VT layer, gamma-encoded (see [`clea_from_srgb_vt`]).
    pub clea: [f32; 4],
    /// `SKIN_Shared_Shading_MSK(A)`: body x freckles, y non-SSS, z vitiligo;
    /// head x roughness, y freckles, z vitiligo, w non-SSS.
    pub shading: [f32; 4],
    /// `CancelMSK`: x non-skin, y melanin removal.
    pub cancel: [f32; 4],
    /// `BaseColor_NonSkin` (bodies use rgb, heads only r).
    pub non_skin: [f32; 4],
    /// Tattoo atlas cell at this texel.
    pub tattoo: [f32; 4],
    /// Make-up atlas cell (head): x eye make-up; and the `CustomIndex` cell's y.
    pub makeup: [f32; 4],
    pub custom: [f32; 4],
    /// Head dirt map's alpha: scalp (>0.5) / beard (<0.5) stubble area.
    pub stubble: f32,
}

/// What the skin shaders write to the G-buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinSurface {
    /// Linear albedo.
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    /// G-buffer "specular" (0.5 = the usual 4 % reflectance).
    pub specular: f32,
    /// Subsurface amount (G-buffer slot 3, y).
    pub subsurface: f32,
    /// Passed through from `clea.w`.
    pub cavity: f32,
}

/// Body skin reads its CLEA channels from the albedo VT layer, which the hardware decodes as
/// sRGB; the shader re-encodes with `pow(x, 1/2.2)` (not the exact sRGB inverse).
pub fn clea_from_srgb_vt(decoded: [f32; 3]) -> [f32; 3] {
    decoded.map(|c| c.abs().max(1e-4).powf(1.0 / 2.2))
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}
fn sat(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn rgb(v: [f32; 4]) -> [f32; 3] {
    [v[0], v[1], v[2]]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
/// `pow` as the shaders evaluate it: `max(|x|, 1e-4)^y`.
fn spow(x: f32, y: f32) -> f32 {
    x.abs().max(1e-4).powf(y)
}

/// Piecewise melanin response: linear up to the dark threshold `t`, then rising to the dark
/// multiplier `d` at full concentration.
pub fn melanin_response(m: f32, t: f32, d: f32) -> f32 {
    let below = sat(m / t);
    let above = sat((m - t) / (1.0 - t));
    mix(mix(0.0, t, below), d, above)
}

/// Absorption weight from a concentration: `1 − e^(−c²)`.
pub fn absorption(c: f32) -> f32 {
    1.0 - 1.0 / std::f32::consts::E.max(1e-4).powf(spow(c, 2.0))
}

/// The pigment colour before any layers: hemoglobin tinted by veins, darkened by melanin,
/// yellowed, tinted by [`SKIN_TINT`] and desaturated where both pigments are dense.
/// Returns the colour and the two absorption weights.
pub fn pigment(u: &SkinUniform, s: &SkinSample) -> [f32; 3] {
    let v = &u.v;
    let head = u.kind() == SkinKind::Head;
    let skin_weight = v[5][3];
    let hemoglobin = sat(v[0][3] * s.hmvy[0]);
    let mut melanin = melanin_response(1.0 - s.hmvy[1], v[4][0], v[4][1]) + v[2][3];
    // Vitiligo: a smoothstep window around the threshold on the shading map's blue channel.
    let vit = v[4][2].clamp(0.0, 0.95);
    let (lo, hi) = (vit - 0.05, (vit + 0.05) * vit.ceil());
    let t = sat((s.shading[2] - lo) / (hi - lo));
    let vitiligo = t * t * (3.0 - 2.0 * t);
    melanin *= vitiligo.max(1.0 - v[4][3] * skin_weight);
    let freckle_map = if head { s.shading[1] } else { s.shading[0] };
    let freckles = v[5][0];
    melanin +=
        ((freckle_map + freckles - 1.0) * (v[5][1] * skin_weight) / freckles.max(0.01)).max(0.0);
    melanin = (melanin * (1.0 - v[5][2] * s.cancel[1])).max(0.0);

    let h = absorption(hemoglobin);
    let m = absorption(melanin);
    let mut c = rgb(v[0]).map(|x| spow(x, h));
    c = mix3(c, rgb(v[1]), sat(v[1][3] * s.hmvy[2]));
    let mel = rgb(v[2]).map(|x| spow(x, m));
    c = [c[0] * mel[0], c[1] * mel[1], c[2] * mel[2]];
    let yellow = [c[0] * v[3][0], c[1] * v[3][1], c[2] * v[3][2]];
    c = mix3(c, yellow, v[3][3] * s.hmvy[3]);
    c = [
        c[0] * SKIN_TINT[0],
        c[1] * SKIN_TINT[1],
        c[2] * SKIN_TINT[2],
    ];
    let l = dot3(c, LUMA);
    mix3([l; 3], c, 1.0 - h * m)
}

/// Full skin surface (no wounds/dirt/sweat state, no scars/age/bloodline layers).
pub fn shade(u: &SkinUniform, s: &SkinSample) -> SkinSurface {
    let v = &u.v;
    let head = u.kind() == SkinKind::Head;
    let mut c = pigment(u, s);
    let curvature = s.clea[0];

    if head {
        // Scalp/beard stubble: desaturate and darken by area.
        let k = s.stubble * 2.0 - 1.0;
        let scalp = [k * v[18][0], k * v[18][1]];
        let beard = [-k * v[18][2], -k * v[18][3]];
        let f = [1.0 - scalp[0].max(beard[0]), 1.0 - scalp[1].max(beard[1])];
        let l = dot3(c, LUMA);
        c = mix3([l; 3], c, f[0]).map(|x| x * f[1]);
        // Lips, custom paint.
        c = mix3(c, rgb(v[15]), s.clea[2] * v[15][3]);
        c = mix3(c, rgb(v[16]), v[16][3] * curvature * s.custom[1]);
    }

    // Tattoos: four colour channels of an atlas cell, optionally shaded by curvature.
    let t4: [f32; 4] = std::array::from_fn(|i| {
        let t = mix(s.tattoo[i], s.tattoo[i] * curvature, v[9][2]);
        t * v[10][i]
    });
    let t_sum = t4.iter().sum::<f32>();
    let tint: [f32; 3] = std::array::from_fn(|k| {
        t4[0] * v[11][k] + t4[1] * v[12][k] + t4[2] * v[13][k] + t4[3] * v[14][k]
    });
    c = std::array::from_fn(|k| c[k] * (1.0 - t_sum) + tint[k]);
    let tattoo_mask = t_sum.min(1.0);

    let makeup = if head {
        let m = v[8][3] * s.makeup[0];
        c = mix3(c, rgb(v[8]), m);
        // Eyebrows.
        let brow = rgb(v[7]).map(|x| x * s.clea[1]);
        c = mix3(c, brow, s.clea[1]);
        m
    } else {
        let hair = rgb(v[7]).map(|x| x * curvature);
        c = mix3(c, hair, s.clea[1]);
        let m = s.clea[2] * v[8][3];
        c = mix3(c, rgb(v[8]), m);
        m
    };

    // Non-skin parts (nails, scars painted as non-skin, …).
    let non_skin_w = v[6][3] * s.cancel[0];
    let ns = if head {
        [s.non_skin[0]; 3]
    } else {
        rgb(s.non_skin)
    };
    c = mix3(
        c,
        [ns[0] * v[6][0], ns[1] * v[6][1], ns[2] * v[6][2]],
        non_skin_w,
    );

    // Roughness.
    let mut r;
    if head {
        let base = sat(1.0 - curvature + curvature * s.shading[0]);
        r = mix((1.0 - s.clea[2]) * base, base, v[17][0]);
        r = mix(r, 0.8, s.clea[1]);
    } else {
        r = sat(v[12][3] * curvature + 1.0 - curvature);
        r = mix(r, 0.8, s.clea[1]);
    }
    let tattoo_r = mix(tattoo_mask, curvature * tattoo_mask, v[9][2]);
    r = mix(r, r + v[9][3], tattoo_r);
    r = sat(if head {
        mix(r, v[9][0], makeup)
    } else {
        mix(r, v[9][0], s.clea[2])
    });

    // Metalness.
    let tattoo_metal = tattoo_r * v[11][3];
    let metallic = if head {
        let lips_metal = s.clea[2] * v[17][3];
        let m = (makeup * v[17][2]).max(lips_metal).max(tattoo_metal);
        let m = (non_skin_w * v[7][3]).max(m);
        (1.0 - non_skin_w * (1.0 - v[7][3])) * m
    } else {
        (non_skin_w * v[7][3]).max(tattoo_metal)
    };

    // Subsurface amount: off on non-skin, eyebrows (head), metal and flagged texels.
    let subsurface = if head {
        let ex = v[19][0].max(1.0 - makeup);
        sat(ex
            * (1.0 - s.cancel[0])
            * (1.0 - s.shading[3].floor())
            * (1.0 - s.clea[1])
            * (1.0 - metallic))
    } else {
        sat((1.0 - s.shading[1].floor()) * (1.0 - metallic))
    };

    SkinSurface {
        albedo: c.map(sat),
        roughness: sat(r),
        metallic: sat(metallic),
        specular: 0.5,
        subsurface,
        cavity: s.clea[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(kind: SkinKind, f: impl FnOnce(&mut ParamSet)) -> SkinUniform {
        let mut p = ParamSet::default();
        f(&mut p);
        SkinUniform::new(kind, &p)
    }

    fn sample(hmvy: [f32; 4]) -> SkinSample {
        SkinSample {
            hmvy,
            clea: [0.0; 4],
            shading: [0.0; 4],
            cancel: [0.0; 4],
            non_skin: [0.0; 4],
            tattoo: [0.0; 4],
            makeup: [0.0; 4],
            custom: [0.0; 4],
            stubble: 0.5,
        }
    }

    #[test]
    fn melanin_response_is_piecewise_linear() {
        assert!((melanin_response(0.45, 0.9, 2.0) - 0.45).abs() < 1e-6);
        assert!((melanin_response(0.9, 0.9, 2.0) - 0.9).abs() < 1e-6);
        assert!((melanin_response(0.95, 0.9, 2.0) - 1.45).abs() < 1e-5);
        assert!((melanin_response(1.0, 0.9, 2.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn absorption_is_one_minus_gaussian() {
        assert!(absorption(0.0).abs() < 1e-6);
        assert!((absorption(1.0) - (1.0 - (-1.0f32).exp())).abs() < 1e-5);
        assert!(absorption(3.0) > 0.999);
    }

    #[test]
    fn no_pigment_gives_the_tinted_white() {
        // Zero hemoglobin and melanin: every pigment power is 0, the product is 1.
        let u = uniform(SkinKind::Body, |p| {
            p.set("MelaninAmount", vec![0.0]);
            p.set("VeinAmount", vec![0.0]);
            p.set("YellowingAmount", vec![0.0]);
        });
        let c = pigment(&u, &sample([0.0, 1.0, 0.0, 0.0]));
        for k in 0..3 {
            assert!((c[k] - SKIN_TINT[k]).abs() < 1e-4, "{c:?}");
        }
    }

    #[test]
    fn melanin_colour_drives_the_hue() {
        // A grey-lilac melanin colour (deep gnome-like) with dense melanin stays bluish.
        let u = uniform(SkinKind::Body, |p| {
            p.set("MelaninColor", vec![0.2, 0.18, 0.3]);
            p.set("MelaninAmount", vec![1.5]);
            p.set("HemoglobinAmount", vec![0.0]);
            p.set("VeinAmount", vec![0.0]);
            p.set("YellowingAmount", vec![0.0]);
        });
        let c = pigment(&u, &sample([0.0, 1.0, 0.0, 0.0]));
        assert!(
            c[2] / c[0] > 0.3 / 0.2 * SKIN_TINT[2] / SKIN_TINT[0] * 0.95,
            "{c:?}"
        );
    }

    #[test]
    fn hemoglobin_reddens_and_veins_mix_in() {
        let u = uniform(SkinKind::Body, |p| {
            p.set("MelaninAmount", vec![0.0]);
            p.set("YellowingAmount", vec![0.0]);
            p.set("HemoglobinColor", vec![0.65, 0.009, 0.009]);
            p.set("VeinColor", vec![0.0, 0.0, 1.0]);
            p.set("VeinAmount", vec![1.0]);
        });
        let red = pigment(&u, &sample([1.0, 1.0, 0.0, 0.0]));
        assert!(red[0] > red[1] * 1.5, "{red:?}");
        let vein = pigment(&u, &sample([1.0, 1.0, 1.0, 0.0]));
        assert!(vein[2] > vein[0], "{vein:?}");
    }

    #[test]
    fn vitiligo_removes_melanin_only_inside_the_mask() {
        let u = uniform(SkinKind::Body, |p| {
            p.set("Vitiligo", vec![0.5]);
            p.set("VitiligoWeight", vec![1.0]);
            p.set("MelaninAmount", vec![1.0]);
        });
        let mut inside = sample([0.0, 1.0, 0.0, 0.0]);
        inside.shading[2] = 0.0;
        let mut outside = inside;
        outside.shading[2] = 1.0;
        let a = pigment(&u, &inside);
        let b = pigment(&u, &outside);
        assert!(dot3(a, LUMA) > dot3(b, LUMA) * 2.0, "{a:?} {b:?}");
    }

    #[test]
    fn head_layers_and_roughness() {
        let u = uniform(SkinKind::Head, |p| {
            p.set("Eyebrow_Color", vec![0.1, 0.05, 0.02]);
            p.set("MakeupColor", vec![0.0, 0.0, 0.0]);
            p.set("MakeupIntensity", vec![1.0]);
            p.set("MakeupRoughness", vec![0.5]);
            p.set("LipsMakeupRoughness", vec![1.0]);
        });
        let mut s = sample([0.5, 0.5, 0.0, 0.0]);
        s.clea = [1.0, 0.0, 0.0, 0.7];
        s.shading[0] = 0.4;
        let plain = shade(&u, &s);
        assert!((plain.roughness - 0.4).abs() < 1e-5);
        assert_eq!(plain.cavity, 0.7);
        // Full eye make-up paints black and sets the make-up roughness.
        s.makeup[0] = 1.0;
        let painted = shade(&u, &s);
        assert!(painted.albedo.iter().all(|&x| x < 1e-6), "{painted:?}");
        assert!((painted.roughness - 0.5).abs() < 1e-5);
        assert!(painted.subsurface < plain.subsurface);
    }

    #[test]
    fn body_tattoo_replaces_skin_by_its_weight() {
        let u = uniform(SkinKind::Body, |p| {
            p.set("BodyTattooIntensity", vec![1.0, 0.0, 0.0, 0.0]);
            p.set("BodyTattooColor", vec![0.0, 1.0, 0.0]);
            p.set("TattooCurvatureInfluence", vec![0.0]);
        });
        let mut s = sample([0.3, 0.5, 0.0, 0.0]);
        s.tattoo = [1.0, 0.0, 0.0, 0.0];
        assert_eq!(shade(&u, &s).albedo, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn genital_material_disables_tattoos() {
        let u = uniform(SkinKind::Body, |p| {
            p.set("isGenitals", vec![1.0]);
            p.set("BodyTattooIntensity", vec![1.0; 4]);
        });
        assert_eq!(u.v[10], [0.0; 4]);
        assert_eq!(u.v[5][3], 0.0);
    }
}
