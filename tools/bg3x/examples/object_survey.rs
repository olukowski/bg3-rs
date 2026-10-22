//! Survey every LOD0 mesh of a level's objects: object, template, visual, GR2 mesh flags,
//! material and shader graph, world-space extent, and whether the game draws it (scenery
//! blockers get no visual; proxy, occluder and cloth simulation meshes aren't drawn). Used to
//! find the helper geometry the viewer drew by mistake (`docs/research/level-structure.md`).
//!
//! Run: `cargo run --release -p bg3x --example object_survey [LEVEL] > .cache/objects.tsv`
//! (game data: keep the output local).

use std::collections::HashMap;

use anyhow::Result;
use bg3_install::Install;
use bg3_vfs::Vfs;
use bg3_world::{Level, ResourceBanks, RootTemplates};

fn main() -> Result<()> {
    let level_name = std::env::args().nth(1).unwrap_or("TUT_Avernus_C".into());
    let vfs = Vfs::from_install(&Install::locate()?)?;
    let level = Level::load(&vfs, &level_name)?;
    let templates = RootTemplates::load(&vfs)?;
    let banks = ResourceBanks::load(&vfs)?;
    let mut scenes: HashMap<String, Option<bg3_gr2::Scene>> = HashMap::new();
    println!(
        "kind\tname\ttemplate\tvisual\tgr2\tmesh\tflags\tmaterial\tgraph\textent_world\tpos\tdrawn"
    );
    for obj in &level.objects {
        let tname = obj
            .template
            .as_deref()
            .and_then(|t| templates.get(t))
            .map_or("", |t| t.name.as_str());
        let Some(visual) = obj
            .visual_template
            .as_deref()
            .or_else(|| templates.visual_for(obj.template.as_deref()?))
            .and_then(|v| banks.visuals.get(v))
        else {
            continue;
        };
        let Some(path) = visual.source_file.as_deref() else {
            continue;
        };
        let scene = scenes
            .entry(path.to_owned())
            .or_insert_with(|| bg3_gr2::Scene::from_bytes(&vfs.read(path).ok()?).ok());
        let Some(scene) = scene else { continue };
        for o in visual.objects.iter().filter(|o| o.lod == 0) {
            let Some(mesh) = scene.meshes.iter().find(|m| m.name == o.mesh_name()) else {
                continue;
            };
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for p in &mesh.positions {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
            let s = obj.transform.scale;
            let ext: Vec<String> = (0..3)
                .map(|i| format!("{:.1}", (hi[i] - lo[i]) * s))
                .collect();
            let mat = o
                .material_id
                .as_deref()
                .and_then(|m| banks.materials.get(m));
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{}\t{}\t{}\t{:?}\t{}",
                obj.kind,
                obj.name,
                tname,
                visual.name,
                path.rsplit('/').next().unwrap_or(path),
                mesh.name,
                mesh.properties.as_ref().map(|p| p.flags),
                mat.map_or("", |m| m.name.as_str()),
                mat.and_then(|m| m.source_file.as_deref())
                    .and_then(|p| p.rsplit('/').next())
                    .unwrap_or(""),
                ext.join("x"),
                obj.transform.position,
                obj.draws_visual(&templates) && mesh.is_drawn(),
            );
        }
    }
    Ok(())
}
