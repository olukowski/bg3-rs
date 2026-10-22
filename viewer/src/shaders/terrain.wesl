// Terrain: the game's terrain pixel shader (layer height blend, base layers, triplanar-style
// projection, normal/physical/detail maps, height gradient) producing the G-buffer inputs, lit
// with the game's light loop (`game_lighting.wesl`) like level meshes. Written from our
// description in docs/research/terrain.md ("Drawing"), not translated from the game's shaders;
// `bg3_world::terrain_shading` prepares the inputs and models the blend on the CPU.
//
// bg3: Terrain_PS4 … Terrain_PS64, SampleLayerCore, GetHeights4 (Core.metallib, 4.1.1.7631656)

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_functions,
};
import bg3_viewer::shaders::game_lighting::{GameShading, game_lighting};

struct TerrainUniform {
    // 1/(cells+1) (vertex normal map), 1/cells (weight maps).
    texel_size: vec4<f32>,
    // Local height = normalised · x + y.
    scale_offset: vec2<f32>,
    base_layers: vec2<u32>,
    height_transition: f32,
    ramp_width: u32,
    layer_count: u32,
    flags: u32,
}

struct LayerParam {
    // Array slices: base colour, normal, physical, detail normal (−1 none).
    textures: vec4<f32>,
    gradient_bottom_color: vec4<f32>,
    gradient_mid_color: vec4<f32>,
    height_scale: f32,
    height_offset: f32,
    height_contrast: f32,
    alpha_threshold: f32,
    tiling: f32,
    detail_normal_tiling: f32,
    detail_normal_strength: f32,
    gradient_bottom_height: f32,
    gradient_mid_point: f32,
    gradient_top_height: f32,
    gradient_roughness_reach: f32,
    gradient_roughness_power: f32,
    flags: u32,
    material_type: u32,
    _pad0: u32,
    _pad1: u32,
}

struct LayerParams {
    v: array<LayerParam, 64>,
}

const FLAG_BEVY_LIGHTING: u32 = 256u;

const LAYER_DETAIL_NORMAL: u32 = 1u;
const LAYER_HEIGHT_GRADIENT: u32 = 2u;
const LAYER_BLACK: u32 = 4u;

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> tu: TerrainUniform;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> layers: LayerParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var base_color_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var layer_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var normal_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var physical_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var weight_maps: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var weight_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var normal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(110) var base_ramp: texture_2d<f32>;

// Layer texture coordinates (before tiling) with their screen derivatives, the projection's
// sign and whether it is the X-facing side projection.
struct Coords {
    uv: vec2<f32>,
    ddx: vec2<f32>,
    ddy: vec2<f32>,
    sign: f32,
    side_x: bool,
}

struct Accum {
    albedo: vec3<f32>,
    normal: vec3<f32>,
    physical: vec3<f32>,
    black: f32,
}

fn sample_layer(tex: texture_2d_array<f32>, c: Coords, scale: f32, slice: f32) -> vec4<f32> {
    let i = i32(max(slice, 0.0));
    return textureSampleGrad(tex, layer_sampler, c.uv * scale, i, c.ddx * scale, c.ddy * scale);
}

// A layer's height value (−1 = absent) and its base colour.
fn layer_height(c: Coords, i: u32, w: f32) -> vec4<f32> {
    let p = layers.v[i];
    var albedo = vec4(0.0, 0.0, 0.0, 0.5);
    if p.textures.x != -1.0 {
        albedo = sample_layer(base_color_array, c, p.tiling, p.textures.x);
    }
    var v = 0.0;
    if w + albedo.a - 1.0 > p.alpha_threshold {
        v = w * pow(max(abs(albedo.a), 1e-6), p.height_contrast);
    }
    v *= p.height_scale;
    if v > 0.01 {
        v += p.height_offset;
    }
    return vec4(albedo.rgb, select(-1.0, v, v > 0.01));
}

