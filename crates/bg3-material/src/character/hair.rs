//! Hair: `CHAR_Hair` (scalp hair and beards; `DontTouchMe_isBeard` picks the beard colours).
//!
//! bg3: Shared_Characters_CHAR_Hair_ST_DEF_fragmentMain, _ST_DEP_fragmentMain
//! (Materials.metallib, 4.1.1.7631656). Formulas: `docs/research/character-shaders.md`.

use super::kinds::UNIFORM_LEN;
use super::params::ParamSet;

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Hair uniform layout (see `character.wesl`, `hair()`):
/// v0 (colour, graying·0.8), v1 (graying colour, Graying_Seed), v2 (highlight colour,
/// Highlight_Intensity), v3 (Highlight_Falloff, RootTransitionMidPoint, RootTransitionSoftness,
/// DepthColorExponent), v4 (DepthColorIntensity, IDContrast, ColorDepthContrast, HairFrizz),
/// v5 (HueShiftColorWeight, SharedNoiseTiling), v6 (Roughness, RoughnessContrast,
/// SoftenTipsAlpha, DontTouchMe_Beard_Alpha), v7 (DreadNoiseBaseColor).
pub fn uniform(v: &mut [[f32; 4]; UNIFORM_LEN], p: &ParamSet) {
    let s = |n: &str, d: f32| p.scalar(n, d);
    let beard = s("DontTouchMe_isBeard", 0.0);
    let pick = |hair: &str, beard_name: &str, d: [f32; 3]| {
        mix3(p.rgb(hair, d), p.rgb(beard_name, d), beard)
    };
    let colour = pick("Hair_Color", "Beard_Color", [0.1361, 0.0824, 0.043]);
    let gray = pick("Hair_Graying_Color", "Beard_Graying_Color", [0.7317; 3]);
    let highlight = pick(
        "Highlight_Color",
        "Beard_Highlight_Color",
        [0.3531, 0.1078, 0.1888],
    );
    let graying = {
        let h = s("Graying_Intensity", 0.0);
        h + (s("Beard_Graying_Intensity", 0.0) - h) * beard
    };
    v[0] = [colour[0], colour[1], colour[2], graying * 0.8];
    v[1] = [gray[0], gray[1], gray[2], s("Graying_Seed", 0.0)];
    v[2] = [
        highlight[0],
        highlight[1],
        highlight[2],
        s("Highlight_Intensity", 0.0),
    ];
    v[3] = [
        s("Highlight_Falloff", 0.0),
        s("RootTransitionMidPoint", 0.1),
        s("RootTransitionSoftness", 0.2),
        s("DepthColorExponent", 0.0),
    ];
    v[4] = [
        s("DepthColorIntensity", 0.0),
        s("IDContrast", 0.5),
        s("ColorDepthContrast", 0.5),
        s("HairFrizz", 0.0005),
    ];
    let [r, g, b] = p.rgb("HueShiftColorWeight", [0.3; 3]);
    v[5] = [r, g, b, s("SharedNoiseTiling", 50.0)];
    v[6] = [
        s("Roughness", 0.41),
        s("RoughnessContrast", 0.05),
        s("SoftenTipsAlpha", 0.0),
        s("DontTouchMe_Beard_Alpha", 1.0),
    ];
    v[7] = [s("DreadNoiseBaseColor", 0.0), 0.0, 0.0, 0.0];
}

/// Strand colour before hue variation and dirt (reference for the shader's `hair()`):
/// `id`, `depth`, `root` from the hair mask, `uv_y` the texture v, `vc` the vertex colour.
pub fn strand_colour(
    v: &[[f32; 4]; UNIFORM_LEN],
    id: f32,
    depth: f32,
    root: f32,
    uv_y: f32,
    vc: [f32; 4],
) -> ([f32; 3], f32) {
    let sat = |x: f32| x.clamp(0.0, 1.0);
    let g = v[0][3];
    let f = uv_y.fract() * (1.0 - 4.0 * (g - 0.75).max(0.0));
    let seed = (vc[0] + v[1][3]).fract() * 0.4;
    let gray_mask = sat(g.ceil() * (g - 1.0 + (f - (2.0 * id - 0.5 + seed)).abs()));
    let rgb = |i: usize| [v[i][0], v[i][1], v[i][2]];
    let c1 = mix3(rgb(0), rgb(1), gray_mask);
    let falloff = v[3][0];
    let h = v[2][3] * sat(sat(vc[2] - falloff) * 1.5 / (1.0 - falloff));
    let c2 = mix3(c1, rgb(2), h);
    let soft = sat(v[3][2]);
    let t = sat((root + 0.5 * soft - v[3][1]) / soft);
    let r = t * t * (3.0 - 2.0 * t);
    let d = sat(4.0 * uv_y).min(vc[3]).min(r);
    let rr = sat(r + (d.max(1e-4).powf(v[3][3]) - r) * v[4][0]);
    let half = c2.map(|x| 0.5 * x);
    let l = 0.2126 * half[0] + 0.7152 * half[1] + 0.0722 * half[2];
    let dark = mix3([l; 3], half, 1.2);
    let c3 = mix3(dark, c2, rr);
    let k = 1.0 + (1.8 * id + 0.2 - 1.0) * v[4][1];
    let kd = 1.0 + (1.5 * depth.max(1e-4).powf(2.2) + 0.5 - 1.0) * v[4][2];
    (c3.map(|x| x * k * kd), gray_mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_with(f: impl FnOnce(&mut ParamSet)) -> [[f32; 4]; UNIFORM_LEN] {
        let mut p = ParamSet::default();
        f(&mut p);
        let mut v = [[0.0; 4]; UNIFORM_LEN];
        uniform(&mut v, &p);
        v
    }

    #[test]
    fn beards_use_beard_colours() {
        let v = uniform_with(|p| {
            p.set("Hair_Color", vec![1.0, 0.0, 0.0]);
            p.set("Beard_Color", vec![0.0, 1.0, 0.0]);
            p.set("DontTouchMe_isBeard", vec![1.0]);
        });
        assert_eq!(v[0][..3], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn tips_keep_the_colour_and_roots_darken() {
        let v = uniform_with(|p| {
            p.set("Hair_Color", vec![0.8, 0.6, 0.4]);
            p.set("IDContrast", vec![0.0]);
            p.set("ColorDepthContrast", vec![0.0]);
        });
        let (tip, gray) = strand_colour(&v, 0.5, 0.0, 1.0, 0.5, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(gray, 0.0);
        for (a, b) in tip.iter().zip([0.8, 0.6, 0.4]) {
            assert!((a - b).abs() < 1e-5, "{tip:?}");
        }
        let (root, _) = strand_colour(&v, 0.5, 0.0, 0.0, 0.5, [0.0, 0.0, 0.0, 1.0]);
        assert!(root[0] < tip[0] * 0.6, "{root:?}");
    }

    #[test]
    fn full_graying_turns_strands_grey() {
        let v = uniform_with(|p| {
            p.set("Hair_Color", vec![0.1, 0.05, 0.02]);
            p.set("Hair_Graying_Color", vec![0.7; 3]);
            p.set("Graying_Intensity", vec![2.0]);
            p.set("IDContrast", vec![0.0]);
            p.set("ColorDepthContrast", vec![0.0]);
        });
        let (c, gray) = strand_colour(&v, 0.5, 0.0, 1.0, 0.3, [0.0; 4]);
        assert_eq!(gray, 1.0);
        assert!((c[0] - 0.7).abs() < 1e-5, "{c:?}");
    }
}
