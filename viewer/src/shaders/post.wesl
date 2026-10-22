// The game's auto-exposure, colour grading and SDR display transform (see `post.rs` and
// docs/research/lighting.md, "Exposure" and "Tonemapping"). Written from our description of the
// shaders, not translated from them.
//
// bg3: PPCalculateEV, PPHistogram, PPExposure, PPComputeLUT, PPColorGradingApplyLUTPS,
// PPToneMap_LUT (Core.metallib, 4.1.1.7631656)

import bevy_render::view::View;
import bevy_render::globals::Globals;
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

const BINS: u32 = 256u;
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct PostUniform {
    // (ExposureMin, ExposureMax, ExposureCompensation, auto (1) / fixed (0))
    exposure: vec4<f32>,
    // (min percentile, max percentile, metering mode, fixed EV100)
    metering: vec4<f32>,
    // (histogram EV minimum, EV range, EV texture width, height)
    histogram: vec4<f32>,
    // (Contrast, Highlights, Shadows, Pivot)
    grading: vec4<f32>,
    // (Brightness, Saturation, Hue (degrees), 0)
    grading2: vec4<f32>,
    // Global, Dark, Shadow, Midtones, Highlight, Specular: (XYOffset.x, XYOffset.y, Saturation,
    // 1.35^ExposureShift)
    ranges: array<vec4<f32>, 6>,
}

// EV100 → the scale the lighting is multiplied by (as `bevy::camera::Exposure`).
fn exposure_scale(ev100: f32) -> f32 {
    return 1.0 / (1.2 * exp2(ev100));
}

// ---------------------------------------------------------------------------------------------
// Exposure

struct ExposureState {
    ev100: f32,
    frames: u32,
    // The last metered target EV100 before adaptation and clamping (`BG3_POST_DEBUG`).
    target_ev: f32,
    pad: f32,
    histogram: array<atomic<u32>, 256>,
}

struct ExposureStateRead {
    ev100: f32,
    frames: u32,
    pad: vec2<f32>,
    histogram: array<vec4<u32>, 64>,
}

@if(EXPOSURE)
@group(0) @binding(0) var<uniform> globals: Globals;
@if(EXPOSURE)
@group(0) @binding(1) var<uniform> view: View;
@if(EXPOSURE)
@group(0) @binding(2) var<uniform> post: PostUniform;
@if(EXPOSURE)
@group(0) @binding(3) var hdr: texture_2d<f32>;
@if(EXPOSURE)
@group(0) @binding(4) var hdr_sampler: sampler;
@if(EXPOSURE)
@group(0) @binding(5) var<storage, read_write> state: ExposureState;

@if(EXPOSURE)
var<workgroup> local_bins: array<atomic<u32>, 256>;

// One texel of the game's EV texture (640×384, or 1280×768 for large screens): four bilinear
// taps a quarter texel apart, Rec.709 luminance, un-pre-exposed, as EV100 (K = 12.5).
@if(EXPOSURE)
fn texel_ev(p: vec2<u32>) -> f32 {
    let size = post.histogram.zw;
    let uv = vec2<f32>(p) / size;
    let o = 0.25 / size;
    var sum = vec3(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let s = vec2(select(-1.0, 1.0, (i & 1u) != 0u), select(-1.0, 1.0, (i & 2u) != 0u));
        sum += textureSampleLevel(hdr, hdr_sampler, clamp(uv + s * o, vec2(0.0), vec2(1.0)), 0.0).rgb;
    }
    let l = dot(sum, LUMA) * 0.25;
    if !(l < 3.0e38) {
        return 0.0;
    }
    return log2(max(l / view.exposure, 1e-5)) + 3.0;
}

// Metering weight: mode 1 is centre-weighted (1 at the centre, 0 at the edge midpoints).
@if(EXPOSURE)
fn metering_weight(p: vec2<u32>) -> f32 {
    if post.metering.z < 0.5 {
        return 1.0;
    }
    let size = post.histogram.zw;
    let d = (vec2<f32>(p) - size * 0.5) * 2.0 / size;
    return 1.0 - saturate(length(d));
}

