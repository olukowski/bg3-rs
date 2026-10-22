// Level materials: the G-buffer of the game's `Base_*`/`ENV_*` graphs on top of Bevy's standard
// material inputs (base colour, normal map, physical map → roughness, metalness, occlusion), lit
// with the game's light loop (`game_lighting.wesl`) instead of Bevy's PBR. Adds what Bevy's
// material lacks: the graph's `Reflectance`, the shading model and subsurface inputs (3
// subsurface, 4 translucent), the `ENV_DetailMap*` micro detail layer and the glow of the `GM`
// graphs. Written from our description in docs/research/materials.md ("Level shading"), not
// translated from the game's shaders; `bg3_material::level` holds the parameter logic.
//
// bg3: Shared_Base_Base_VT_ST_DEF_fragmentMain, Shared_Base_Base_SSS_VT_ST_DEF_fragmentMain,
// Shared_Base_Base_Translucency_VT_ST_DEF_fragmentMain,
// Shared_Environment_ENV_DetailMap_VT_ST_DEF_fragmentMain and the Base_GM* / Base_Glass_Glow
// _ST_EMI_fragmentMain entry points (Materials.metallib, 4.1.1.7631656)

import bevy_pbr::render::{
    pbr_types,
    pbr_functions::{alpha_discard, calculate_tbn_mikktspace},
    pbr_fragment::pbr_input_from_standard_material,
    mesh_functions,
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
};
@if(!PREPASS_PIPELINE)
import bg3_viewer::shaders::game_lighting::{GameShading, game_lighting};
@if(!PREPASS_PIPELINE)
import bg3_viewer::shaders::decals::{DecalSurface, apply_decals};
@if(!PREPASS_PIPELINE)
import bg3_viewer::shaders::surfaces::apply_surfaces;

struct LevelUniform {
    // Shading model (bits 0–2), FLAG_* above.
    flags: u32,
    glow: u32,
    subsurface: u32,
    _pad: u32,
    // See `bg3_material::level::LevelShading::uniform`; v[6..10] the diffusion profile.
    v: array<vec4<f32>, 10>,
}

const FLAG_SSS_MARK: u32 = 8u;
const FLAG_DETAIL: u32 = 16u;
const FLAG_BEVY_LIGHTING: u32 = 256u;

const MODEL_SSS: u32 = 3u;

const GLOW_PLAIN: u32 = 1u;
const GLOW_PULSE_EXPOSED: u32 = 2u;
const GLOW_PULSE: u32 = 3u;
const GLOW_PULSE_COLOUR: u32 = 4u;
const GLOW_WIND: u32 = 5u;
const GLOW_BOUNCING: u32 = 6u;
const GLOW_GLASS: u32 = 7u;

// `MeshTag` bit of objects that receive decals (`viewer/src/decals.rs`).
const TAG_RECEIVES_DECALS: u32 = 1u;

const SSS_THICKNESS_MAP: u32 = 1u;
const SSS_RADIUS_MASK: u32 = 2u;
const SSS_PHYSICAL_ALPHA: u32 = 3u;

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> lu: LevelUniform;
// Micro detail map (R occlusion, G roughness), micro detail normal map (BC5 like every normal
// map here), glow map, subsurface input (thickness map, SSS mask or the physical map).
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var detail_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var detail_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var detail_normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var detail_normal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var glow_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var glow_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var sss_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var sss_sampler: sampler;

fn spow(x: f32, y: f32) -> f32 { return pow(max(abs(x), 1e-4), y); }

// The object's local position of a world-space point (Bevy's local space is the game's with
// z mirrored; y, the only axis the graphs read, is the same).
@if(!PREPASS_PIPELINE)
fn local_position(instance_index: u32, world: vec3<f32>) -> vec3<f32> {
    return (mesh_functions::get_local_from_world(instance_index) * vec4(world, 1.0)).xyz;
}

