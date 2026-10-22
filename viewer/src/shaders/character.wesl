// Character materials: BG3's skin, clothing palette, eye, hair and eyelash graphs on top of
// Bevy's PBR. Written from the formulas in docs/research/character-shaders.md; the skin math
// mirrors `bg3_material::character::skin` (unit-tested there). Each mode reproduces what the
// game's deferred pass writes to its G-buffer (albedo, roughness, reflectance, metalness,
// occlusion); Bevy's lighting then shades it.
//
// bg3: Shared_Characters_CHAR_Skin_Body_ST_DEF_fragmentMain and the other
// Shared_Characters_<graph>_ST_DEF_fragmentMain entry points (Materials.metallib, 4.1.1.7631656)

import bevy_pbr::render::{
    pbr_types,
    pbr_functions::alpha_discard,
    pbr_fragment::pbr_input_from_standard_material,
};

@if(PREPASS_PIPELINE)
import bevy_pbr::{
    prepass::io::{VertexOutput, FragmentOutput},
    deferred::functions::deferred_output,
};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    clustered_forward as clustering,
    shadows,
    pbr_ambient as ambient,
};
@if(!PREPASS_PIPELINE)
import bg3_viewer::shaders::game_light::bg3_light_falloff;
@if(!PREPASS_PIPELINE && ENVIRONMENT_MAP)
import bevy_pbr::{
    light_probe::environment_map,
    render::pbr_lighting::{LightingInput, LAYER_BASE, F_AB},
};

const MODE_SKIN: u32 = 0u;
const MODE_CLOTH_PALETTE: u32 = 1u;
const MODE_CLOTH_MSK: u32 = 2u;
const MODE_EYE: u32 = 3u;
const MODE_HAIR: u32 = 4u;
const MODE_LASHES: u32 = 5u;