@if(EXPOSURE)
@compute @workgroup_size(16, 16, 1)
fn histogram(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    atomicStore(&local_bins[li], 0u);
    workgroupBarrier();
    let size = vec2<u32>(post.histogram.zw);
    if id.x < size.x && id.y < size.y {
        let ev = texel_ev(id.xy);
        let w = metering_weight(id.xy);
        let min_log = post.histogram.x;
        if ev > min_log && w > 1e-5 {
            // Split linearly between the two nearest bins, in 1/256 fixed point.
            let t = saturate((ev - min_log) / post.histogram.y) * 255.0;
            let i = u32(floor(t));
            let f = fract(t);
            let j = min(i + 1u, BINS - 1u);
            atomicAdd(&local_bins[i], u32(w * (1.0 - f) * 256.0));
            atomicAdd(&local_bins[j], u32(w * f * 256.0));
        }
    }
    workgroupBarrier();
    let n = atomicLoad(&local_bins[li]);
    if n > 0u {
        atomicAdd(&state.histogram[li], n);
    }
}

@if(EXPOSURE)
@compute @workgroup_size(1, 1, 1)
fn average() {
    var total = 0.0;
    for (var i = 0u; i < BINS; i += 1u) {
        total += f32(atomicLoad(&state.histogram[i])) / 256.0;
    }
    // Drop the lowest `min percentile` of the mass, then average until `max percentile`.
    var lo = total * post.metering.x;
    var hi = total * post.metering.y;
    var sum = 0.0;
    var count = 0.0;
    for (var i = 0u; i < BINS; i += 1u) {
        var v = f32(atomicLoad(&state.histogram[i])) / 256.0;
        atomicStore(&state.histogram[i], 0u);
        let m = min(lo, v);
        lo -= m;
        v -= m;
        hi -= m;
        let w = min(hi, v);
        hi -= w;
        sum += w * (f32(i) / 255.0 * post.histogram.y + post.histogram.x);
        count += w;
    }
    var ev = state.ev100;
    if post.exposure.w < 0.5 {
        ev = post.metering.w;
    } else if total > 0.0 {
        let target_ev = sum / max(count, 1e-4) - post.exposure.z;
        state.target_ev = target_ev;
        if state.frames == 0u {
            // First frame: straight to the target (the game's immediate adaptation mode).
            ev = target_ev;
        } else {
            let delta = target_ev - ev;
            let k = select(1.0, 2.0, delta > 0.0);
            ev += delta * (1.0 - exp(-k * globals.delta_time));
        }
        ev = clamp(ev, post.exposure.x, post.exposure.y);
    }
    state.ev100 = ev;
    state.frames = min(state.frames + 1u, 1000000u);
}

// ---------------------------------------------------------------------------------------------
// Grading and display

@if(!EXPOSURE)
@group(0) @binding(0) var source: texture_2d<f32>;
@if(!EXPOSURE)
@group(0) @binding(1) var source_sampler: sampler;
@if(!EXPOSURE)
@group(0) @binding(2) var<uniform> view: View;
@if(!EXPOSURE)
@group(0) @binding(3) var<uniform> post: PostUniform;
@if(!EXPOSURE)
@group(0) @binding(4) var<storage, read> state: ExposureStateRead;
@if(!EXPOSURE)
@group(0) @binding(5) var display_lut: texture_3d<f32>;
@if(!EXPOSURE)
@group(0) @binding(6) var lut_sampler: sampler;

struct Vignette {
    // (VignetteColor, on)
    color: vec4<f32>,
    // (VignetteIntensity, VignettePower, 0, 0)
    params: vec4<f32>,
}
@if(!EXPOSURE)
@group(0) @binding(7) var<uniform> vignette: Vignette;

// Linear sRGB → ACES AP1 (with the D65 → D60 adaptation).
const SRGB_TO_AP1: mat3x3<f32> = mat3x3<f32>(
    vec3(0.613094151, 0.070195667, 0.020615595),
    vec3(0.339519918, 0.916356981, 0.109569855),
    vec3(0.047377691, 0.013453229, 0.869815409),
);
const AP1_Y: vec3<f32> = vec3<f32>(0.272228718, 0.674081743, 0.0536895171);
const EXPAND_GAMUT: mat3x3<f32> = mat3x3<f32>(
    vec3(1.39103854, -0.0823024139, -0.0241881087),
    vec3(-0.3243801, 1.09224474, -0.0883711576),
    vec3(-0.0666656047, -0.00993920118, 1.1125598),
);
const AP1_TO_LMS: mat3x3<f32> = mat3x3<f32>(
    vec3(0.63169384, 0.270069808, 0.0987901613),
    vec3(0.348861217, 0.630950511, 0.185321286),
    vec3(0.019335242, 0.0990051925, 0.716367424),
);
const LMS_TO_LAB: mat3x3<f32> = mat3x3<f32>(
    vec3(0.210454255, 1.9779985, 0.0259040371),
    vec3(0.793617785, -2.42859221, 0.782771766),
    vec3(-0.00407204684, 0.45059371, -0.808675766),
);
const LAB_TO_LMS: mat3x3<f32> = mat3x3<f32>(
    vec3(1.0, 1.0, 1.0),
    vec3(0.396337777, -0.105561346, -0.0894841775),
    vec3(0.215803757, -0.0638541728, -1.29148555),
);
const LMS_TO_AP1: mat3x3<f32> = mat3x3<f32>(
    vec3(2.06879282, -0.876324654, -0.0585943535),
    vec3(-1.17516816, 2.1497612, -0.394073457),
    vec3(0.106573246, -0.273453206, 1.45197642),
);