// Emissive of the glow graphs; `.a` = 1 when the graph pre-exposes it (multiplied by the
// camera exposure like lighting), 0 when it is in display units.
@if(!PREPASS_PIPELINE)
fn glow(uv: vec2<f32>, instance_index: u32, world: vec3<f32>, time: f32) -> vec4<f32> {
    let kind = lu.glow;
    if kind == 0u {
        return vec4(0.0);
    }
    let gm = textureSample(glow_map, glow_sampler, uv).rgb;
    let c = lu.v[2].rgb;
    let mult = lu.v[2].w;
    let use_gm = clamp(floor(lu.v[3].x), 0.0, 1.0);
    let sel = mix(gm.r * c, gm, use_gm);
    let speed = lu.v[3].y;
    let lo = lu.v[3].z;
    let hi = lu.v[3].w;
    let pulse = mix(lo, hi, 0.5 + 0.5 * cos(time * speed));
    if kind == GLOW_PLAIN {
        return vec4(sel * mult, 0.0);
    }
    if kind == GLOW_PULSE_EXPOSED {
        return vec4(sel * mult * pulse, 1.0);
    }
    if kind == GLOW_PULSE {
        return vec4(sel * mult * pulse, 0.0);
    }
    if kind == GLOW_PULSE_COLOUR {
        return vec4(gm * c * mult * pulse, 0.0);
    }
    if kind == GLOW_WIND {
        return vec4(sel * mix(lo, hi, 0.5 + 0.5 * sin(time * speed)), 0.0);
    }
    let local = local_position(instance_index, world);
    if kind == GLOW_BOUNCING {
        // A triangle wave over time, the object's distance from the world origin and its height;
        // the vertex colour's blue (part of the phase in the game) is not on our meshes (0).
        let origin = mesh_functions::get_world_from_local(instance_index)[3].xyz;
        let phase = time * speed + length(origin) + local.y / lu.v[4].x + 0.5;
        let u = abs(fract(phase) * 2.0 - 1.0);
        let s = u * u * (3.0 - 2.0 * u);
        return vec4(gm * c * mult * mix(lo, hi, s), 0.0);
    }
    if kind == GLOW_GLASS {
        let t = clamp((local.y - lu.v[4].z) / (lu.v[4].w - lu.v[4].z), 0.0, 1.0);
        let w = mix(1.0, t * t * (3.0 - 2.0 * t), clamp(lu.v[4].y, 0.0, 1.0));
        return vec4(c * gm * mult * w, 0.0);
    }
    return vec4(0.0);
}

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Geometric specular anti-aliasing input: the normal's screen-space change.
    let dn_x = dpdx(pbr_input.N);
    let dn_y = dpdy(pbr_input.N);
    let normal_variance = min(dot(dn_x, dn_x) + dot(dn_y, dn_y), 0.18);
    // Screen derivatives of the position, for the decals' texture gradients and flat normal.
    let dp_x = dpdx(in.world_position.xyz);
    let dp_y = dpdy(in.world_position.xyz);

    var sss_amount = 1.0;
    var thickness = 0.0;
@if(VERTEX_UVS_A && !PREPASS_PIPELINE) {
    let uv = in.uv;
    // ENV_DetailMap*: the micro maps tile `MicroDetailTiling` times per UV unit, scaled by the
    // object's scale (distance from its origin over the local position's length).
    if (lu.flags & FLAG_DETAIL) != 0u {
        let local = local_position(in.instance_index, in.world_position.xyz);
        let origin = mesh_functions::get_world_from_local(in.instance_index)[3].xyz;
        let scale = length(in.world_position.xyz - origin) / max(length(local), 1e-6);
        let duv = uv * scale * lu.v[0].y;
        let dm = textureSample(detail_map, detail_sampler, duv);
        pbr_input.diffuse_occlusion *= clamp(spow(dm.r, lu.v[0].w), 0.0, 1.0);
        pbr_input.material.perceptual_roughness = clamp(
            pbr_input.material.perceptual_roughness + lu.v[1].x * (1.0 - dm.g), 0.0, 1.0);
@if(VERTEX_TANGENTS) {
        // The detail normal's tangent-plane part (strength × its x and binormal components)
        // added to the main normal; formed after Bevy's normal mapping, which is the same for an
        // orthonormal tangent frame.
        let dn = textureSample(detail_normal_map, detail_normal_sampler, duv).rg * 2.0 - 1.0;
        let tbn = calculate_tbn_mikktspace(pbr_input.world_normal, in.world_tangent);
        var d = lu.v[0].z * (dn.x * tbn[0] + dn.y * tbn[1]);
        if (pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u && !is_front {
            d = -d;
        }
        pbr_input.N = normalize(pbr_input.N + d);
}
    }
    let sss = lu.subsurface;
    if sss == SSS_THICKNESS_MAP {
        thickness = clamp(textureSample(sss_map, sss_sampler, uv).r, 0.0, 1.0);
    } else if sss == SSS_RADIUS_MASK {
        sss_amount = clamp(lu.v[1].y * textureSample(sss_map, sss_sampler, uv).r, 0.0, 1.0);
    } else if sss == SSS_PHYSICAL_ALPHA {
        thickness = clamp(textureSample(sss_map, sss_sampler, uv).a, 0.0, 1.0);
    }
    pbr_input.material.emissive = glow(uv, in.instance_index, in.world_position.xyz, view_bindings::globals.time);
}
    // G-buffer target 2: sat(Reflectance) (F0 = 0.16·r², as Bevy's).
    pbr_input.material.reflectance = vec3(clamp(lu.v[0].x, 0.0, 1.0));

    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    );

