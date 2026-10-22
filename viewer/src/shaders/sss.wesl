// Screen-space subsurface scattering: a separable 25-tap blur of subsurface pixels with the
// kernel of their diffusion profile, horizontal then vertical, following the surface's depth.
//
// bg3: PPSSSS_H, PPSSSS_V (Core.metallib, 4.1.1.7631656); kernels from
// ls::SubStage_SSSS (see docs/research/character-shaders.md). Our port differs in what it
// blurs: the game blurs diffuse irradiance and multiplies by albedo afterwards; Bevy's forward
// pass gives us only the final colour, so this blurs lit colour (albedo and specular included).
// Subsurface pixels carry −(1 + kernel index + amount/2) in alpha (see `character.wesl`).

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

const TAPS: u32 = 25u;
const PROFILES: u32 = 16u;

struct SssUniform {
    // Per profile: tap 0 is the centre weight, taps 1.. (rgb weight, offset in [-3, 3]).
    kernel: array<array<vec4<f32>, 25>, 16>,
    // Per profile: (scatter distance in mm, 0, 0, 0).
    profile: array<vec4<f32>, 16>,
    // (projection x scale, projection y scale, near plane, 0)
    camera: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_clamp: sampler;
@if(MULTISAMPLED)
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
@else
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var<uniform> u: SssUniform;

// View distance (m) at pixel `p`: Bevy's reverse-Z infinite projection stores near / z.
fn linear_depth(p: vec2<i32>) -> f32 {
    let dims = vec2<i32>(textureDimensions(depth));
    let q = clamp(p, vec2(0), dims - 1);
    let d = textureLoad(depth, q, 0);
    return u.camera.z / max(d, 1e-7);
}

fn decode(a: f32) -> vec2<f32> {
    let x = -a;
    let index = floor(x) - 1.0;
    return vec2(index, clamp(fract(x) * 2.0, 0.0, 1.0));
}

fn is_sss(a: f32) -> bool {
    return a <= -1.0;
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    let c = textureLoad(source, p, 0);
@if(VERTICAL)
    let out_alpha = 1.0;
@else
    let out_alpha = c.a;
    if !is_sss(c.a) {
        return vec4(c.rgb, out_alpha);
    }
    let code = decode(c.a);
    let k = u32(clamp(code.x, 0.0, f32(PROFILES - 1u)));
    let dist = u.profile[k].x;
    let width = dist * code.y;
    if width <= 0.0 {
        return vec4(c.rgb, out_alpha);
    }
    let depth_m = linear_depth(p);
    // Kernel offsets span [-3, 3] over the scatter width (mm → m), projected to UV.
@if(VERTICAL)
    let dir = vec2(0.0, u.camera.y);
@else
    let dir = vec2(u.camera.x, 0.0);
    let step = dir * (width * 0.001) / (2.0 * depth_m) / 3.0;
    let size = vec2<f32>(textureDimensions(source));

    var acc = c.rgb * u.kernel[k][0].rgb;
    for (var i = 1u; i < TAPS; i += 1u) {
        let tap = u.kernel[k][i];
        let uv = in.uv + tap.w * step;
        // Use the tap only where all four texels around it are subsurface pixels.
        let quad = textureGather(3, source, linear_clamp, uv);
        var col = c.rgb;
        if is_sss(quad.x) && is_sss(quad.y) && is_sss(quad.z) && is_sss(quad.w) {
            let s = textureSampleLevel(source, linear_clamp, uv, 0.0).rgb;
            let d = linear_depth(vec2<i32>(uv * size));
            // Follow the surface: fall back to the centre colour across depth steps.
            let t = clamp(1000.0 * abs(depth_m - d) / width, 0.0, 1.0);
            col = mix(s, c.rgb, t);
        }
        acc += tap.rgb * col;
    }
    return vec4(acc, out_alpha);
}