// Tangent-space normal from a normal map texel (X in alpha, Y in green, Z in blue).
fn decode_normal(t: vec4<f32>, c: Coords) -> vec3<f32> {
    var n = vec3(t.a * 2.0 * c.sign - c.sign, -t.g * 2.0 * c.sign + c.sign, t.b * 2.0 - 1.0);
    if c.side_x {
        n = vec3(-n.y, n.x, n.z);
    }
    return n;
}

fn smooth01(x: f32) -> f32 {
    return x * x * (3.0 - 2.0 * x);
}

// bg3: SampleLayerCore (Core.metallib, 4.1.1.7631656)
fn sample_layer_core(c: Coords, i: u32, f: f32, world_y: f32, albedo_in: vec3<f32>, acc: ptr<function, Accum>) {
    let p = layers.v[i];
    if (p.flags & LAYER_BLACK) != 0u {
        (*acc).black += f;
        return;
    }
    var physical = sample_layer(physical_array, c, p.tiling, p.textures.z).rgb;
    var n = decode_normal(sample_layer(normal_array, c, p.tiling, p.textures.y), c);
    if (p.flags & LAYER_DETAIL_NORMAL) != 0u {
        let dt = max(p.detail_normal_tiling, 0.01);
        let d = decode_normal(sample_layer(normal_array, c, dt, p.textures.w), c);
        // Reoriented normal mapping with the detail's XY scaled by −strength.
        let n1 = vec3(n.xy, n.z + 1.0);
        let n2 = vec3(-p.detail_normal_strength * d.xy, d.z);
        n = n1 * dot(n1, n2) - n2 * n1.z;
    }
    var albedo = albedo_in;
    if (p.flags & LAYER_HEIGHT_GRADIENT) != 0u {
        let t = clamp((world_y - p.gradient_bottom_height) / (p.gradient_top_height - p.gradient_bottom_height), 0.0, 1.0);
        let s = smooth01(t);
        let a = sqrt(smooth01(clamp(s / p.gradient_mid_point, 0.0, 1.0)));
        let b = sqrt(smooth01(clamp((s - p.gradient_mid_point) / (1.0 - p.gradient_mid_point), 0.0, 1.0)));
        let col = mix(mix(p.gradient_bottom_color.rgb, p.gradient_mid_color.rgb, vec3(a)), vec3(1.0), vec3(b));
        let r = clamp(pow(max(abs(p.gradient_roughness_reach + col.x), 1e-4), p.gradient_roughness_power), 0.0, 1.0);
        albedo *= col;
        physical.y *= r;
    }
    (*acc).albedo += albedo * f;
    (*acc).physical += physical * f;
    (*acc).normal += normalize(n) * f;
}