@if(!PREPASS_PIPELINE) {
    // Decals blend into the G-buffer of decal receivers; blended (forward) surfaces get none.
    let alpha_mode = pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if (mesh_functions::get_tag(in.instance_index) & TAG_RECEIVES_DECALS) != 0u
        && alpha_mode != pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_BLEND {
        var ds: DecalSurface;
        ds.albedo = pbr_input.material.base_color.rgb;
        ds.normal = pbr_input.N;
        ds.roughness = pbr_input.material.perceptual_roughness;
        ds.reflectance = pbr_input.material.reflectance.x;
        ds.metallic = pbr_input.material.metallic;
        ds = apply_decals(ds, in.world_position.xyz, view_bindings::view.world_position, dp_x, dp_y);
        // Surfaces (blood, …) after the level decals (`surfaces.wesl`).
        ds = apply_surfaces(ds, in.world_position.xyz, view_bindings::view.world_position, dp_x, dp_y);
        pbr_input.material.base_color = vec4(ds.albedo, pbr_input.material.base_color.a);
        pbr_input.N = ds.normal;
        pbr_input.material.perceptual_roughness = ds.roughness;
        pbr_input.material.reflectance = vec3(ds.reflectance);
        pbr_input.material.metallic = ds.metallic;
    }
}

@if(PREPASS_PIPELINE) {
    let out = deferred_output(in, pbr_input);
    return out;
}
@if(!PREPASS_PIPELINE) {
    var out: FragmentOutput;
    let model = lu.flags & 7u;
    if (pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT) != 0u {
        out.color = pbr_input.material.base_color;
    } else if (lu.flags & FLAG_BEVY_LIGHTING) != 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        var sh: GameShading;
        sh.model = model;
        sh.n = pbr_input.N;
        sh.v = pbr_input.V;
        let base = pbr_input.material.base_color.rgb;
        sh.albedo = base;
        sh.metallic = pbr_input.material.metallic;
        sh.diffuse = base * (1.0 - sh.metallic);
        let refl = pbr_input.material.reflectance;
        sh.f0 = mix(0.16 * refl * refl, base, sh.metallic);
        sh.r0 = max(0.09, pbr_input.material.perceptual_roughness);
        sh.r = sqrt(clamp(sh.r0 * sh.r0 + normal_variance, 0.0, 1.0));
        sh.sss_amount = sss_amount;
        sh.thickness = thickness;
        sh.tangent = vec3(0.0, 1.0, 0.0);
        sh.scatter = 0.0;
        sh.backlit = 0.0;
        sh.profile = array<vec4<f32>, 4>(lu.v[6], lu.v[7], lu.v[8], lu.v[9]);
        out.color = vec4(game_lighting(pbr_input, sh), pbr_input.material.base_color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    // Mark subsurface pixels for the screen-space scattering pass (`sss.wesl`), as the
    // character shader does: −(1 + kernel index + amount/2).
    if model == MODEL_SSS && (lu.flags & FLAG_SSS_MARK) != 0u {
        out.color.a = -(1.0 + lu.v[7].w + 0.5 * clamp(sss_amount, 0.0, 1.0));
    }
    return out;
}
}
