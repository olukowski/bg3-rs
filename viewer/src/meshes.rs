//! GR2 → Bevy mesh conversion for level visuals.

use std::collections::{HashMap, HashSet};

use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bg3_vfs::Vfs;
use rayon::prelude::*;

use crate::level::LoadedLevel;
use crate::space;

/// One drawn mesh of a visual, with the camera distances (m) it is drawn at.
pub struct VisualMesh {
    pub mesh: Mesh,
    pub material: Option<String>,
    /// `[near, far)` from the object's origin; `None` = always (LOD 0 without lower LODs, or
    /// `BG3_LOD_SCALE=0`).
    pub range: Option<(f32, f32)>,
}

/// Converted meshes of every LOD per visual resource ID.
#[derive(Resource, Default)]
pub struct VisualMeshes {
    pub by_visual: HashMap<String, Vec<VisualMesh>>,
    pub failed: Vec<(String, String)>,
}

impl VisualMeshes {
    /// Parse every GR2 the level needs (in parallel) and convert the meshes of every LOD.
    pub fn load(vfs: &Vfs, level: &LoadedLevel) -> Self {
        let visuals: HashSet<&str> = level
            .level
            .objects
            .iter()
            .filter(|o| o.draws_visual(&level.templates))
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

fn convert_visual(vfs: &Vfs, level: &LoadedLevel, id: &str) -> Result<Vec<VisualMesh>, String> {
    let visual = &level.banks.visuals[id];
    let path = visual
        .source_file
        .as_deref()
        .ok_or("visual has no SourceFile")?;
    let data = vfs.read(path).map_err(|e| e.to_string())?;
    let scene = bg3_gr2::Scene::from_bytes(&data).map_err(|e| format!("{path}: {e}"))?;
    // (LOD, base name, LodDistance, mesh, material) of every drawn mesh.
    let mut drawn = Vec::new();
    let mut matched = false;
    for obj in &visual.objects {
        let Some(mesh) = scene.meshes.iter().find(|m| m.name == obj.mesh_name()) else {
            continue;
        };
        matched |= obj.lod == 0;
        // Proxies, occluders and cloth simulation meshes aren't drawn.
        if mesh.is_drawn() && !obj.cloth_proxy {
            let distance = mesh
                .properties
                .as_ref()
                .and_then(|p| p.lod_distance)
                .unwrap_or(f32::MAX);
            drawn.push((
                obj.lod,
                lod_base(&mesh.name),
                distance,
                mesh,
                obj.material_id.clone(),
            ));
        }
    }
    if !matched {
        return Err(format!("{path}: no LOD0 meshes matched"));
    }
    let scale = lod_scale();
    let mut out = Vec::new();
    for &(lod, base, distance, mesh, ref material) in &drawn {
        let siblings = || drawn.iter().filter(|d| d.1 == base);
        let range = if scale <= 0.0 {
            // LOD 0 only.
            (lod == 0).then_some(None)
        } else if siblings().all(|d| d.0 <= lod) {
            // The last LOD of its mesh: drawn from its predecessors' end on.
            let near = siblings()
                .filter(|d| d.0 < lod)
                .map(|d| d.2)
                .fold(0.0f32, f32::max);
            Some((near > 0.0).then_some((near * scale, f32::INFINITY)))
        } else {
            let near = siblings()
                .filter(|d| d.0 < lod)
                .map(|d| d.2)
                .fold(0.0f32, f32::max);
            (near < distance).then_some(Some((near * scale, distance * scale)))
        };
        let Some(range) = range else { continue };
        out.push(VisualMesh {
            mesh: to_bevy_mesh(mesh),
            material: material.clone(),
            range,
        });
    }
    Ok(out)
}

/// A LOD mesh's name without its `_LOD<n>` suffix (LOD *n* meshes are named `<mesh>_LOD<n>`,
/// `docs/research/lod.md`).
fn lod_base(name: &str) -> &str {
    match name.rfind("_LOD") {
        Some(i) if name[i + 4..].bytes().all(|b| b.is_ascii_digit()) && i + 4 < name.len() => {
            &name[..i]
        }
        _ => name,
    }
}

/// Multiplier on the game's LOD distances: `BG3_LOD_SCALE` (default 1; 0 draws LOD 0 only).
/// The game's `ModelDetail` option probably scales them; not read (`docs/research/lod.md`).
fn lod_scale() -> f32 {
    std::env::var("BG3_LOD_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0)
}

pub fn to_bevy_mesh(m: &bg3_gr2::Mesh) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, crate::pick::mesh_usages());
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

#[cfg(test)]
mod tests {
    use super::lod_base;

    #[test]
    fn lod_suffix_is_stripped() {
        assert_eq!(lod_base("Skirt_Mesh_LOD1"), "Skirt_Mesh");
        assert_eq!(lod_base("Skirt_Mesh_LOD12"), "Skirt_Mesh");
        assert_eq!(lod_base("Skirt_Mesh"), "Skirt_Mesh");
        assert_eq!(lod_base("Skirt_LOD"), "Skirt_LOD");
        assert_eq!(lod_base("Skirt_LODx"), "Skirt_LODx");
    }
}