fn spow(x: f32, y: f32) -> f32 {
    return pow(max(abs(x), 1e-4), y);
}

@if(!EXPOSURE)
fn shadows_curve(c: vec3<f32>, s: f32) -> vec3<f32> {
    if s == 0.0 {
        return c;
    }
    let y = max(dot(c, AP1_Y), 0.0);
    let q = exp2(min(0.005 * s - 1.0, -1.0));
    let t = 0.18 * q;
    if !(y > 0.0 && y < t) {
        return c;
    }
    let k = exp2(0.06 * s);
    var p = 1.3435;
    if s > 0.0 {
        p = pow(0.453, 0.06 * s) * ((max(0.0, 0.015 * s - 0.5) + 4.0) * 0.175 + 0.825);
    }
    let a = (k - 1.0) * p + (2.0 - k);
    let b = (3.0 - (2.0 * k + a)) / t;
    let cc = a - (2.0 * b * t + k);
    let d = 0.54 * q * t;
    let r = spow(y / t, p);
    return c * (t * r * (cc * y / d + b) + k);
}

@if(!EXPOSURE)
fn highlights_curve(c: vec3<f32>, h: f32) -> vec3<f32> {
    let m = length(c) * 0.577350259;
    if h == 0.0 || !(m > 0.36 && m < 5.76) {
        return c;
    }
    let e = 0.035 * h;
    let a = pow(0.36, e);
    let b = e + 1.0;
    let m1 = (spow(m, b) - 0.36 * a) / (a * b + 1e-10) + 0.36;
    let f = select(saturate(m1 / m), saturate(m / m1), m1 > m);
    return c * (mix(m, m1, f) / m);
}

// Matrices below are written as rows.
fn rows(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> mat3x3<f32> {
    return transpose(mat3x3<f32>(a, b, c));
}

fn pq(y: f32) -> f32 {
    let p = pow(y, 0.1593017578125);
    return pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), 78.84375);
}

fn pq_inverse(x: f32) -> f32 {
    let n = pow(x, 1.0 / 78.84375);
    return pow(max(n - 0.8359375, 0.0) / (18.8515625 - 18.6875 * n), 1.0 / 0.1593017578125);
}

// XYZ (D65) → ICtCp, with 1.0 = 1 cd/m² and a soft gamut clip in Rec.2020.
fn xyz_to_ictcp(xyz: vec3<f32>) -> vec3<f32> {
    let to_2020 = rows(
        vec3(1.71665096, -0.35567075, -0.253366232),
        vec3(-0.66668427, 1.61648107, 0.0157685373),
        vec3(0.0176398568, -0.042770613, 0.942103088),
    );
    var rgb = to_2020 * xyz;
    let m = max(max(rgb.r, rgb.g), rgb.b);
    let n = rgb / (m + 1e-10);
    let mn = min(min(n.r, n.g), n.b);
    let s = 1.0 - mn;
    var s2 = s;
    if s >= 0.95 {
        s2 = 0.925 + 0.225 * sqrt(max(0.0625 - mn, 0.0));
    }
    if s != 0.0 {
        rgb = max(m * mix(vec3(1.0), n, s2 / s), vec3(0.0));
    }
    let to_lms = rows(
        vec3(0.412109375, 0.523925781, 0.0639648438),
        vec3(0.166748047, 0.720458984, 0.112792969),
        vec3(0.0241699219, 0.0754394531, 0.900390625),
    );
    let lms = to_lms * rgb;
    let e = sign(lms) * vec3(pq(abs(lms.x) * 1e-4), pq(abs(lms.y) * 1e-4), pq(abs(lms.z) * 1e-4));
    return rows(
        vec3(0.5, 0.5, 0.0),
        vec3(1.61376953, -3.32348633, 1.7097168),
        vec3(4.37817383, -4.24560547, -0.132568359),
    ) * e;
}

