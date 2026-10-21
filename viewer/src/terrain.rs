//! Terrain rendering: one mesh per 64×64-cell patch, coloured by blending a flat placeholder
//! colour per terrain brush with the per-cell layer weights. Holes are left out.

use std::collections::HashMap;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bg3_vfs::Vfs;
use bg3_world::terrain::{PATCH_CELLS, load_level_terrains, load_terrain_brushes};
use bg3_world::{Terrain, TerrainBrush};

use crate::space;

#[derive(Resource)]
pub struct LevelTerrain {
    pub terrains: Vec<Terrain>,
    pub brushes: HashMap<String, TerrainBrush>,
}

impl LevelTerrain {
    /// Terrains of `level` plus any extra (sub-)levels.
    pub fn load(vfs: &Vfs, levels: &[String]) -> Result<Self> {
        let started = std::time::Instant::now();
        let mut terrains = Vec::new();
        for level in levels {
            terrains.extend(load_level_terrains(vfs, level)?);
        }
        let brushes = load_terrain_brushes(vfs)?;
        let cells: usize = terrains.iter().map(|t| t.cells[0] * t.cells[1]).sum();
        eprintln!(
            "loaded {} terrains ({cells} cells, {} brushes) from {levels:?} in {:.2?}",
            terrains.len(),
            brushes.len(),
            started.elapsed()
        );
        for t in &terrains {
            for (path, err) in &t.errors {
                eprintln!("  terrain {}: {path}: {err}", t.name);
            }
        }
        let out = Self { terrains, brushes };
        if let Some((lo, hi)) = out.bounds() {
            eprintln!("terrain bounds (Bevy space): {lo} .. {hi}");
        }
        Ok(out)
    }

    /// World-space (Bevy) bounds of all terrains, ignoring rotation.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut out: Option<(Vec3, Vec3)> = None;
        for t in &self.terrains {
            let Some((lo, hi)) = t.height_range() else {
                continue;
            };
            let [x, y, z] = t.transform.position;
            let s = t.transform.scale;
            for corner in [
                [x, y + lo * s, z],
                [
                    x + t.cells[0] as f32 * s,
                    y + hi * s,
                    z + t.cells[1] as f32 * s,
                ],
            ] {
                let p = Vec3::from_array(space::vec3(corner));
                out = Some(match out {
                    None => (p, p),
                    Some((a, b)) => (a.min(p), b.max(p)),
                });
            }
        }
        out
    }
}

/// Placeholder linear colour for a brush, from keywords in its name (textures are not loaded yet).
fn brush_color(brush: Option<&TerrainBrush>) -> [f32; 3] {
    let Some(b) = brush else {
        return [0.5, 0.5, 0.5];
    };
    if b.is_black_brush {
        return [0.02, 0.02, 0.02];
    }
    let name = b.name.to_ascii_lowercase();
    let palette: &[(&str, [f32; 3])] = &[
        ("flesh", [0.45, 0.12, 0.12]),
        ("membrane", [0.35, 0.10, 0.18]),
        ("nautiloid", [0.30, 0.18, 0.20]),
        ("black", [0.02, 0.02, 0.02]),
        ("grass", [0.18, 0.30, 0.08]),
        ("moss", [0.14, 0.26, 0.08]),
        ("needles", [0.30, 0.20, 0.10]),
        ("sand", [0.55, 0.47, 0.32]),
        ("ash", [0.22, 0.21, 0.20]),
        ("mud", [0.22, 0.16, 0.10]),
        ("soil", [0.25, 0.18, 0.11]),
        ("dirt", [0.32, 0.24, 0.15]),
        ("gravel", [0.35, 0.32, 0.28]),
        ("cliff", [0.45, 0.38, 0.30]),
        ("rock", [0.38, 0.36, 0.33]),
        ("paving", [0.45, 0.44, 0.42]),
        ("marble", [0.60, 0.58, 0.55]),
        ("stone", [0.42, 0.41, 0.39]),
        ("roots", [0.25, 0.17, 0.10]),
        ("snow", [0.85, 0.87, 0.90]),
        ("water", [0.10, 0.18, 0.22]),
    ];
    let base = palette
        .iter()
        .find(|(k, _)| name.contains(k))
        .map_or([0.4, 0.4, 0.4], |(_, c)| *c);
    // Small stable variation so neighbouring brushes of the same family stay distinguishable.
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    b.id.hash(&mut h);
    let v = 0.85 + (h.finish() % 1000) as f32 / 1000.0 * 0.3;
    base.map(|c| (c * v).min(1.0))
}

