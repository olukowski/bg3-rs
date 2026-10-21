//! Determine the normal-map Y convention of a level's plain-textured materials, and whether
//! the viewer's tangent frames need Bevy's `flip_normal_map_y`.
//!
//! 1. **Texture side** (integrability): a normal map baked from a height field `h` has
//!    `x ∝ -∂h/∂u` and `y ∝ -∂h/∂v'`, where `v'` is the image direction green points to. Then
//!    `∂x/∂v' = ∂y/∂u`. Correlating `∂x/∂row` with `∂y/∂col` in pixel space gives the sign:
//!    positive ⇒ green points down the image (+v, "DirectX"), negative ⇒ up (−v, "OpenGL").
//! 2. **Mesh side**: in Bevy space (Z mirrored, tangent `w` negated as in `viewer/src/space.rs`),
//!    the bitangent `cross(N, T)·w` is compared with the triangle's `∂P/∂v`.
//!
//! Bevy computes `N' = x·T + y·B + z·N` with `y` = green. So no flip is needed exactly when
//! green and `B` point the same way along v.
//!
//! Run: `cargo run --release -p bg3x --example normal_convention [LEVEL]`.

use std::collections::HashSet;

use anyhow::Result;
use bg3_install::Install;
use bg3_material::{ShaderGraph, Slot, dds::DdsHeader, resolve};
use bg3_vfs::Vfs;
use bg3_world::{Level, ResourceBanks, RootTemplates};

fn main() -> Result<()> {
    let level_name = std::env::args().nth(1).unwrap_or("TUT_Avernus_C".into());
    let vfs = Vfs::from_install(&Install::locate()?)?;
    let level = Level::load(&vfs, &level_name)?;
    let templates = RootTemplates::load(&vfs)?;
    let banks = ResourceBanks::load(&vfs)?;

    let mut seen = HashSet::new();
    for obj in &level.objects {
        let Some(visual) = obj
            .visual_template
            .as_deref()
            .or_else(|| templates.visual_for(obj.template.as_deref()?))
            .and_then(|v| banks.visuals.get(v))
        else {
            continue;
        };
        for o in visual.objects.iter().filter(|o| o.lod == 0) {
            let Some(mat) = o
                .material_id
                .as_deref()
                .and_then(|m| banks.materials.get(m))
            else {
                continue;
            };
            let key = (visual.id.clone(), mat.id.clone());
            if !seen.insert(key) {
                continue;
            }
            let src = mat.source_file.as_deref().unwrap_or_default();
            let graph = vfs
                .read(src)
                .ok()
                .and_then(|d| bg3_lsf::parse(&d).ok())
                .and_then(|r| ShaderGraph::from_resource(&r));
            let r = resolve(src, graph.as_ref(), &mat.params);
            if r.needs_virtual_texture() {
                continue;
            }
            let Some(nm) = r
                .texture(Slot::Normal)
                .and_then(|t| banks.textures.get(&t.id))
                .and_then(|t| t.source_file.as_deref())
            else {
                continue;
            };
            let tex = texture_side(&vfs, nm);
            let mesh = visual
                .source_file
                .as_deref()
                .and_then(|p| mesh_side(&vfs, p, o.mesh_name()));
            println!(
                "{:45} green·v corr {:>7}  mesh T·∂P/∂u, B·∂P/∂v {:>16}",
                mat.name,
                tex.map_or("?".into(), |c| format!("{c:+.3}")),
                mesh.map_or("?".into(), |(t, b)| format!("{t:+.3}, {b:+.3}")),
            );
        }
    }
    Ok(())
}

/// Normalised correlation of ∂x/∂row with ∂y/∂col (see module docs).
fn texture_side(vfs: &Vfs, path: &str) -> Option<f64> {
    let data = vfs.read(path).ok()?;
    let h = DdsHeader::parse(&data)?;
    let (w, ht, range) = h.mip_levels()?.into_iter().find(|l| l.0 <= 512)?;
    let px = bg3_material::bc::decode_level(h.format, w, ht, data.get(range)?)?;
    let (w, ht) = (w as usize, ht as usize);
    let at = |x: usize, y: usize, c: usize| px[(y * w + x) * 4 + c] as f64 / 255.0 * 2.0 - 1.0;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for y in 0..ht - 1 {
        for x in 0..w - 1 {
            // X from alpha, Y from green (BG3 layout).
            let dx_drow = at(x, y + 1, 3) - at(x, y, 3);
            let dy_dcol = at(x + 1, y, 1) - at(x, y, 1);
            sxy += dx_drow * dy_dcol;
            sxx += dx_drow * dx_drow;
            syy += dy_dcol * dy_dcol;
        }
    }
    Some(sxy / (sxx * syy).sqrt().max(1e-12))
}

/// Means of sign(T·∂P/∂u) and sign(B·∂P/∂v) over triangles, in Bevy space.
fn mesh_side(vfs: &Vfs, gr2: &str, mesh: &str) -> Option<(f64, f64)> {
    let scene = bg3_gr2::Scene::from_bytes(&vfs.read(gr2).ok()?).ok()?;
    let m = scene.meshes.iter().find(|m| m.name == mesh)?;
    let uv = m.uvs.first()?;
    if m.tangents.len() != m.positions.len() || m.normals.len() != m.positions.len() {
        return None;
    }
    // viewer/src/space.rs: positions/normals (x, y, -z); tangents (x, y, -z, -w).
    let p = |i: usize| {
        let [x, y, z] = m.positions[i];
        [x as f64, y as f64, -z as f64]
    };
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let (mut sum, mut sum_t, mut n) = (0.0, 0.0, 0.0);
    for tri in m.indices.as_chunks::<3>().0 {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (e1, e2) = (sub(p(b), p(a)), sub(p(c), p(a)));
        let (du1, dv1) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1]);
        let (du2, dv2) = (uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
        let det = (du1 * dv2 - du2 * dv1) as f64;
        if det.abs() < 1e-12 {
            continue;
        }
        // ∂P/∂v = (du1·e2 − du2·e1) / det
        let dpdv: [f64; 3] =
            std::array::from_fn(|k| (du1 as f64 * e2[k] - du2 as f64 * e1[k]) / det);
        let [nx, ny, nz] = m.normals[a];
        let normal = [nx as f64, ny as f64, -nz as f64];
        let [tx, ty, tz, tw] = m.tangents[a];
        let tangent = [tx as f64, ty as f64, -tz as f64];
        let bitangent = cross(normal, tangent).map(|x| x * -tw as f64);
        // ∂P/∂u = (dv2·e1 − dv1·e2) / det
        let dpdu: [f64; 3] =
            std::array::from_fn(|k| (dv2 as f64 * e1[k] - dv1 as f64 * e2[k]) / det);
        sum += dot(bitangent, dpdv).signum();
        sum_t += dot(tangent, dpdu).signum();
        n += 1.0;
    }
    (n > 0.0).then(|| (sum_t / n, sum / n))
}
