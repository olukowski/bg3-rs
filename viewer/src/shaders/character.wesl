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
}

fn surface_new() -> Surface {
    return Surface(vec3(1.0), 1.0, 0.5, 0.0, 0.5, 1.0, vec3(0.0));
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
    return uv - 0.045 * (1.0 + h) * vec2(vt.x, vt.z);
}
    return uv;
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
    iris = mix(iris, mix(vec3(0.4375, 0.4974, 0.5222), vec3(0.2611, 0.3551, 0.3715), w), blind);
    iris = mix(iris, mix(vec3(0.2508, 0.5989, 0.8277), vec3(0.9323, 0.5542, 0.1836), w), ersatz);
    iris *= mix(parts.g, 1.0, blind);
    iris *= 1.0 - parts.b * mix(edge, 0.5, ersatz);

    let rr = clamp(spow(parts.r, mix(redness, 1.5, ersatz)), 0.0, 1.0);
    var sclera = mix(blood_c, sclera_c, rr);
    sclera = mix(sclera, mix(vec3(0.3587, 0.0414, 0.0342), vec3(0.4045, 0.4704, 0.3112), rr), ersatz);
    let smask = region.g * region.b;
    sclera *= smask;
    let t = mix(select(smask, region.g, creature), 1.0 - bl.g, blind);

    var s = surface_new();
    s.albedo = clamp(mix(iris, sclera, t), vec3(0.0), vec3(1.0));
    s.roughness = mix(0.9, 0.0, t);
    s.occlusion = region.r * region.r;
    return s;
}

// Catchlight: the reflected view direction in object space looks up a lat-long HDRI; the
// game adds a third of it as emissive.
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

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;
    let mode = cu.mode;
    var surf = surface_new();
    var vc_r = 0.0;
@if(VERTEX_COLORS)
    vc_r = in.color.r;

@if(VERTEX_UVS_A) {
    if mode == MODE_EYE {
        let view = normalize(bevy_pbr::render::pbr_functions::calculate_view(in.world_position, false));
        in.uv = eye_parallax_uv(in, in.uv, view);
    }
}

    var pbr_input = pbr_input_from_standard_material(in, is_front);

@if(VERTEX_UVS_A) {
    let uv = in.uv;
    if mode == MODE_SKIN {
        surf = skin(uv);
    } else if mode == MODE_EYE {
        surf = eye(uv, vc_r);
        surf.emissive = eye_catchlight(in, pbr_input.N, pbr_input.V);
    } else if mode == MODE_LASHES {
        surf = lashes(uv);
    }
    if mode == MODE_SKIN || mode == MODE_EYE || mode == MODE_LASHES {
        pbr_input.material.base_color = vec4(surf.albedo, surf.alpha * pbr_input.material.base_color.a);
        pbr_input.material.perceptual_roughness = surf.roughness;
        pbr_input.material.metallic = surf.metallic;
        pbr_input.material.reflectance = vec3(sqrt(surf.specular * 0.5));
        pbr_input.diffuse_occlusion *= surf.occlusion;
        pbr_input.material.emissive = vec4(surf.emissive, 1.0);
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
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
}
