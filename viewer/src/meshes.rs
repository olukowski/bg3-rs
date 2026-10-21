//! GR2 → Bevy mesh conversion for level visuals.

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bg3_vfs::Vfs;
use rayon::prelude::*;

use crate::level::LoadedLevel;
use crate::space;

/// Converted LOD0 meshes per visual resource ID.
#[derive(Resource, Default)]
pub struct VisualMeshes {
    /// visual ID → [(mesh, material ID)]
    pub by_visual: HashMap<String, Vec<(Mesh, Option<String>)>>,
    pub failed: Vec<(String, String)>,
}

impl VisualMeshes {
    /// Parse every GR2 the level needs (in parallel) and convert its LOD0 meshes.
    pub fn load(vfs: &Vfs, level: &LoadedLevel) -> Self {
        let visuals: HashSet<&str> = level
            .level
            .objects
            .iter()
            .filter_map(|o| level.visual_id(o))
            .filter(|v| level.banks.visuals.contains_key(*v))
            .collect();
        let results: Vec<_> = visuals
            .into_par_iter()
            .map(|id| {
                let res = convert_visual(vfs, level, id);
                (id.to_owned(), res)
            })
            .collect();
        let mut out = Self::default();
        for (id, res) in results {
            match res {
                Ok(meshes) => {
                    out.by_visual.insert(id, meshes);
                }
                Err(e) => out.failed.push((id, e)),
            }
        }
        out
    }
}

fn convert_visual(
    vfs: &Vfs,
    level: &LoadedLevel,
    id: &str,
) -> Result<Vec<(Mesh, Option<String>)>, String> {
    let visual = &level.banks.visuals[id];
    let path = visual
        .source_file
        .as_deref()
        .ok_or("visual has no SourceFile")?;
    let data = vfs.read(path).map_err(|e| e.to_string())?;
    let scene = bg3_gr2::Scene::from_bytes(&data).map_err(|e| format!("{path}: {e}"))?;
    let mut out = Vec::new();
    for obj in visual.objects.iter().filter(|o| o.lod == 0) {
        let Some(mesh) = scene.meshes.iter().find(|m| m.name == obj.mesh_name()) else {
            continue;
        };
        out.push((to_bevy_mesh(mesh), obj.material_id.clone()));
    }
    if out.is_empty() {
        return Err(format!("{path}: no LOD0 meshes matched"));
    }
    Ok(out)
}

pub fn to_bevy_mesh(m: &bg3_gr2::Mesh) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    let positions: Vec<[f32; 3]> = m.positions.iter().map(|&p| space::vec3(p)).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    if m.normals.len() == m.positions.len() {
        let normals: Vec<[f32; 3]> = m.normals.iter().map(|&n| space::vec3(n)).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    if m.tangents.len() == m.positions.len() {
        let tangents: Vec<[f32; 4]> = m.tangents.iter().map(|&t| space::tangent(t)).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
    }
    if let Some(uv) = m.uvs.first().filter(|uv| uv.len() == m.positions.len()) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv.clone());
    }
    let mut indices = m.indices.clone();
    space::flip_winding(&mut indices);
    mesh.insert_indices(Indices::U32(indices));
    if m.normals.len() != m.positions.len() {
        mesh.compute_flat_normals();
    }
    mesh
}
