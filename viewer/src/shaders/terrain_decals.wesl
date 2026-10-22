// `BG3_STANDARD_TERRAIN=1` tiles: Bevy's standard material with the level's decals blended into its inputs
// before lighting. The game's terrain G-buffer shader always sets the receives-decals bit
// (docs/research/decals.md); the terrain itself stays on Bevy's lighting (`terrain.rs`).
//
// bg3: Terrain_PS* (Core.metallib, 4.1.1.7631656)

import bevy_pbr::render::{
    pbr_types,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
};
import bg3_viewer::shaders::decals::{DecalSurface, apply_decals};

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;
    let dp_x = dpdx(in.world_position.xyz);
    let dp_y = dpdy(in.world_position.xyz);
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    );
    var ds: DecalSurface;
    ds.albedo = pbr_input.material.base_color.rgb;
    ds.normal = pbr_input.N;
    ds.roughness = pbr_input.material.perceptual_roughness;
    ds.reflectance = pbr_input.material.reflectance.x;
    ds.metallic = pbr_input.material.metallic;
    ds = apply_decals(ds, in.world_position.xyz, view_bindings::view.world_position, dp_x, dp_y);
    pbr_input.material.base_color = vec4(ds.albedo, pbr_input.material.base_color.a);
    pbr_input.N = ds.normal;
    pbr_input.material.perceptual_roughness = ds.roughness;
    pbr_input.material.reflectance = vec3(ds.reflectance);
    pbr_input.material.metallic = ds.metallic;

    var out: FragmentOutput;
    if (pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT) != 0u {
        out.color = pbr_input.material.base_color;
    } else {
        out.color = apply_pbr_lighting(pbr_input);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
