// Surfaces applied to a level surface's material inputs before lighting, as the game's surface
// decals (`ecl::SurfaceDeferredRenderer`) blend into the G-buffer after the level decals. Per
// surface type near the pixel: the pixel moved by the material's noise warp, the type's
// coverage interpolated bilinearly between cell centres (two (type, amount) pairs per cell),
// faded where the receiver is off the AI grid's height, shaped into an edge, then base colour
// (mixed with the edge colour at low coverage), roughness, reflectance, metalness and the normal
// map blended by that alpha. Written from our description in docs/research/surfaces.md, not
// translated from the game's shader; the math runs in game (left-handed) space.
//
// bg3: Shared_Surface_VFX_Surface_AlphaBlend_Fluid_ST_DEF_fragmentMain (Materials.metallib,
// 4.1.1.7631656), ecl::SurfaceDeferredRenderer::UpdateCell / ::DrawSurface (4.1.1.7631656)

import bg3_viewer::shaders::decals::{DecalSurface, rotate_euler, mirror};

// [0] first cell column x, z, texture width, height; [1] enabled, fade distance;
// then 4 per surface type: (texture layer or −1, Tilling_WarpNoise, Intensity, Tilling),
// (Edge_Width, Edge_Range, MinOpacity, Normal_Intensity), (Color_Intensity, Reflectance,
// normal blending, opacity), (EdgeColor, 0).
@group(constants::MATERIAL_BIND_GROUP) @binding(140) var<storage, read> surface_params: array<vec4<f32>>;
// Per cell: R type, G second type, B amount, A second amount (the game's `_SurfaceMask`).
@group(constants::MATERIAL_BIND_GROUP) @binding(141) var surface_mask: texture_2d<f32>;
// Per cell: the AI grid height (m).
@group(constants::MATERIAL_BIND_GROUP) @binding(142) var surface_height: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(143) var surface_base: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(144) var surface_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(145) var surface_noise: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(146) var surface_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(147) var surface_physical: texture_2d_array<f32>;

const SURFACE_HALF_PI: f32 = 1.5707964;
const CELL: f32 = 0.5;
const HEADER: u32 = 2u;
// Surface types the shader looks at per pixel (distinct types around it).
const MAX_TYPES: u32 = 3u;

