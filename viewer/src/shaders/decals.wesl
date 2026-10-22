// Level decals applied to a level surface's material inputs before lighting, as the game's
// deferred decal pass blends them into the G-buffer: per decal, the pixel's position in the
// decal's unit box, the distance and angle fades, base colour / roughness / reflectance /
// metalness blended by alpha, and the decal's normal map applied as a rotation of the
// receiver's normal. Decals come from a world-space grid in draw order (`viewer/src/decals.rs`).
// Written from our description in docs/research/decals.md, not translated from the game's
// shaders; the math runs in game (left-handed) space, Bevy's world with z mirrored.
//
// bg3: Engine_Materials_Engine_Default_Decal_Deferred_ST_DEF_fragmentMain (Materials.metallib,
// 4.1.1.7631656), ls::DecalObject::PrepareDraw (4.1.1.7631656)

struct Decal {
    // Game world → unit box (inside within ±½).
    local_from_world: mat4x4<f32>,
    // Box axes in game space; x.w = texture layer, y.w = reflectance.
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    // Tiling (xy) and offset (zw).
    tiling: vec4<f32>,
    // Normal blending factor, fade distance, angle cutoff, opacity.
    props: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(110) var<storage, read> decals: array<Decal>;
// Header (origin x, origin z, 1/cell as f32 bits, cells x, cells z, debug tint), (first, count)
// per cell,
// then the decal indices.
@group(constants::MATERIAL_BIND_GROUP) @binding(111) var<storage, read> decal_grid: array<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(112) var decal_albedo: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(113) var decal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(114) var decal_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(115) var decal_physical: texture_2d_array<f32>;

const HALF_PI: f32 = 1.5707964;
// The angle fade's slope: 1 / 0.17.
const ANGLE_FADE_SLOPE: f32 = 5.882353;
const GRID_HEADER: u32 = 8u;

// Material inputs a decal can change (G-buffer targets 0–2).
struct DecalSurface {
    albedo: vec3<f32>,
    // Bevy world space.
    normal: vec3<f32>,
    roughness: f32,
    reflectance: f32,
    metallic: f32,
}

fn mirror(v: vec3<f32>) -> vec3<f32> {
    return vec3(v.x, v.y, -v.z);
}

// Rotation by Euler angles r (about x, then y, then z), applied to v.
fn rotate_euler(r: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let ca = cos(r.x);
    let sa = sin(r.x);
    let cb = cos(r.y);
    let sb = sin(r.y);
    let cc = cos(r.z);
    let sc = sin(r.z);
    let row0 = vec3(cb * cc, -cb * sc, sb);
    let row1 = vec3(ca * sc + sa * sb * cc, ca * cc - sa * sb * sc, -sa * cb);
    let row2 = vec3(sa * sc - ca * sb * cc, sa * cc + ca * sb * sc, ca * cb);
    return vec3(dot(row0, v), dot(row1, v), dot(row2, v));
}

// Blend every decal covering `world` (Bevy space) into `s`. `camera` is the view position;
// `dpx`/`dpy` are the screen derivatives of `world` (taken in uniform control flow).
fn apply_decals(
    s_in: DecalSurface,
    world: vec3<f32>,
    camera: vec3<f32>,
    dpx: vec3<f32>,
    dpy: vec3<f32>,
) -> DecalSurface {
    var s = s_in;
    let nx = decal_grid[3];
    let nz = decal_grid[4];
    if nx == 0u {
        return s;
    }
    let origin = vec2(bitcast<f32>(decal_grid[0]), bitcast<f32>(decal_grid[1]));
    let cell = floor((world.xz - origin) * bitcast<f32>(decal_grid[2]));
    if cell.x < 0.0 || cell.y < 0.0 || cell.x >= f32(nx) || cell.y >= f32(nz) {
        return s;
    }
    let c = GRID_HEADER + 2u * (u32(cell.y) * nx + u32(cell.x));
    let first = decal_grid[c];
    let count = decal_grid[c + 1u];
    if count == 0u {
        return s;
    }

    // Game space from here on.
    let p = mirror(world);
    let gx = mirror(dpx);
    let gy = mirror(dpy);
    let distance = length((p - mirror(camera)).xz);
    // The receiver's normal as written to the G-buffer (before any decal).
    let n_received = mirror(s_in.normal);
    // Flat geometric normal from the position's derivatives, facing the camera.
    var n_flat = normalize(cross(gx, gy));
    if dot(n_flat, mirror(camera) - p) < 0.0 {
        n_flat = -n_flat;
    }
    var n = n_received;

    for (var k = 0u; k < count; k++) {
        let d = decals[decal_grid[first + k]];
        let local = (d.local_from_world * vec4(p, 1.0)).xyz;
        if any(abs(local) > vec3(0.5)) {
            continue;
        }
        if decal_grid[5] != 0u {
            // `BG3_DECAL_DEBUG=1`: a checker of the box's local x (red) and z (green) quarters.
            let q = step(vec2(0.5), fract(local.xz * 4.0 + 0.5));
            s.albedo = vec3(q.x, q.y, 1.0);
        }
        let fade_distance = d.props.y;
        if distance >= fade_distance {
            continue;
        }
        let t = clamp((fade_distance - distance) / (fade_distance + 1.5 - distance), 0.0, 1.0);
        let fade = t * t * (3.0 - 2.0 * t);

        // Angle fade against the decal's up axis (off when the cutoff is 0).
        let up = d.axis_y.xyz;
        var angle = ANGLE_FADE_SLOPE;
        let cutoff = d.props.z;
        if cutoff > 0.0 {
            let a = clamp(dot(n_received, up), 0.0, 1.0) - cutoff;
            if a < 0.0 {
                continue;
            }
            angle = a * ANGLE_FADE_SLOPE;
        }

        let uv = vec2(local.x + 0.5, 0.5 - local.z) * d.tiling.xy + d.tiling.zw;
        let lx = (d.local_from_world * vec4(gx, 0.0)).xyz;
        let ly = (d.local_from_world * vec4(gy, 0.0)).xyz;
        let duv_x = vec2(lx.x, -lx.z) * d.tiling.xy;
        let duv_y = vec2(ly.x, -ly.z) * d.tiling.xy;
        let layer = i32(d.axis_x.w + 0.5);
        let base = textureSampleGrad(decal_albedo, decal_sampler, uv, layer, duv_x, duv_y);
        let alpha = min(base.a, clamp(angle, 0.0, 1.0));
        if alpha <= 0.0 {
            continue;
        }

        // Normal map: X in A, out-of-plane in B (along the decal's up axis), Y in G (along −Z).
        let nm = textureSampleGrad(decal_normal, decal_sampler, uv, layer, duv_x, duv_y);
        var tn = vec3(nm.a, nm.b, nm.g) * 2.0 - 1.0;
        tn.z = -tn.z;
        tn = normalize(tn);
        let n_decal = normalize(d.axis_x.xyz * tn.x + up * tn.y + d.axis_z.xyz * tn.z);
        // The rotation taking the decal's up to its normal, applied to the receiver: Euler
        // angles π/2 · (n × −up).
        let r = cross(n_decal, -up) * HALF_PI;
        let n_base = mix(n_flat, n_received, d.props.x);
        let n_rotated = normalize(rotate_euler(r, n_base));
        let n_out = normalize(mix(n_received, n_rotated, alpha));
        n = normalize(mix(n, n_out, fade));

        let a = clamp(d.props.w * alpha * fade, 0.0, 1.0);
        let ph = textureSampleGrad(decal_physical, decal_sampler, uv, layer, duv_x, duv_y);
        s.albedo = mix(s.albedo, clamp(base.rgb, vec3(0.0), vec3(1.0)), a);
        s.roughness = mix(s.roughness, clamp(ph.g, 0.0, 1.0), a);
        s.reflectance = mix(s.reflectance, clamp(d.axis_y.w, 0.0, 1.0), a);
        s.metallic = mix(s.metallic, clamp(ph.r, 0.0, 1.0), a);
    }
    s.normal = mirror(n);
    return s;
}
