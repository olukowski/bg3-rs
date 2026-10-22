// Bindless access for the viewer's material extensions (`level_materials.rs`,
// `char_materials.rs`). Both declare `index_table(range(100..148), binding(160))`, so the
// resource at non-bindless binding N is field N − 100 of the material's index table for every
// material type, and the shared modules (`game_lighting.wesl`, `decals.wesl`, `surfaces.wesl`)
// can find the level's box lights, decals and surfaces without knowing which material they
// shade. Without bindless (terrain, or a device without buffer binding arrays) none of this is
// used and the extensions bind their resources at 100–147.

import bevy_pbr::render::mesh_bindings::mesh;

// Fields per index table entry (`range(100..148)`).
const TABLE_LEN: u32 = 48u;

@if(BINDLESS)
@group(constants::MATERIAL_BIND_GROUP) @binding(160) var<storage> ext_indices: array<u32>;

// The slot of the material being shaded; fragment entry points call `set_slot` first.
var<private> material_slot: u32;

fn set_slot(instance_index: u32) {
    material_slot = mesh[instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
}

// The binding-array index of the current material's resource at non-bindless `binding`.
@if(BINDLESS)
fn ext_index(binding: u32) -> u32 {
    return ext_indices[material_slot * TABLE_LEN + binding - 100u];
}