// Global hue rotation (degrees) and saturation, in ICtCp: intensity and chroma of the
// saturated colour, hue of the original (rotated). AP1 in and out.
@if(!EXPOSURE)
fn hue_saturation(c: vec3<f32>) -> vec3<f32> {
    let saturation = post.grading2.y;
    let hue = post.grading2.z;
    if saturation == 1.0 && hue == 0.0 {
        return c;
    }
    let ap1_to_xyz = rows(
        vec3(0.652237535, 0.1282361, 0.169982255),
        vec3(0.267671794, 0.674338996, 0.0579877384),
        vec3(-0.00538181374, 0.00136906456, 1.09306991),
    );
    let y = max(dot(c, AP1_Y), 0.0);
    let cs = mix(vec3(y), c, saturation);
    let q0 = xyz_to_ictcp(ap1_to_xyz * c);
    let q1 = xyz_to_ictcp(ap1_to_xyz * cs);
    var h0 = 0.0;
    if length(q0.yz) > 1e-3 {
        h0 = atan2(q0.z, q0.y);
        if h0 < 0.0 {
            h0 += 6.28318531;
        }
    }
    let d = max(saturate(saturation - 1.0) * 0.0025, 0.0);
    let chroma = length(q1.yz) - d;
    let i = q1.x - d;
    let h = h0 + hue * 0.0174532924;
    let ct = chroma * cos(h);
    let cp = chroma * sin(h);
    let e = rows(
        vec3(1.0, 0.00860903692, 0.111029625),
        vec3(1.0, -0.00860903692, -0.111029625),
        vec3(1.0, 0.560031354, -0.320627183),
    ) * vec3(i, ct, cp);
    let lms = sign(e) * 10000.0 * vec3(pq_inverse(abs(e.x)), pq_inverse(abs(e.y)), pq_inverse(abs(e.z)));
    let xyz = rows(
        vec3(2.07015252, -1.32634759, 0.206651047),
        vec3(0.364738494, 0.680566132, -0.0453045554),
        vec3(-0.04974721, -0.0492609628, 1.18806601),
    ) * lms;
    return rows(
        vec3(1.6605773, -0.315296024, -0.241509631),
        vec3(-0.659922779, 1.60839415, 0.0172986612),
        vec3(0.00900251884, -0.0035668863, 0.913644135),
    ) * xyz;
}

// One grading range in Oklab: chroma offset scaled by lightness, saturation, then the exposure
// factor on everything.
@if(!EXPOSURE)
fn grade_range(lab: vec3<f32>, r: vec4<f32>) -> vec3<f32> {
    let lc = min(lab.x, 1.0);
    let ab = vec2(lab.y + r.y * lc, lab.z - r.x * lc) * r.z;
    return vec3(lab.x, ab) * r.w;
}

@if(!EXPOSURE)
fn oklab_ranges(c: vec3<f32>) -> vec3<f32> {
    let lms = pow(max(AP1_TO_LMS * c, vec3(0.0)), vec3(1.0 / 3.0));
    let lab = LMS_TO_LAB * lms;
    let l = lab.x;
    let w_dark = 1.0 - smoothstep(0.0, 0.3, l);
    let w_shadow = 1.0 - smoothstep(0.0, 0.7, l);
    let w_high = smoothstep(0.8, 1.2, l);
    let w_spec = smoothstep(1.2, 1.4, l);
    let w_mid = smoothstep(0.0, 0.7, l) - w_high;
    let g = grade_range(lab, post.ranges[0]);
    let sh = grade_range(g, post.ranges[2]);
    let shadow = mix(sh, grade_range(sh, post.ranges[1]), w_dark);
    let mid = grade_range(g, post.ranges[3]);
    let hi = grade_range(g, post.ranges[4]);
    let high = mix(hi, grade_range(hi, post.ranges[5]), w_spec);
    let out = shadow * w_shadow + mid * w_mid + high * w_high;
    let lms2 = LAB_TO_LMS * out;
    return LMS_TO_AP1 * (lms2 * lms2 * lms2);
}

