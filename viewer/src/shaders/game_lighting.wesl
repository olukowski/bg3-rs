// The game's deferred light loop for materials drawn on Bevy's forward pass: Burley diffuse,
// GGX with the remapped Smith visibility and specular anti-aliasing (models 0, 3, 4), dual-lobe
// specular and profile transmission (3), transmission without the sun (4), the hair lobes (5),
// over Bevy's clustered point/spot lights and directional lights (with the game's falloff,
// `game_light.wesl`), the level's light probes and the emissive. Used by `character.wesl` and
// `level.wesl`; see docs/research/character-shaders.md ("Lighting") and materials.md.
//
// bg3: ClusteredLighting_* (DoClusteredLight, DoClusteredLight_Subsurface,
// DoClusteredLight_Translucent, Specular_Hair, DirectionLight_Default) (Core.metallib,
// 4.1.1.7631656)

@if(!PREPASS_PIPELINE)
import bevy_pbr::render::{
    pbr_types,
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

// `pow` as the game's shaders evaluate it.
@if(!PREPASS_PIPELINE)
fn spow3(x: vec3<f32>, y: vec3<f32>) -> vec3<f32> { return pow(max(abs(x), vec3(1e-4)), y); }

const MODEL_SSS: u32 = 3u;
const MODEL_TRANSLUCENT: u32 = 4u;
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

struct GameLight {
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

struct GameShading {
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
    // Diffusion profile of subsurface (3) and translucent (4) surfaces, as
    // `DiffusionProfile::uniform` packs it: scatter colour + distance, tint + kernel slot,
    // thickness remap (min, max − min) + thick-object flag + transmission on, dual specular.
    profile: array<vec4<f32>, 4>,
}

// Lit (0) and subsurface (3) surfaces: Burley diffuse, GGX (dual-lobe for 3), transmission.
@if(!PREPASS_PIPELINE)
fn shade_surface(sh: GameShading, li: GameLight) -> vec3<f32> {
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
            let ds = sh.profile[3];
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
    // Translucent surfaces (4) get no transmission from the sun.
    let transmits = sh.model == MODEL_SSS || (sh.model == MODEL_TRANSLUCENT && !li.sun);
    if transmits && sh.profile[2].w > 0.5 {
        let p = sh.profile;
        let w = 0.258819;
        let wt = clamp((w - n_l_raw) / ((1.0 + w) * (1.0 + w)), 0.0, 1.0);
        if wt > 0.0 {
            let t = p[2].x + p[2].y * sh.thickness;
            let sc = 1.0 / max(0.0966094 * p[0].rgb * p[0].w * sh.sss_amount, vec3(1e-4));
            let e = exp(-t * sc / 3.0);
            var tr = p[1].rgb * e * (e * e + 3.0);
            // Thin objects take the light's shadow. Thick ones take, instead, the larger of the
            // remapped thickness and the depth through the occluder in the light's shadow map
            // (0 for lights without shadows). Approximations: with a Bevy shadow map we gate by
            // the shadow instead of measuring that depth; for a game-shadowed light without one
            // we take the occluder as much thicker than the scatter length (no transmission).
            if p[2].z < 0.5 || li.has_shadow {
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
fn shade_hair(sh: GameShading, li: GameLight) -> vec3<f32> {
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
fn shade_light(sh: GameShading, li: GameLight) -> vec3<f32> {
    if sh.model == MODEL_HAIR {
        return shade_hair(sh, li);
    }
    return shade_surface(sh, li);
}

// Direct light from Bevy's clustered point/spot lights and directional lights, ambient, and
// emissive, with the game's BRDFs. Replaces `apply_pbr_lighting` for character surfaces.
@if(!PREPASS_PIPELINE)
fn game_lighting(in: pbr_types::PbrInput, sh: GameShading) -> vec3<f32> {
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
        var li: GameLight;
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
        var li: GameLight;
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
