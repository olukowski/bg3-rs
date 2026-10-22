// The game's bloom (see `post.rs` and docs/research/lighting.md, "Bloom"): a thresholded
// half-resolution copy of the pre-exposed HDR scene, a chain of 3×3 tent downsamples to 1/32,
// tent upsamples adding each level back, and 0.2 × the result added to the scene before grading
// and tonemapping. Written from our description of the shaders, not translated from them.
//
// bg3: PPBloomScaleBiasDownsampleAndBlur_PS, PPBloomDownsampleAndBlur_PS,
// PPBloomUpsampleAndBlur_PS, PPBloomUpsampleBlurAndCombine_PS (Core.metallib),
// ls::PostProcessStep_Bloom::ExecuteWTKernel (4.1.1.7631656)

import bevy_render::view::View;
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct BloomUniform {
    // (Threshold, Amount, blur scale = screen height / 1080, combine weight)
    params: vec4<f32>,
    // (1 / destination width, 1 / destination height, 0, 0)
    texel: vec4<f32>,
}

struct ExposureStateRead {
    ev100: f32,
    frames: u32,
    pad: vec2<f32>,
    histogram: array<vec4<u32>, 64>,
}

@group(0) @binding(0) var base: texture_2d<f32>;
@group(0) @binding(1) var base2: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
@group(0) @binding(3) var<uniform> view: View;
@group(0) @binding(4) var<uniform> bloom: BloomUniform;
@group(0) @binding(5) var<storage, read> state: ExposureStateRead;

// Our frame is lit with the camera's exposure, read back a few frames late; the game's with the
// GPU's current one (as in `post.wesl`'s tonemap).
fn exposure_correction() -> f32 {
    if state.frames == 0u {
        return 1.0;
    }
    return 1.0 / (1.2 * exp2(state.ev100)) / view.exposure;
}

fn sample(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t, linear_clamp, clamp(uv, vec2(0.0), vec2(1.0)), 0.0);
}

// 3×3 tent: centre 1/4, edges 1/8, corners 1/16, taps `o` apart.
fn tent(t: texture_2d<f32>, uv: vec2<f32>, o: vec2<f32>) -> vec4<f32> {
    var c = sample(t, uv) * 0.25;
    c += (sample(t, uv + vec2(-o.x, -o.y)) + sample(t, uv + vec2(o.x, -o.y))
        + sample(t, uv + vec2(-o.x, o.y)) + sample(t, uv + vec2(o.x, o.y))) * 0.0625;
    c += (sample(t, uv + vec2(-o.x, 0.0)) + sample(t, uv + vec2(0.0, -o.y))
        + sample(t, uv + vec2(o.x, 0.0)) + sample(t, uv + vec2(0.0, o.y))) * 0.125;
    return c;
}

// Full resolution → 1/2: four taps a quarter destination texel apart (one source texel each),
// each scaled by sat(Amount · (luma − Threshold)) with luma weights 0.3/0.59/0.11, and the
// smallest of the four kept (per channel).
@fragment
fn threshold(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let k = exposure_correction();
    let o = 0.25 * bloom.texel.xy;
    var m = vec4(3.0e38);
    for (var i = 0u; i < 4u; i += 1u) {
        let s = vec2(select(-1.0, 1.0, (i & 1u) != 0u), select(-1.0, 1.0, (i & 2u) != 0u));
        let c = sample(base, in.uv + s * o).rgb * k;
        let w = saturate(bloom.params.y * (dot(c, vec3(0.3, 0.59, 0.11)) - bloom.params.x));
        m = min(m, vec4(c * w, 1.0));
    }
    return m;
}

// 1/n → 1/2n: tent with taps one destination texel × (height / 1080) apart.
@fragment
fn downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return tent(base, in.uv, bloom.texel.xy * bloom.params.z);
}

// 1/2n + 1/n → 1/n: this level plus `Amount` (1) × the tent of the level below, taps two
// destination texels × (height / 1080) apart.
@fragment
fn upsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return sample(base, in.uv) + tent(base2, in.uv, 2.0 * bloom.texel.xy * bloom.params.z);
}

// Scene + 0.2 × bloom (bilinear from 1/2), back in our exposure.
@fragment
fn combine(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let scene = textureLoad(base, vec2<i32>(in.position.xy), 0);
    let b = sample(base2, in.uv).rgb * bloom.params.w / exposure_correction();
    return vec4(scene.rgb + b, scene.a);
}