// The grading LUT's per-texel math, evaluated per pixel: pre-exposed linear sRGB in, graded
// linear AP1 out. (The game bakes this into a 32³ LUT indexed by PQ(x/222).)
@if(!EXPOSURE)
fn grade(x_in: vec3<f32>) -> vec3<f32> {
    var c = SRGB_TO_AP1 * (min(x_in, vec3(222.0)) + post.grading2.x);
    // Gamut expansion of saturated colours.
    let y = max(dot(c, AP1_Y), 0.0);
    let chroma = c / (y + 1e-10) - 1.0;
    let amount = (1.0 - exp2(-4.0 * dot(chroma, chroma))) * (1.0 - exp2(-4.0 * y * y));
    c = mix(c, EXPAND_GAMUT * c, amount);
    c = shadows_curve(c, post.grading.z);
    c = highlights_curve(c, post.grading.y);
    // White balance: identity at 6500 K / tint 0 (the only case `post.rs` passes through).
    c = hue_saturation(c);
    c = oklab_ranges(c);
    // Contrast around log2(0.18 · 2^Pivot) on AP1 luminance.
    let yc = max(dot(c, AP1_Y), 0.0);
    if yc > 0.0 {
        let lp = log2(0.18 * exp2(post.grading.w));
        let y2 = exp2((log2(yc) - lp) * post.grading.x + lp);
        c *= y2 / yc;
    } else {
        c = vec3(0.0);
    }
    return c - min(min(min(c.r, c.g), c.b), 0.0);
}

// Reference gamut compression of the display pass.
fn compress_distance(d: f32, threshold: f32, s: f32) -> f32 {
    if d < threshold {
        return d;
    }
    var x = 0.0;
    if s > 1e-6 {
        x = (d - threshold) / s;
    }
    if x <= 0.0 {
        return threshold;
    }
    return threshold + s * x / pow(1.0 + pow(x, 1.2), 1.0 / 1.2);
}

fn gamut_compress(c: vec3<f32>) -> vec3<f32> {
    let ach = max(max(c.r, c.g), c.b);
    if ach == 0.0 {
        return c;
    }
    let d = (ach - c) / abs(ach);
    // Scale s_i = limit_i / (q_i^-1.2 − 1)^(1/1.2) from the thresholds and limits.
    let num = vec3(0.332, 0.461, 0.312);
    let q = vec3(0.557229, 0.427332, 0.0);
    let s = vec3(
        num.x / pow(pow(q.x, -1.2) - 1.0, 1.0 / 1.2),
        num.y / pow(pow(q.y, -1.2) - 1.0, 1.0 / 1.2),
        0.0,
    );
    let dc = vec3(
        compress_distance(d.x, 0.815, s.x),
        compress_distance(d.y, 0.803, s.y),
        compress_distance(d.z, 1.0, s.z),
    );
    return ach - dc * abs(ach);
}

fn acescc(c: vec3<f32>) -> vec3<f32> {
    var x = max(c, vec3(0.0));
    x = select(x, x * 0.5 + exp2(-16.0), x <= vec3(exp2(-15.0)));
    return saturate((log2(x) + 9.72) * 0.0570776239);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

@if(!EXPOSURE)
@fragment
fn tonemap(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(source, source_sampler, in.uv, 0.0);
    // The frame was lit with the camera's exposure (read back a few frames late); correct it to
    // the GPU's current one, as the game lights with last frame's exposure.
    var k = 1.0;
    if state.frames > 0u {
        k = exposure_scale(state.ev100) / view.exposure;
    }
    let graded = grade(max(c.rgb * k, vec3(0.0)));
    // The game samples its LUTs at the raw coordinate (no half-texel remap); so do we.
    var encoded = textureSampleLevel(display_lut, lut_sampler, acescc(gamut_compress(graded)), 0.0).rgb;
    // The vignette step (after the tonemap, on the display-encoded colour): towards the
    // vignette colour by sat(Intensity · |(|2(uv − ½)|)^Power|).
    // bg3: PPVignette_PS, ls::PostProcessStep_Vignette::ExecuteWTKernel (4.1.1.7631656)
    if vignette.color.w > 0.5 {
        let d = pow(max(abs((in.uv - 0.5) * 2.0), vec2(1e-4)), vec2(vignette.params.y));
        let w = saturate(vignette.params.x * length(d));
        encoded = mix(encoded, vignette.color.rgb, w);
    }
    // The LUT outputs display-encoded sRGB; Bevy's sRGB swapchain re-encodes linear values.
    return vec4(srgb_to_linear(encoded), 1.0);
}