struct CharacterUniform {
    mode: u32,
    flags: u32,
    _pad0: u32,
    _pad1: u32,
    v: array<vec4<f32>, 24>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> cu: CharacterUniform;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var t0: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var s0: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var t1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var t2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var t3: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var t4: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var t5: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var t6: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var t7: texture_2d<f32>;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const SKIN_TINT: vec3<f32> = vec3<f32>(0.95222, 0.7440979, 0.5952425);

// `pow` as the game's shaders evaluate it.
fn spow(x: f32, y: f32) -> f32 { return pow(max(abs(x), 1e-4), y); }
fn spow3(x: vec3<f32>, y: vec3<f32>) -> vec3<f32> { return pow(max(abs(x), vec3(1e-4)), y); }

fn absorption(c: f32) -> f32 {
    return 1.0 - 1.0 / pow(2.7182817, spow(c, 2.0));
}

fn melanin_response(m: f32, t: f32, d: f32) -> f32 {
    let below = clamp(m / t, 0.0, 1.0);
    let above = clamp((m - t) / (1.0 - t), 0.0, 1.0);
    return mix(mix(0.0, t, below), d, above);
}

// Sample cell `index` of a cols×rows atlas at `uv`.
fn atlas_uv(index: f32, cols: f32, rows: f32, uv: vec2<f32>) -> vec2<f32> {
    let i = index + 1e-5;
    let cell = trunc(vec2(i % cols, (i / cols) % rows));
    return (cell + uv) / vec2(cols, rows);
}

struct Surface {
    albedo: vec3<f32>,
    alpha: f32,
    roughness: f32,
    metallic: f32,
    // G-buffer reflectance (0.5 = 4 % F0).
    specular: f32,
    occlusion: f32,
    emissive: vec3<f32>,
    // Subsurface (G-buffer target 3 of shading model 3): scatter amount and thickness map.
    sss_amount: f32,
    thickness: f32,
    // Hair (target 3 of model 5): world strand tangent, scatter; backlit (target 2, blue).
    tangent: vec3<f32>,
    scatter: f32,
    backlit: f32,
}

fn surface_new() -> Surface {
    return Surface(vec3(1.0), 1.0, 0.5, 0.0, 0.5, 1.0, vec3(0.0), 1.0, 0.0, vec3(0.0, 1.0, 0.0), 0.3, 0.4);
}

// ---- Skin (CHAR_Skin_Body, CHAR_Skin_Head_v3, CHAR_Skin_Head_Creature) ----
// t0 HMVY, t1 CLEA, t2 shading mask, t3 CancelMSK, t4 non-skin colour, t5 tattoo atlas,
// t6 make-up atlas (head), t7 head dirt map (alpha: scalp/beard).
fn skin(uv: vec2<f32>) -> Surface {
    let v = cu.v;
    let head = v[14].w > 0.5;
    let hmvy = textureSample(t0, s0, uv);
    var clea = textureSample(t1, s0, uv);
    if v[19].y > 0.5 {
        // Bodies read CLEA from the sRGB albedo VT layer and re-encode it.
        clea = vec4(spow3(clea.rgb, vec3(0.45454550)), clea.a);
    }
    let shading = textureSample(t2, s0, uv);
    let cancel = textureSample(t3, s0, uv);
    let skin_weight = v[5].w;

    // Pigments.
    let hemoglobin = clamp(v[0].w * hmvy.x, 0.0, 1.0);
    var melanin = melanin_response(1.0 - hmvy.y, v[4].x, v[4].y) + v[2].w;
    let vit = clamp(v[4].z, 0.0, 0.95);
    let lo = vit - 0.05;
    let hi = (vit + 0.05) * ceil(vit);
    let vt = clamp((shading.z - lo) / (hi - lo), 0.0, 1.0);
    melanin *= max(vt * vt * (3.0 - 2.0 * vt), 1.0 - v[4].w * skin_weight);
    let freckle_map = select(shading.x, shading.y, head);
    let freckles = v[5].x;
    melanin += max((freckle_map + freckles - 1.0) * (v[5].y * skin_weight) / max(freckles, 0.01), 0.0);
    melanin = max(melanin * (1.0 - v[5].z * cancel.y), 0.0);
    let h = absorption(hemoglobin);
    let m = absorption(melanin);
    var c = spow3(v[0].rgb, vec3(h));
    c = mix(c, v[1].rgb, clamp(v[1].w * hmvy.z, 0.0, 1.0));
    c *= spow3(v[2].rgb, vec3(m));
    c = mix(c, c * v[3].rgb, v[3].w * hmvy.w);
    c *= SKIN_TINT;
    c = mix(vec3(dot(c, LUMA)), c, 1.0 - h * m);

    let curvature = clea.x;
    if head {
        let stubble = textureSample(t7, s0, uv).a * 2.0 - 1.0;
        let f = 1.0 - max(stubble * v[18].xy, -stubble * v[18].zw);
        c = mix(vec3(dot(c, LUMA)), c, f.x) * f.y;
        c = mix(c, v[15].rgb, clea.z * v[15].w);
        let custom = textureSample(t6, s0, atlas_uv(v[17].y, 4.0, 4.0, uv)).y;
        c = mix(c, v[16].rgb, v[16].w * curvature * custom);
    }

    // Tattoos.
    let tat = textureSample(t5, s0, atlas_uv(v[9].y, v[19].z, v[19].w, uv));
    let t4v = mix(tat, tat * curvature, v[9].z) * v[10];
    let t_sum = dot(t4v, vec4(1.0));
    c = c * (1.0 - t_sum) + t4v.x * v[11].rgb + t4v.y * v[12].rgb + t4v.z * v[13].rgb + t4v.w * v[14].rgb;
    let tattoo_mask = min(t_sum, 1.0);

    var makeup: f32;
    if head {
        makeup = v[8].w * textureSample(t6, s0, atlas_uv(v[13].w, 4.0, 4.0, uv)).x;
        c = mix(c, v[8].rgb, makeup);
        c = mix(c, v[7].rgb * clea.y, clea.y);
    } else {
        c = mix(c, v[7].rgb * curvature, clea.y);
        makeup = clea.z * v[8].w;
        c = mix(c, v[8].rgb, makeup);
    }

    let non_skin_w = v[6].w * cancel.x;
    var ns = textureSample(t4, s0, uv).rgb;
    if head { ns = vec3(ns.x); }
    c = mix(c, ns * v[6].rgb, non_skin_w);

    var r: f32;
    if head {
        let base = clamp(1.0 - curvature + curvature * shading.x, 0.0, 1.0);
        r = mix((1.0 - clea.z) * base, base, v[17].x);
    } else {
        r = clamp(v[12].w * curvature + 1.0 - curvature, 0.0, 1.0);
    }
    r = mix(r, 0.8, clea.y);
    let tattoo_r = mix(tattoo_mask, curvature * tattoo_mask, v[9].z);
    r = mix(r, r + v[9].w, tattoo_r);
    r = clamp(mix(r, v[9].x, select(clea.z, makeup, head)), 0.0, 1.0);

    let tattoo_metal = tattoo_r * v[11].w;
    var metal: f32;
    if head {
        let mm = max(max(makeup * v[17].z, clea.z * v[17].w), tattoo_metal);
        metal = (1.0 - non_skin_w * (1.0 - v[7].w)) * max(non_skin_w * v[7].w, mm);
    } else {
        metal = max(non_skin_w * v[7].w, tattoo_metal);
    }

    var s = surface_new();
    s.albedo = clamp(c, vec3(0.0), vec3(1.0));
    s.roughness = r;
    s.metallic = clamp(metal, 0.0, 1.0);
    s.occlusion = clea.w;
    // Subsurface amount and thickness map (G-buffer target 3: y, z). Off on non-skin,
    // eyebrows, metal and texels the shading mask flags (value 1).
    if head {
        let flagged = floor(shading.w);
        s.sss_amount = clamp(max(v[19].x, 1.0 - makeup) * (1.0 - cancel.x) * (1.0 - flagged)
            * (1.0 - clea.y) * (1.0 - s.metallic), 0.0, 1.0);
        s.thickness = clamp(max(s.metallic, clea.y + cancel.x + flagged), 0.0, 1.0);
    } else {
        let flagged = floor(shading.y);
        s.sss_amount = clamp((1.0 - flagged) * (1.0 - s.metallic), 0.0, 1.0);
        // Assumption: the body's thickness term mirrors the head's (not fully traced).
        s.thickness = clamp(max(s.metallic, cancel.x + flagged), 0.0, 1.0);
    }
    return s;
}

// ---- Clothing: 12-colour palette (CHAR_BASE_VT family) ----
// t0 MSKColor. The palette points are fixed per parameter; v[0..12] hold the colours.
fn palette_point(i: u32) -> vec3<f32> {
    switch i {
        case 0u: { return vec3(1.0, 0.5, 0.0); }   // Cloth_Primary
        case 1u: { return vec3(1.0, 0.0, 0.0); }   // Cloth_Secondary
        case 2u: { return vec3(1.0, 0.5, 0.5); }   // Cloth_Tertiary
        case 3u: { return vec3(1.0, 0.0, 0.5); }   // Accent_Color
        case 4u: { return vec3(0.5, 0.0, 1.0); }   // Leather_Primary
        case 5u: { return vec3(0.0, 0.0, 1.0); }   // Leather_Secondary
        case 6u: { return vec3(0.5, 0.5, 1.0); }   // Leather_Tertiary
        case 7u: { return vec3(0.0, 0.5, 1.0); }   // Custom_1
        case 8u: { return vec3(0.0, 1.0, 0.5); }   // Metal_Primary
        case 9u: { return vec3(0.0, 1.0, 0.0); }   // Metal_Secondary
        case 10u: { return vec3(0.5, 1.0, 0.5); }  // Metal_Tertiary
        default: { return vec3(0.5, 1.0, 0.0); }   // Custom_2
    }
}

fn palette_tint(uv: vec2<f32>) -> vec3<f32> {
    let m = textureSample(t0, s0, uv).rgb;
    var tint = vec3(0.0);
    for (var i = 0u; i < 12u; i++) {
        let w = 4.0 * clamp(1.0 - length(m - palette_point(i)) / 1.7320508 - 0.75, 0.0, 1.0);
        tint += w * cu.v[i].rgb;
    }
    return tint;
}

// ---- Clothing: 3-colour mask (*_MSK_* graphs) ----
// t0 MSKcloth; v[0..3] Color_01..03.
fn msk_tint(albedo: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let m = textureSample(t0, s0, uv).rgb;
    let tinted = albedo * (m.r * cu.v[0].rgb + m.g * cu.v[1].rgb + m.b * cu.v[2].rgb);
    return mix(albedo, tinted, clamp(m.r + m.g + m.b, 0.0, 1.0));
}

// ---- Eyes (CHAR_Eye_Parallax*) ----
// t0 RegionMask, t1 ColorsMask (parts), t2 blindness mask, t3 catchlight lat-long HDRI.
// v[0] iris, v[1] iris secondary, v[2] sclera, v[3] blood (rgb; w unused),
// v[4..7] the `_L` set, v[8] (SecondaryColourIntensity, _L, IrisEdgeStrength, _L),
// v[9] (Redness, _L, Blindness, _L), v[10] (Heterochromia, Ersatz, creature, catchlight on).
fn eye_parallax_uv(in: VertexOutput, uv: vec2<f32>, view: vec3<f32>) -> vec2<f32> {
@if(VERTEX_TANGENTS && !PREPASS_PIPELINE) {
    let n = normalize(in.world_normal);
    let t = normalize(in.world_tangent.xyz);
    let b = cross(n, t) * sign(in.world_tangent.w);
    let vt = normalize(vec3(dot(view, t), dot(view, n), dot(view, b)));
    let h = textureSample(t0, s0, uv).a;
    // `_Creature_FX` mixes the parallax in by ParallaxWeight (1 for the other graphs).
    return mix(uv, uv - 0.045 * (1.0 + h) * vec2(vt.x, vt.z), cu.v[11].y);
}
    return uv;
}

// The sclera detail normal (`EYES_Sclera_NM`, t4) as tangent and binormal offsets.
fn eye_sclera_offset(uv: vec2<f32>) -> vec2<f32> {
    let ns = textureSample(t4, s0, uv);
    return vec2(ns.a * 2.0 - 1.0, -(ns.g * 2.0 - 1.0));
}

// `_FX` / `_Creature_FX` glow (static unless the material sets speeds): a noise-modulated iris
// mask times GlowColour × GlowBrightness, plus the pupil glow.
// bg3: Shared_Characters_CHAR_Eye_Parallax_FX_ST_EMI_fragmentMain,
// ..._Creature_FX_ST_EMI_fragmentMain (Materials.metallib, 4.1.1.7631656)
fn eye_glow(uv: vec2<f32>, vc_r: f32, time: f32) -> vec3<f32> {
    let v = cu.v;
    let creature = v[10].z > 0.5;
    var het = vc_r * v[10].x;
    var ersatz = (1.0 - vc_r) * v[10].y;
    if creature {
        het = 0.0;
        ersatz = 0.0;
    }
    let region = textureSample(t0, s0, uv);
    let n1 = textureSample(t2, s0, uv + time * v[15].xy).r;
    let uv2 = uv + v[15].zw * (n1 * region.g * region.b);
    let colors = textureSample(t1, s0, uv2);
    let m1 = spow(1.0 - (colors.g + colors.b), mix(v[12].w, v[13].w, het));
    let s = clamp(1.0 - textureSample(t0, s0, uv2).g, 0.0, 1.0);
    let mask = m1 * s * s * (3.0 - 2.0 * s);
    let gc = mix(v[12].rgb, v[13].rgb, het);
    let n0 = textureSample(t2, s0, uv).r;
    let n3 = textureSample(t2, s0, uv + vec2(n0, n0 + time * v[14].w)).r;
    let blind = mix(v[9].z, v[9].w, vc_r);
    let bright = (1.0 - blind) * mix(v[14].x * (1.0 - ersatz), v[14].y, het);
    let pupil = gc * (1.0 - region.b) * (1.0 - ersatz) * v[14].z;
    return mask * gc * n3 * bright + pupil;
}

fn eye(uv: vec2<f32>, vc_r: f32) -> Surface {
    let v = cu.v;
    let creature = v[10].z > 0.5;
    let region = textureSample(t0, s0, uv);
    let parts = textureSample(t1, s0, uv);
    let bl = textureSample(t2, s0, uv);
    var het = vc_r * v[10].x;
    var ersatz = (1.0 - vc_r) * v[10].y;
    if creature {
        het = 0.0;
        ersatz = 0.0;
    }
    let iris_c = mix(v[0].rgb, v[4].rgb, het);
    let iris2_c = mix(v[1].rgb, v[5].rgb, het);
    let sclera_c = mix(v[2].rgb, v[6].rgb, het);
    let blood_c = mix(v[3].rgb, v[7].rgb, het);
    let k = mix(mix(v[8].x, v[8].y, het), 2.0, ersatz);
    let edge = mix(v[8].z, v[8].w, het);
    let redness = mix(v[9].x, v[9].y, het);
    let blind = mix(v[9].z, v[9].w, vc_r);

    let w = clamp(mix(k * parts.a, bl.r * bl.g * (bl.g * k), blind), 0.0, 1.0);
    var iris = mix(iris_c, iris2_c, w);
    var blind_iris = mix(vec3(0.4375, 0.4974, 0.5222), vec3(0.2611, 0.3551, 0.3715), w);
    if v[11].x > 0.5 {
        blind_iris = mix(vec3(0.43681, 0.49951, 0.52344), vec3(0.25903, 0.35374, 0.37362), w);
    }
    iris = mix(iris, blind_iris, blind);
    iris = mix(iris, mix(vec3(0.2508, 0.5989, 0.8277), vec3(0.9323, 0.5542, 0.1836), w), ersatz);
    iris *= mix(parts.g, 1.0, blind);
    iris *= 1.0 - parts.b * mix(edge, 0.5, ersatz);

    let rr = clamp(spow(parts.r, mix(redness, 1.5, ersatz)), 0.0, 1.0);
    var sclera = mix(blood_c, sclera_c, rr);
    sclera = mix(sclera, mix(vec3(0.3587, 0.0414, 0.0342), vec3(0.4045, 0.4704, 0.3112), rr), ersatz);
    let smask = region.g * region.b;
    sclera *= smask;
    // The FX graphs blend iris and sclera by the region's green alone.
    let fx = v[11].x > 0.5;
    let t = mix(select(smask, region.g, fx), 1.0 - bl.g, blind);

    var s = surface_new();
    s.albedo = clamp(mix(iris, sclera, t), vec3(0.0), vec3(1.0));
    s.roughness = mix(0.9, 0.0, t);
    s.occlusion = region.r * region.r;
    return s;
}

// Catchlight: the view direction reflected about the eye mesh's normal (with the sclera detail
// normal only), in object space, looks up a lat-long HDRI; the game adds a third of it as
// emissive, unexposed. Approximation: the game uses the unskinned bind-pose frame; we use the
// mesh's object frame.
fn eye_catchlight(in: VertexOutput, n_world: vec3<f32>, view: vec3<f32>) -> vec3<f32> {
    let r = reflect(-view, n_world);
@if(!PREPASS_PIPELINE) {
    // World → object space: the transpose of the (rotation) upper 3×3.
    let m = bevy_pbr::render::mesh_functions::get_world_from_local(in.instance_index);
    let o = normalize(vec3(dot(m[0].xyz, r), dot(m[1].xyz, r), dot(m[2].xyz, r)));
    // Our object space is BG3's mirrored along z (docs/research/coordinate-space.md).
    let uv = vec2(atan2(-o.z, o.x) / 6.2831853 + 0.5, 0.5 - asin(clamp(o.y, -1.0, 1.0)) / 3.1415927);
    return textureSampleLevel(t3, s0, uv, 0.0).rgb / 3.0 * cu.v[10].w;
}
    return vec3(0.0);
}

// ---- Hair (CHAR_Hair) ----
// t0 ID/Depth/Root/Alpha mask, t1 HAIR_Splatter_MSK (noise), t2 hue LUT (256×4).
// Uniform layout: bg3_material::character::hair::uniform. Vertex colour: r graying seed,
// b highlight mask, a root occlusion.
fn hair(uv: vec2<f32>, vc: vec4<f32>, n_dot_v: f32) -> Surface {
    let v = cu.v;
    // Frizz: two mask reads, the second shifted by noise; per channel keep the one further
    // from 0.5 (ID) / 0 (others).
    let noise = textureSample(t1, s0, uv * v[5].w).r * 2.0 - 1.0;
    let a = textureSample(t0, s0, uv + v[4].w * noise) - vec4(0.5, 0.0, 0.0, 0.0);
    let b = textureSample(t0, s0, uv) - vec4(0.5, 0.0, 0.0, 0.0);
    let m = max(sign(a), sign(b)) * max(abs(a), abs(b));
    let id = m.x + 0.5;
    let depth = m.y;
    let root = m.z;
    let alpha = m.w;

    let g = v[0].w;
    let f = fract(uv.y) * (1.0 - 4.0 * max(g - 0.75, 0.0));
    let seed = fract(vc.r + v[1].w) * 0.4;
    let gray_mask = clamp(ceil(g) * (g - 1.0 + abs(f - (2.0 * id - 0.5 + seed))), 0.0, 1.0);
    let c1 = mix(v[0].rgb, v[1].rgb, gray_mask);
    let falloff = v[3].x;
    let h = v[2].w * clamp(clamp(vc.b - falloff, 0.0, 1.0) * 1.5 / (1.0 - falloff), 0.0, 1.0);
    let c2 = mix(c1, v[2].rgb, h);
    let soft = clamp(v[3].z, 0.0, 1.0);
    let t = clamp((root + 0.5 * soft - v[3].y) / soft, 0.0, 1.0);
    let r = t * t * (3.0 - 2.0 * t);
    let d = min(min(clamp(4.0 * uv.y, 0.0, 1.0), vc.a), r);
    let rr = clamp(mix(r, spow(d, v[3].w), v[4].x), 0.0, 1.0);
    let half_c = 0.5 * c2;
    let dark = mix(vec3(dot(half_c, LUMA)), half_c, 1.2);
    var c = mix(dark, c2, rr);
    c *= mix(1.0, 1.8 * id + 0.2, v[4].y);
    c *= mix(1.0, 1.5 * spow(depth, 2.2) + 0.5, v[4].z);
    let lut = textureSampleLevel(t2, s0, vec2(clamp(id, 0.0, 1.0), 0.0), 0.0).rgb;
    c *= max(vec3(0.0), (lut - 0.5) * v[5].rgb * gray_mask + 1.0);
    c += noise * v[7].x;

    var s = surface_new();
    s.albedo = clamp(c, vec3(0.0), vec3(1.0));
    s.roughness = clamp(v[6].x * mix(1.0, 1.5 * (1.0 - depth) + 0.5, v[6].y), 0.0, 1.0);
    // Coverage as the game's depth prepass computes it (tested against 1/3 without TAA).
    s.alpha = alpha * (1.0 - root * v[6].z) * clamp(5.0 * abs(n_dot_v), 0.0, 1.0) * v[6].w;
    // Strand tangent in tangent space (x tangent, y normal, z binormal): the binormal tilted
    // toward the normal per strand ID.
    s.tangent = vec3(0.0, (1.0 - 2.0 * id) * v[7].y + noise * v[7].z, 1.0);
    let gray_sum = clamp(v[1].x + v[1].y + v[1].z, 0.0, 1.0);
    s.scatter = clamp(clamp(v[8].x + gray_mask * (2.0 * gray_sum - 1.0), 0.0, 1.0), 0.01, 1.0);
    s.backlit = clamp(v[8].y, 0.0, 1.0);
    return s;
}

// ---- Eyelash shadow card (CHAR_LashesShadow), alpha-blended ----
// t0 lash mask (R lash cards, G coverage, B root gradient).
// v[0] (Eyelashes_Color.rgb, GradientIntensity), v[1] (LashesAlphaIntensity, ShadowAlphaIntensity).
fn lashes(uv: vec2<f32>) -> Surface {
    let m = textureSample(t0, s0, uv);
    var s = surface_new();
    s.albedo = cu.v[0].rgb * m.r * (m.b * cu.v[0].w);
    s.alpha = clamp(cu.v[1].y * m.g * (1.0 - m.r) + spow(m.g * m.r, 2.0 * (1.0 - cu.v[1].x)), 0.0, 1.0);
    s.roughness = 1.0;
    s.specular = 0.0;
    return s;
}

// ---- Lighting: the game's deferred light loop for character surfaces ----
// bg3: ClusteredLighting_N kernels, DoClusteredLight / DoClusteredLight_Subsurface,
// Specular_Hair, CalculateHairParameters (Core.metallib, 4.1.1.7631656). Our own port of the
// BRDFs, run over Bevy's lights with the game's falloff (`game_light.wesl`); shadows and
// ambient stay Bevy's.
// Shading model in `cu.flags`: 0 lit, 3 subsurface, 5 hair. Profile block (model 3):
// v[20] (scatter colour, scatter distance mm), v[21] (transmission tint / 4, kernel index),
// v[22] (thickness remap min, max − min, thick object, transmission), v[23] (dual specular
// roughness A, B, mix).
const MODEL_SSS: u32 = 3u;
// `cu.flags` bits above the shading model.
const FLAG_SSS_MARK: u32 = 8u;
const FLAG_BEVY_LIGHTING: u32 = 256u;
const MODEL_HAIR: u32 = 5u;
const PI: f32 = 3.14159265;

@if(!PREPASS_PIPELINE)
fn pow5(x: f32) -> f32 {
    let y = max(abs(x), 1e-4);
    let y2 = y * y;
    return y2 * y2 * y;
}

// Renormalised Disney diffuse (Frostbite form) with perceptual roughness `r`.
@if(!PREPASS_PIPELINE)
fn burley(r: f32, n_l: f32, n_v: f32, l_h: f32) -> f32 {
    let bias = 0.5 * r;
    let factor = mix(1.0, 1.0 / 1.51, r);
    let fd90 = bias + 2.0 * l_h * l_h * r;
    return (1.0 + (fd90 - 1.0) * pow5(1.0 - n_l)) * (1.0 + (fd90 - 1.0) * pow5(1.0 - n_v)) * factor / PI;
}

@if(!PREPASS_PIPELINE)
fn d_ggx(a: f32, n_h: f32) -> f32 {
    let a2 = a * a;
    let d = (n_h * a2 - n_h) * n_h + 1.0;
    return a2 / (PI * d * d);
}

// Height-correlated Smith with the remapped k² = ((r + 1)/2)².
@if(!PREPASS_PIPELINE)
fn vis_smith(r: f32, n_l: f32, n_v: f32) -> f32 {
    let k = (r + 1.0) * 0.5;
    let k2 = k * k;
    return 0.5 / max(n_l * sqrt(n_v * n_v * (1.0 - k2) + k2) + n_v * sqrt(n_l * n_l * (1.0 - k2) + k2), 1e-5);
}

struct CharLight {
    l: vec3<f32>,
    // Light colour × falloff (no N·L, no shadow).
    radiance: vec3<f32>,
    shadow: f32,
    // A shadow map is bound (Bevy).
    has_shadow: bool,
    // The game's light casts shadows (the viewer marks these with a negative normal bias when
    // it renders no shadow map for them).
    game_shadow: bool,
    sun: bool,
}

struct CharShading {
    model: u32,
    n: vec3<f32>,
    v: vec3<f32>,
    albedo: vec3<f32>,
    diffuse: vec3<f32>,
    f0: vec3<f32>,
    metallic: f32,
    // Perceptual roughness before (r0) and after (r) the specular anti-aliasing.
    r0: f32,
    r: f32,
    sss_amount: f32,
    thickness: f32,
    tangent: vec3<f32>,
    scatter: f32,
    backlit: f32,
}

// Lit (0) and subsurface (3) surfaces: Burley diffuse, GGX (dual-lobe for 3), transmission.
@if(!PREPASS_PIPELINE)
fn shade_surface(sh: CharShading, li: CharLight) -> vec3<f32> {
    let n_l_raw = dot(sh.n, li.l);
    let n_l = clamp(n_l_raw, 0.0, 1.0);
    let n_v = max(dot(sh.n, sh.v), 1e-5);
    var out = vec3(0.0);
    if n_l > 0.0 {
        let h = normalize(sh.v + li.l);
        let l_h = clamp(dot(li.l, h), 0.0, 1.0);
        let diffuse = sh.diffuse * burley(sh.r, n_l, n_v, l_h);
        // The sun's specular uses the closest point of its disc (~0.26°) to the reflection.
        var ls = li.l;
        if li.sun {
            let refl = reflect(-sh.v, sh.n);
            let c = dot(li.l, refl);
            if c < 0.99999 {
                ls = normalize(0.99999 * li.l + 0.00459006 * normalize(refl - c * li.l));
            } else {
                ls = refl;
            }
        }
        let hs = normalize(sh.v + ls);
        let n_h = clamp(dot(sh.n, hs), 0.0, 1.0);
        let l_hs = clamp(dot(ls, hs), 0.0, 1.0);
        let n_ls = clamp(dot(sh.n, ls), 0.0, 1.0);
        let f = sh.f0 + (1.0 - sh.f0) * pow5(1.0 - l_hs);
        var d: f32;
        if sh.model == MODEL_SSS {
            let ds = cu.v[23];
            let ra = max(0.09, clamp(sh.r0 * ds.x, 0.0, 1.0));
            let rb = max(0.09, clamp(sh.r0 * ds.y, 0.0, 1.0));
            let aa = mix(ra * ra, sh.r * sh.r, sh.metallic);
            let ab = mix(rb * rb, sh.r * sh.r, sh.metallic);
            d = mix(d_ggx(aa, n_h), d_ggx(ab, n_h), ds.z);
        } else {
            d = d_ggx(sh.r * sh.r, n_h);
        }
        let spec = f * d * vis_smith(sh.r, n_ls, n_v);
        out = (diffuse + spec) * n_l * li.shadow;
    }
    // Back-lit transmission through skin (thin objects take the light's shadow).
    if sh.model == MODEL_SSS && cu.v[22].w > 0.5 {
        let p = cu.v;
        let w = 0.258819;
        let wt = clamp((w - n_l_raw) / ((1.0 + w) * (1.0 + w)), 0.0, 1.0);
        if wt > 0.0 {
            let t = p[22].x + p[22].y * sh.thickness;
            let sc = 1.0 / max(0.0966094 * p[20].rgb * p[20].w * sh.sss_amount, vec3(1e-4));
            let e = exp(-t * sc / 3.0);
            var tr = p[21].rgb * e * (e * e + 3.0);
            // Thin objects take the light's shadow. Thick ones take, instead, the larger of the
            // remapped thickness and the depth through the occluder in the light's shadow map
            // (0 for lights without shadows). Approximations: with a Bevy shadow map we gate by
            // the shadow instead of measuring that depth; for a game-shadowed light without one
            // we take the occluder as much thicker than the scatter length (no transmission).
            if p[22].z < 0.5 || li.has_shadow {
                tr *= li.shadow;
            } else if li.game_shadow {
                tr = vec3(0.0);
            }
            out += tr * sh.diffuse * wt / PI;
        }
    }
    return out * li.radiance;
}

// Hair (5): Marschner R/TT/TRT lobes and the Kajiya-Kay scatter term.
@if(!PREPASS_PIPELINE)
fn hair_g(b: f32, x: f32) -> f32 {
    return exp(-0.5 * x * x / (b * b)) / (b * 2.5066283);
}

@if(!PREPASS_PIPELINE)
fn hair_fresnel(c: f32) -> f32 {
    return 0.0465206 + 0.9534794 * pow5(1.0 - c);
}

@if(!PREPASS_PIPELINE)
fn shade_hair(sh: CharShading, li: CharLight) -> vec3<f32> {
    let t = sh.tangent;
    let l = li.l;
    let v = sh.v;
    let a = sh.albedo;
    let r = max(0.09, sh.r0);
    let r2 = r * r;
    let sin_l = clamp(dot(t, l), -1.0, 1.0);
    let sin_v = clamp(dot(t, v), -1.0, 1.0);
    let cos_d = cos(0.5 * abs(asin(sin_v) - asin(sin_l)));
    let lp = l - sin_l * t;
    let vp = v - sin_v * t;
    let cos_phi = dot(lp, vp) / sqrt(dot(lp, lp) * dot(vp, vp) + 1e-4);
    let cos_half = sqrt(clamp(0.5 + 0.5 * cos_phi, 0.0, 1.0));
    let v_l = dot(v, l);

    // R
    let sa = sin(-0.07);
    let ca = cos(-0.07);
    let shift_r = 2.0 * sa * (ca * cos_half * sqrt(max(1.0 - sin_v * sin_v, 0.0)) + sa * sin_v);
    let lobe_r = hair_g(1.4142135 * r2 * cos_half, sin_l + sin_v - shift_r) * 0.25 * cos_half
        * hair_fresnel(sqrt(clamp(0.5 + 0.5 * v_l, 0.0, 1.0)))
        * mix(1.0, sh.backlit, clamp(-v_l, 0.0, 1.0));
    // TT
    let n_p = 1.19 / cos_d + 0.36 * cos_d;
    let ia = 1.0 / n_p;
    let h = cos_half * (1.0 + ia * (0.6 - 0.8 * cos_phi));
    let f_tt = hair_fresnel(cos_d * sqrt(clamp(1.0 - h * h, 0.0, 1.0)));
    let t_tt = spow3(a, vec3(0.5 * sqrt(max(1.0 - h * h * ia * ia, 0.0)) / cos_d));
    let lobe_tt = hair_g(0.5 * r2, sin_l + sin_v - 0.035) * exp(-3.65 * cos_phi - 3.98)
        * (1.0 - f_tt) * (1.0 - f_tt) * t_tt * sh.backlit;
    // TRT
    let f_trt = hair_fresnel(0.5 * cos_d);
    let t_trt = spow3(a, vec3(0.8 / cos_d));
    let lobe_trt = hair_g(2.0 * r2, sin_l + sin_v - 0.14) * exp(17.0 * cos_phi - 16.78)
        * (1.0 - f_trt) * (1.0 - f_trt) * f_trt * t_trt;
    let spec = lobe_r + lobe_tt + lobe_trt;

    // Visibility: the light's shadow, or a soft terminator for lights without one.
    var sigma = li.shadow;
    if !li.has_shadow {
        sigma = clamp(2.0 * dot(sh.n, l) + 0.5, 0.0, 1.0);
    }
    let kk = 1.0 - abs(dot(t, l));
    let wrap = clamp((dot(sh.n, l) + 1.0) * 0.25, 0.0, 1.0);
    let luma = max(dot(a, vec3(0.3, 0.59, 0.11)), 1e-4);
    var e = 1.0 - sigma;
    if li.sun {
        e = 1.0 - sigma * sigma;
    }
    let tint = spow3(a / luma, vec3(e));
    let diff = sqrt(a) * (mix(wrap, kk, 0.33) * sh.scatter / PI) * tint;
    return (spec + diff) * sigma * li.radiance;
}

@if(!PREPASS_PIPELINE)
fn shade_light(sh: CharShading, li: CharLight) -> vec3<f32> {
    if sh.model == MODEL_HAIR {
        return shade_hair(sh, li);
    }
    return shade_surface(sh, li);
}

// Direct light from Bevy's clustered point/spot lights and directional lights, ambient, and
// emissive, with the game's BRDFs. Replaces `apply_pbr_lighting` for character surfaces.
@if(!PREPASS_PIPELINE)
fn char_lighting(in: pbr_types::PbrInput, sh: CharShading) -> vec3<f32> {
    let p = in.world_position;
    let receiver = (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u;
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), p);
    let cluster_index = clustering::view_fragment_cluster_index(in.frag_coord.xy, view_z, in.is_orthographic);
    var ranges = clustering::unpack_clusterable_object_index_ranges(cluster_index);
    // Shadow lookups offset along the side the light is on (hair: along the light).
    var direct = vec3(0.0);