/// Mesh for one patch-sized tile of a terrain, in the terrain's local space (Bevy convention).
fn tile_mesh(t: &Terrain, colors: &[[f32; 3]], tile: [usize; 2]) -> Option<Mesh> {
    let [cw, ch] = t.cells;
    let x0 = tile[0] * PATCH_CELLS;
    let z0 = tile[1] * PATCH_CELLS;
    let x1 = (x0 + PATCH_CELLS).min(cw);
    let z1 = (z0 + PATCH_CELLS).min(ch);
    let vw = x1 - x0 + 1;
    let vertex = |x: usize, z: usize| ((z - z0) * vw + (x - x0)) as u32;

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut vcolors = Vec::new();
    for z in z0..=z1 {
        for x in x0..=x1 {
            let h = t.height(x, z);
            positions.push(space::vec3([x as f32, h, z as f32]));
            // Central differences in BG3 local space, then mirrored like any other direction.
            let hx = |x: usize| t.height(x, z);
            let hz = |z: usize| t.height(x, z);
            let dx = (hx((x + 1).min(cw)) - hx(x.saturating_sub(1)))
                / ((x + 1).min(cw) - x.saturating_sub(1)) as f32;
            let dz = (hz((z + 1).min(ch)) - hz(z.saturating_sub(1)))
                / ((z + 1).min(ch) - z.saturating_sub(1)) as f32;
            normals.push(space::vec3(Vec3::new(-dx, 1.0, -dz).normalize().to_array()));
            vcolors.push(vertex_color(t, colors, x, z));
        }
    }
    let mut indices = Vec::new();
    for z in z0..z1 {
        for x in x0..x1 {
            if t.is_hole(x, z) {
                continue;
            }
            let (a, b, c, d) = (
                vertex(x, z),
                vertex(x, z + 1),
                vertex(x + 1, z),
                vertex(x + 1, z + 1),
            );
            // Counter-clockwise seen from +Y in BG3's coordinates (like GR2 index data), so
            // `flip_winding` below applies to terrain the same way as to meshes.
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    if indices.is_empty() {
        return None;
    }
    space::flip_winding(&mut indices);
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vcolors);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

/// Blend brush colours by the layer weights of the cells touching vertex `(x, z)`.
fn vertex_color(t: &Terrain, colors: &[[f32; 3]], x: usize, z: usize) -> [f32; 4] {
    let [cw, ch] = t.cells;
    let mut acc = [0.0f32; 3];
    let mut total = 0.0f32;
    for (cx, cz) in [
        (x.wrapping_sub(1), z.wrapping_sub(1)),
        (x, z.wrapping_sub(1)),
        (x.wrapping_sub(1), z),
        (x, z),
    ] {
        if cx >= cw || cz >= ch {
            continue;
        }
        for (layer, c) in t.weights.iter().zip(colors) {
            let w = layer[cz * cw + cx] as f32;
            for i in 0..3 {
                acc[i] += c[i] * w;
            }
            total += w;
        }
    }
    if total <= 0.0 {
        let c = colors.first().copied().unwrap_or([0.4; 3]);
        return [c[0], c[1], c[2], 1.0];
    }
    [acc[0] / total, acc[1] / total, acc[2] / total, 1.0]
}

pub fn spawn_terrain(
    mut commands: Commands,
    terrain: Res<LevelTerrain>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.9,
        ..default()
    });
    let mut tiles = 0;
    for t in &terrain.terrains {
        let colors: Vec<[f32; 3]> = t
            .layers
            .iter()
            .map(|l| brush_color(terrain.brushes.get(&l.brush)))
            .collect();
        let parent = commands
            .spawn((
                Name::new(format!("terrain {}", t.name)),
                space::transform(&t.transform),
                Visibility::default(),
            ))
            .id();
        for tz in 0..t.cells[1].div_ceil(PATCH_CELLS) {
            for tx in 0..t.cells[0].div_ceil(PATCH_CELLS) {
                let Some(mesh) = tile_mesh(t, &colors, [tx, tz]) else {
                    continue;
                };
                tiles += 1;
                commands.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material.clone()),
                    ChildOf(parent),
                ));
            }
        }
    }
    eprintln!(
        "spawned {} terrains as {tiles} tile meshes",
        terrain.terrains.len()
    );
}

/// `--terrain-only`: hide every level object so only terrain (and lights) remain visible.
pub fn hide_objects(
    mut commands: Commands,
    roots: Query<(Entity, &Name, Has<ChildOf>, Has<Camera>)>,
) {
    for (e, name, _, _) in roots.iter().filter(|(_, _, child, cam)| !child && !cam) {
        let keep = ["terrain ", "light ", "sun"]
            .iter()
            .any(|p| name.as_str().starts_with(p));
        if !keep {
            commands.entity(e).insert(Visibility::Hidden);
        }
    }
}