fn smooth01(x: f32) -> f32 {
    let t = clamp(x, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn smoothstep_lin(lo: f32, hi: f32, x: f32) -> f32 {
    return smooth01((x - lo) / (hi - lo));
}

// The cell texel at a texel coordinate, zero outside.
fn mask_at(t: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    if any(t < vec2(0)) || any(t >= size) {
        return vec4(0.0);
    }
    return textureLoad(surface_mask, t, 0);
}

fn height_at(t: vec2<i32>, size: vec2<i32>) -> f32 {
    let c = clamp(t, vec2(0), size - 1);
    return textureLoad(surface_height, c, 0).r;
}

// This type's amount in a cell: amount × (type matches), for both pairs.
fn amount(m: vec4<f32>, ty: f32) -> f32 {
    let types = floor(m.rg * 255.0 + 0.5);
    return clamp(m.b * f32(types.x == ty) + m.a * f32(types.y == ty), 0.0, 1.0);
}

struct Coverage {
    // Bilinear coverage of the type (0 when no cell around has it).
    c: f32,
    // Whether any of the four cells has the type.
    any: bool,
    // Bilinear AI height.
    h: f32,
}

// Coverage of type `ty` at game-space position `q` (x, z), bilinear between cell centres.
fn coverage(q: vec2<f32>, ty: f32) -> Coverage {
    let header = surface_params[0];
    let size = vec2<i32>(header.zw);
    // Texel space: texel i's centre at i + ½.
    let t = q / CELL - header.xy - 0.5;
    let t0 = vec2<i32>(floor(t));
    let f = fract(t);
    let m00 = mask_at(t0, size);
    let m10 = mask_at(t0 + vec2(1, 0), size);
    let m01 = mask_at(t0 + vec2(0, 1), size);
    let m11 = mask_at(t0 + vec2(1, 1), size);
    let a00 = amount(m00, ty);
    let a10 = amount(m10, ty);
    let a01 = amount(m01, ty);
    let a11 = amount(m11, ty);
    var out: Coverage;
    let types = floor(vec4(m00.r, m10.r, m01.r, m11.r) * 255.0 + 0.5);
    let types2 = floor(vec4(m00.g, m10.g, m01.g, m11.g) * 255.0 + 0.5);
    out.any = any(types == vec4(ty)) || any(types2 == vec4(ty));
    out.c = mix(mix(a00, a10, f.x), mix(a01, a11, f.x), f.y);
    let h00 = height_at(t0, size);
    let h10 = height_at(t0 + vec2(1, 0), size);
    let h01 = height_at(t0 + vec2(0, 1), size);
    let h11 = height_at(t0 + vec2(1, 1), size);
    out.h = mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
    return out;
}

// Blend the surfaces covering `world` (Bevy space) into `s`. `camera` is the view position;
// `dpx`/`dpy` are the screen derivatives of `world` (taken in uniform control flow).
fn apply_surfaces(
    s_in: DecalSurface,
    world: vec3<f32>,
    camera: vec3<f32>,
    dpx: vec3<f32>,
    dpy: vec3<f32>,
) -> DecalSurface {
    var s = s_in;
    let header = surface_params[0];
    if surface_params[1].x == 0.0 {
        return s;
    }
    let p = mirror(world);
    let size = vec2<i32>(header.zw);
    // Candidate types: the cells within a cell and a half (the warp moves up to ½ m).
    let centre = vec2<i32>(floor(p.xz / CELL - header.xy));
    if any(centre < vec2(-2)) || any(centre >= size + 2) {
        return s;
    }
    var types: array<f32, 3>;
    var count = 0u;
    for (var dz = -2; dz <= 2; dz++) {
        for (var dx = -2; dx <= 2; dx++) {
            let m = mask_at(centre + vec2(dx, dz), size);
            let pair = floor(m.rg * 255.0 + 0.5);
            for (var k = 0u; k < 2u; k++) {
                let ty = pair[k];
                if ty == 0.0 || count >= MAX_TYPES {
                    continue;
                }
                var seen = false;
                for (var j = 0u; j < count; j++) {
                    seen = seen || types[j] == ty;
                }
                if !seen {
                    types[count] = ty;
                    count++;
                }
            }
        }
    }
    if count == 0u {
        return s;
    }
    if surface_params[1].z != 0.0 {
        s.albedo = vec3(0.0, 0.0, 1.0);
    }

    let gx = mirror(dpx);
    let gy = mirror(dpy);
    let distance = length((p - mirror(camera)).xz);
    let fade_distance = surface_params[1].y;
    if distance >= fade_distance {
        return s;
    }
    let fade = smooth01((fade_distance - distance) / (fade_distance + 1.5 - distance));
    let n_received = mirror(s_in.normal);
    var n = n_received;

    for (var k = 0u; k < count; k++) {
        let ty = types[k];
        let at = HEADER + 4u * u32(ty);
        let p0 = surface_params[at];
        if p0.x < 0.0 {
            continue;
        }
        let p1 = surface_params[at + 1u];
        let p2 = surface_params[at + 2u];
        let edge_color = surface_params[at + 3u].xyz;
        let layer = i32(p0.x + 0.5);
        // **Assumption**: the material's texture coordinates start at the subgrid's origin in
        // the game; here at the world origin (same tiling, another phase).
        let rel = p.xz;
        let uv_noise = rel / p0.y;
        let uv = rel / p0.w;
        let dn_x = gx.xz / p0.y;
        let dn_y = gy.xz / p0.y;
        let du_x = gx.xz / p0.w;
        let du_y = gy.xz / p0.w;
        let base = textureSampleGrad(surface_base, surface_sampler, uv, layer, du_x, du_y);
        // Edge warp: the noise's RG scaled by Intensity around 0, pulled towards ½ where the
        // base colour is transparent.
        let noise = textureSampleGrad(surface_noise, surface_sampler, uv_noise, layer, dn_x, dn_y).rg;
        var warp = noise * p0.z - p0.z * 0.5;
        warp = mix(warp, vec2(0.5), clamp(1.0 - base.a, 0.0, 1.0) * 0.25);
        let cov = coverage(p.xz + warp, ty);
        if !cov.any {
            continue;
        }
        // Off the AI grid's height by more than 0.1 m: fades out over 2.5 cm.
        let height_fade = 1.0 - smooth01((abs(cov.h - p.y) - 0.1) * 40.0);
        if surface_params[1].z != 0.0 {
            // `BG3_SURFACE_DEBUG=1`: red = coverage, green = height fade, blue = a candidate.
            s.albedo = vec3(cov.c, height_fade, 1.0);
            continue;
        }
        let c = cov.c * height_fade;
        // Edge shaping.
        let a = smoothstep_lin(0.6, 0.9, c);
        let b = smoothstep_lin(0.95, 0.96, c);
        let sv = c * clamp(base.a + b, 0.0, 1.0);
        let tt = clamp(a + sv, 0.0, 1.0);
        let lo = clamp(1.0 - clamp(p1.x, 0.1, 1.0), 0.2, 1.0);
        let hi = clamp(lo + p1.y, 0.3, 1.0);
        let e = smoothstep_lin(lo, hi, tt);
        let alpha = e * (p1.z + base.a * sv * (1.0 - p1.z));
        if alpha <= 0.0 {
            continue;
        }
        // Colour: the base colour, towards the edge colour where coverage is low.
        let colour = clamp(
            mix(base.rgb, edge_color, clamp(p2.x, 0.0, 1.0) * (1.0 - c)),
            vec3(0.0),
            vec3(1.0),
        );
        let ph = textureSampleGrad(surface_physical, surface_sampler, uv, layer, du_x, du_y);
        // Normal map (X in A, out-of-plane in B, Y in G along −Z), towards up by
        // Normal_Intensity, applied to the receiver as a rotation (as the level decals).
        // *Approximate*: no animated wave normals, the receiver's normal as the base.
        let nm = textureSampleGrad(surface_normal, surface_sampler, uv, layer, du_x, du_y);
        var tn = vec3(nm.a, nm.b, nm.g) * 2.0 - 1.0;
        tn.z = -tn.z;
        let up = vec3(0.0, 1.0, 0.0);
        let n_decal = normalize(mix(up, normalize(tn), clamp(p1.w, 0.0, 1.0)));
        let r = cross(n_decal, -up) * SURFACE_HALF_PI;
        let n_rotated = normalize(rotate_euler(r, n_received));
        n = normalize(mix(n, normalize(mix(n_received, n_rotated, alpha)), fade));

        let w = clamp(alpha * fade * p2.w, 0.0, 1.0);
        s.albedo = mix(s.albedo, colour, w);
        s.roughness = mix(s.roughness, clamp(ph.g, 0.0, 1.0), w);
        s.reflectance = mix(s.reflectance, clamp(p2.y, 0.0, 1.0), w);
        s.metallic = mix(s.metallic, clamp(ph.r, 0.0, 1.0), w);
    }
    s.normal = mirror(n);
    return s;
}