    for (var i = ranges.first_point_light_index_offset; i < ranges.first_rect_light_index_offset; i += 1u) {
        let id = clustering::get_clusterable_object_id(i);
        let light = &view_bindings::clustered_lights.data[id];
        let to_light = (*light).position_radius.xyz - p.xyz;
        let d2 = dot(to_light, to_light);
        var li: CharLight;
        li.l = normalize(to_light);
        li.sun = false;
        var falloff = bg3_light_falloff(d2, (*light).color_inverse_square_range.w, (*light).shadow_depth_bias);
        let spot = i >= ranges.first_spot_light_index_offset;
        if spot {
            var spot_dir = vec3((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);
            spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
            if ((*light).flags & mesh_view_types::POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u {
                spot_dir.y = -spot_dir.y;
            }
            let cd = dot(-spot_dir, li.l);
            let att = clamp(cd * (*light).light_custom_data.z + (*light).light_custom_data.w, 0.0, 1.0);
            falloff *= att * att;
        }
        li.radiance = (*light).color_inverse_square_range.rgb * falloff;
        li.game_shadow = (*light).shadow_normal_bias < 0.0;
        li.has_shadow = receiver && ((*light).flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u;
        li.shadow = 1.0;
        if li.has_shadow {
            var bias_n = select(-in.world_normal, in.world_normal, dot(in.world_normal, li.l) >= 0.0);
            if sh.model == MODEL_HAIR {
                bias_n = li.l;
            }
            if spot {
                li.shadow = shadows::fetch_spot_shadow(id, p, bias_n, (*light).shadow_map_near_z, in.frag_coord.xy);
            } else {
                li.shadow = shadows::fetch_point_shadow(id, p, bias_n, in.frag_coord.xy);
            }
        }
        direct += shade_light(sh, li);
    }

    let n_dir = view_bindings::lights.n_directional_lights;
    for (var i = 0u; i < n_dir; i += 1u) {
        let light = &view_bindings::lights.directional_lights[i];
        var li: CharLight;
        li.l = (*light).direction_to_light.xyz;
        li.sun = true;
        li.radiance = (*light).color.rgb;
        li.game_shadow = (*light).shadow_normal_bias < 0.0;
        li.has_shadow = receiver && ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u;
        li.shadow = 1.0;
        if li.has_shadow {
            var bias_n = select(-in.world_normal, in.world_normal, dot(in.world_normal, li.l) >= 0.0);
            if sh.model == MODEL_HAIR {
                bias_n = li.l;
            }
            li.shadow = shadows::fetch_directional_shadow(i, p, bias_n, view_z, in.frag_coord.xy) * in.directional_shadow_factor;
        }
        direct += shade_light(sh, li);
    }

    // Ambient: Bevy's uniform ambient (zero for levels with probes) and the probes below, through
    // the split-sum approximation as for every other surface here. Hair uses |N·V|.
    let n_v = select(max(dot(sh.n, sh.v), 1e-4), max(abs(dot(sh.n, sh.v)), 1e-4), sh.model == MODEL_HAIR);
    var amb_diffuse = sh.diffuse;
    if sh.model == MODEL_HAIR {
        amb_diffuse = sh.albedo;
    }
    var indirect = ambient::ambient_light(p, sh.n, sh.v, n_v, amb_diffuse, sh.f0, sh.r, in.diffuse_occlusion);
    // The level's light probes (Bevy reflection probes, see `probes.rs`): the game lights
    // characters with the same probe loop as everything else. Hair evaluates them with N and −N
    // and blends 0.3 / 0.7.
@if(ENVIRONMENT_MAP) {
    var env_in: LightingInput;
    env_in.layers[LAYER_BASE].NdotV = n_v;
    env_in.layers[LAYER_BASE].N = sh.n;
    env_in.layers[LAYER_BASE].R = reflect(-sh.v, sh.n);
    env_in.layers[LAYER_BASE].perceptual_roughness = sh.r;
    env_in.layers[LAYER_BASE].roughness = sh.r * sh.r;
    env_in.P = p.xyz;
    env_in.V = sh.v;
    env_in.diffuse_color = amb_diffuse;
    env_in.metallic = 0.0;
    env_in.F0_dielectric = sh.f0;
    env_in.F0_metallic = sh.f0;
    env_in.F_ab = F_AB(sh.r, n_v);
    var env = environment_map::environment_map_light(&env_in, &ranges, false);
    if sh.model == MODEL_HAIR {
        env_in.layers[LAYER_BASE].N = -sh.n;
        env_in.layers[LAYER_BASE].R = reflect(-sh.v, -sh.n);
        let back = environment_map::environment_map_light(&env_in, &ranges, false);
        env.diffuse = mix(env.diffuse, back.diffuse, 0.7);
        env.specular = mix(env.specular, back.specular, 0.7);
    }
    indirect += env.diffuse * in.diffuse_occlusion + env.specular * in.specular_occlusion;
}

    let emissive = in.material.emissive.rgb * mix(1.0, view_bindings::view.exposure, in.material.emissive.a);
    return view_bindings::view.exposure * (direct + indirect) + emissive;
}

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;
    let mode = cu.mode;
    var surf = surface_new();
    var vc = vec4(0.0, 0.0, 0.0, 1.0);
@if(VERTEX_COLORS)
    vc = in.color;
    let vc_r = vc.r;

@if(VERTEX_UVS_A) {
    if mode == MODE_EYE {
        let view = normalize(bevy_pbr::render::pbr_functions::calculate_view(in.world_position, false));
        in.uv = eye_parallax_uv(in, in.uv, view);
    }
}

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Geometric specular anti-aliasing input: the normal's screen-space change.
    let dn_x = dpdx(pbr_input.N);
    let dn_y = dpdy(pbr_input.N);
    let normal_variance = min(dot(dn_x, dn_x) + dot(dn_y, dn_y), 0.18);

@if(VERTEX_UVS_A) {
    let uv = in.uv;
    if mode == MODE_SKIN {
        surf = skin(uv);
    } else if mode == MODE_EYE {
        surf = eye(uv, vc_r);
@if(VERTEX_TANGENTS) {
        // Lighting normal: the iris normal map flattened toward the mesh normal by blindness,
        // plus the sclera detail normal; the catchlight reflects about the mesh normal plus the
        // detail normal only.
        let t = normalize(in.world_tangent.xyz);
        let b = cross(pbr_input.world_normal, t) * sign(in.world_tangent.w);
        let ns = eye_sclera_offset(uv);
        let detail = (ns.x * t + ns.y * b) * cu.v[11].w;
        let blind = mix(cu.v[9].z, cu.v[9].w, vc_r) * textureSample(t2, s0, uv).g;
        let ni = mix(pbr_input.N, pbr_input.world_normal, blind);
        pbr_input.N = normalize(ni + cu.v[11].z * detail);
        surf.emissive = eye_catchlight(in, normalize(pbr_input.world_normal + detail), pbr_input.V);
} @else {
        surf.emissive = eye_catchlight(in, pbr_input.world_normal, pbr_input.V);
}
@if(!PREPASS_PIPELINE) {
        if cu.v[11].x > 0.5 {
            surf.emissive += eye_glow(uv, vc_r, view_bindings::globals.time);
        }
}
    } else if mode == MODE_LASHES {
        surf = lashes(uv);
    } else if mode == MODE_HAIR {
        surf = hair(uv, vc, dot(pbr_input.world_normal, pbr_input.V));
    }
    if mode == MODE_SKIN || mode == MODE_EYE || mode == MODE_LASHES || mode == MODE_HAIR {
        pbr_input.material.base_color = vec4(surf.albedo, surf.alpha * pbr_input.material.base_color.a);
        pbr_input.material.perceptual_roughness = surf.roughness;
        pbr_input.material.metallic = surf.metallic;
        // The game's F0 is 0.16·reflectance², as Bevy's.
        pbr_input.material.reflectance = vec3(surf.specular);
        pbr_input.diffuse_occlusion *= surf.occlusion;
        // Emissive (eye catchlight and glow) is added unexposed, as the game's emissive pass.
        pbr_input.material.emissive = vec4(surf.emissive, 0.0);
    } else if mode == MODE_CLOTH_PALETTE {
        let base = pbr_input.material.base_color;
        pbr_input.material.base_color = vec4(base.rgb * palette_tint(uv), base.a);
    } else if mode == MODE_CLOTH_MSK {
        let base = pbr_input.material.base_color;
        pbr_input.material.base_color = vec4(msk_tint(base.rgb, uv), base.a);
    }
}

    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    );

@if(PREPASS_PIPELINE) {
    let out = deferred_output(in, pbr_input);
    return out;
}
@if(!PREPASS_PIPELINE) {
    var out: FragmentOutput;
    let model = cu.flags & 7u;
    if (cu.flags & FLAG_BEVY_LIGHTING) != 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        var sh: CharShading;
        sh.model = model;
        sh.n = pbr_input.N;
        sh.v = pbr_input.V;
        let base = pbr_input.material.base_color.rgb;
        sh.albedo = base;
        sh.metallic = select(pbr_input.material.metallic, 0.0, model == MODEL_HAIR);
        sh.diffuse = base * (1.0 - sh.metallic);
        let refl = pbr_input.material.reflectance;
        sh.f0 = mix(0.16 * refl * refl, base, sh.metallic);
        sh.r0 = max(0.09, pbr_input.material.perceptual_roughness);
        sh.r = sqrt(clamp(sh.r0 * sh.r0 + normal_variance, 0.0, 1.0));
        sh.sss_amount = surf.sss_amount;
        sh.thickness = surf.thickness;
        sh.scatter = surf.scatter;
        sh.backlit = surf.backlit;
        sh.tangent = hair_tangent(in, pbr_input.N, surf.tangent);
        out.color = vec4(char_lighting(pbr_input, sh), pbr_input.material.base_color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    // Mark subsurface pixels for the screen-space scattering pass (`sss.wesl`) in the HDR
    // target's alpha: −(1 + kernel index + amount/2). Blended surfaces keep their alpha.
    if model == MODEL_SSS && (cu.flags & FLAG_SSS_MARK) != 0u {
        out.color.a = -(1.0 + cu.v[21].w + 0.5 * clamp(surf.sss_amount, 0.0, 1.0));
    }
    return out;
}
}

// World strand tangent from the hair's tangent-space vector (x tangent, y normal, z binormal).
@if(!PREPASS_PIPELINE)
fn hair_tangent(in: VertexOutput, n: vec3<f32>, ts: vec3<f32>) -> vec3<f32> {
@if(VERTEX_TANGENTS) {
    let t = normalize(in.world_tangent.xyz);
    let b = cross(n, t) * sign(in.world_tangent.w);
    return normalize(ts.x * t + ts.y * n + ts.z * b);
}
    // No tangents: strands run along the surface, toward world up.
    let up = vec3(0.0, 1.0, 0.0);
    return normalize(up - n * dot(up, n) + vec3(1e-4, 0.0, 0.0));
}