// The ramp row `row` at u, as the shader's gather + lerp with a clamping sampler.
fn ramp_at(row: i32, u: f32) -> f32 {
    let w = f32(tu.ramp_width);
    let x = u * w - 0.5;
    let i0 = i32(floor(x));
    let f = clamp(fract(x), 0.0, 1.0);
    let last = i32(tu.ramp_width) - 1;
    let a = textureLoad(base_ramp, vec2(clamp(i0, 0, last), row), 0).r;
    let b = textureLoad(base_ramp, vec2(clamp(i0 + 1, 0, last), row), 0).r;
    return mix(a, b, f);
}

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // Terrain-local position in the game's space: UV 0 = (x, z), UV 1.x = local height.
    let xz = in.uv;
    let height = in.uv_b.x;
    var hn = 0.0;
    if tu.scale_offset.x > 0.0 {
        hn = (height - tu.scale_offset.y) / tu.scale_offset.x;
    }
    let world_y = in.world_position.y;

    let nt = normalize(textureSample(normal_map, normal_sampler, (xz + 0.5) * tu.texel_size.xy).xyz * 2.0 - 1.0);

    // Projection: top-down, or one side projection where the slope is steep enough.
    var b = max(abs(nt) - 0.576, vec3(0.0));
    b /= b.x + b.y + b.z;
    if max(max(b.x, b.y), b.z) != 1.0 {
        var c = vec3(b.x, 0.01, b.z) / (b.x + 0.01 + b.z);
        let m = max(max(c.x, c.y), c.z);
        c = clamp(c + select(vec3(-m), vec3(0.0), c == vec3(m)), vec3(0.0), vec3(1.0));
        b = c / (c.x + c.y + c.z);
    }
    let s = select(-b, b, nt >= vec3(0.0));
    let h = hn * tu.scale_offset.x + tu.scale_offset.y;
    var co: Coords;
    co.uv = vec2(xz.y * s.x + xz.x * s.y - s.z * xz.x, -xz.y * b.y - (b.x + b.z) * h);
    co.ddx = dpdx(co.uv);
    co.ddy = dpdy(co.uv);
    co.sign = s.x + s.y - s.z;
    co.side_x = b.x > 0.0;

    // Height values of every layer; the four highest take part (fewer layers: all of them).
    let wuv = xz * tu.texel_size.zw;
    var top_i = array<u32, 4>(0u, 0u, 0u, 0u);
    var top_h = array<f32, 4>(-2.0, -2.0, -2.0, -2.0);
    var top_w = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var top_c = array<vec3<f32>, 4>(vec3(0.0), vec3(0.0), vec3(0.0), vec3(0.0));
    var weights = vec4(0.0);
    for (var i = 0u; i < tu.layer_count; i += 1u) {
        if (i & 3u) == 0u {
            weights = textureSampleLevel(weight_maps, weight_sampler, wuv, i32(i / 4u), 0.0);
        }
        let w = weights[i & 3u];
        var hv = vec4(0.0, 0.0, 0.0, -1.0);
        if w > 1e-3 {
            hv = layer_height(co, i, w);
        }
        // Insert into the top four (kept sorted, highest first).
        if hv.w > top_h[3] {
            var k = 3;
            while k > 0 && hv.w > top_h[k - 1] {
                top_h[k] = top_h[k - 1];
                top_i[k] = top_i[k - 1];
                top_w[k] = top_w[k - 1];
                top_c[k] = top_c[k - 1];
                k -= 1;
            }
            top_h[k] = hv.w;
            top_i[k] = i;
            top_w[k] = w;
            top_c[k] = hv.rgb;
        }
    }

    // The blend: relative to the highest layer (or the base) within the transition width.
    let hh = max(vec4(top_h[0], top_h[1], top_h[2], top_h[3]), vec4(0.0));
    let base = clamp(1.0 - (hh.x + hh.y + hh.z + hh.w), 0.0, 1.0);
    let top = max(max(max(hh.x, hh.y), max(hh.z, hh.w)), base);
    let tr = tu.height_transition;
    let g = vec4(top_w[0], top_w[1], top_w[2], top_w[3]) * (max(vec4(0.0), hh - top + tr) + 1e-6);
    let total = g.x + g.y + g.z + g.w + base * tr;
    let r = g / total;
    let rest = clamp(1.0 - (r.x + r.y + r.z + r.w), 0.0, 1.0);

    var acc: Accum;
    acc.albedo = vec3(0.0);
    acc.normal = vec3(0.0);
    acc.physical = vec3(0.0);
    acc.black = 0.0;
    for (var k = 0; k < 4; k += 1) {
        if r[k] > 1e-3 {
            sample_layer_core(co, top_i[k], r[k], world_y, top_c[k], &acc);
        }
    }

    // Base layers share the rest by their slope and height curves.
    if tu.base_layers.x > 63u {
        // No base layer: a grey checker (2 m squares).
        let q = vec2<u32>(xz * 0.5) & vec2(1u);
        let checker = select(0.2, 0.1, q.x == q.y);
        acc.albedo += rest * vec3(checker);
        acc.normal += rest * vec3(0.0, 0.0, 1.0);
        acc.physical += rest * vec3(0.0, 1.0, 1.0);
    } else {
        let slope_u = clamp(nt.y, 0.0, 1.0);
        var a = ramp_at(0, slope_u) * ramp_at(2, hn);
        let b2 = 2.0 * ramp_at(1, slope_u) * ramp_at(3, hn);
        if a + b2 == 0.0 {
            a = 1.0;
        }
        let f = vec2(a, b2) * (rest / (a + b2));
        for (var k = 0; k < 2; k += 1) {
            if f[k] > 1e-3 {
                let li = tu.base_layers[k];
                let p = layers.v[li];
                let albedo = sample_layer(base_color_array, co, p.tiling, p.textures.x).rgb;
                sample_layer_core(co, li, f[k], world_y, albedo, &acc);
            }
        }
    }

    // Tangent frame from the terrain normal: T = (n.y, −n.x, 0), B = T × N.
    var tn = acc.normal;
    let d = dot(tn, tn);
    if d > 1e-3 {
        tn *= inverseSqrt(d);
    }
    var t = vec3(nt.y, -nt.x, 0.0);
    t = select(vec3(1.0, 0.0, 0.0), normalize(t), dot(t, t) > 1e-8);
    let bt = normalize(cross(t, nt));
    let n_game = tn.x * t + tn.y * bt + tn.z * nt;
    // Into Bevy's (mirrored) local space, then world space.
    let n_world = normalize(mesh_functions::mesh_normal_local_to_world(vec3(n_game.x, n_game.y, -n_game.z), in.instance_index));

    let black = acc.black;
    let albedo = clamp(acc.albedo, vec3(0.0), vec3(1.0));
    let occlusion = clamp(acc.physical.z + black, 0.0, 1.0);
    let roughness = clamp(acc.physical.y + black * (1.0 - 2.0 * clamp(black - 0.5, 0.0, 1.0)), 0.0, 1.0);
    let metallic = clamp(acc.physical.x + select(black, 0.0, black < 0.5), 0.0, 1.0);
    // Shading model 1 (unlit) where black brushes cover the pixel.
    let unlit = black >= 0.975;

    pbr_input.N = n_world;
    pbr_input.material.base_color = vec4(albedo, 1.0);
    pbr_input.material.perceptual_roughness = roughness;
    pbr_input.material.metallic = metallic;
    pbr_input.material.reflectance = vec3(0.5);
    pbr_input.diffuse_occlusion = vec3(occlusion);

    var out: FragmentOutput;
    if unlit {
        out.color = vec4(albedo, 1.0);
    } else if (tu.flags & FLAG_BEVY_LIGHTING) != 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        // Geometric specular anti-aliasing input: the normal's screen-space change.
        let dn_x = dpdx(n_world);
        let dn_y = dpdy(n_world);
        let normal_variance = min(dot(dn_x, dn_x) + dot(dn_y, dn_y), 0.18);
        var sh: GameShading;
        sh.model = 0u;
        sh.n = n_world;
        sh.v = pbr_input.V;
        sh.albedo = albedo;
        sh.metallic = metallic;
        sh.diffuse = albedo * (1.0 - metallic);
        sh.f0 = mix(vec3(0.16 * 0.25), albedo, metallic);
        sh.r0 = max(0.09, roughness);
        sh.r = sqrt(clamp(sh.r0 * sh.r0 + normal_variance, 0.0, 1.0));
        sh.sss_amount = 1.0;
        sh.thickness = 0.0;
        sh.tangent = vec3(0.0, 1.0, 0.0);
        sh.scatter = 0.0;
        sh.backlit = 0.0;
        sh.profile = array<vec4<f32>, 4>(vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0));
        out.color = vec4(game_lighting(pbr_input, sh), 1.0);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
