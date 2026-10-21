//! `bg3x materials`: survey the materials a level's LOD0 meshes use.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use bg3_install::Install;
use bg3_material::dds::DdsHeader;
use bg3_material::{ShaderGraph, Slot, resolve};
use bg3_vfs::Vfs;
use bg3_world::{Level, ResourceBanks, RootTemplates};

/// Format plus per-channel mean and range of a small mip, to infer channel layouts.
fn channel_stats(vfs: &Vfs, path: &str) -> Option<String> {
    let data = vfs.read(path).ok()?;
    let h = DdsHeader::parse(&data)?;
    let levels = h.mip_levels()?;
    let (w, ht, range) = levels
        .iter()
        .find(|(w, _, _)| *w <= 256)
        .or(levels.last())?
        .clone();
    let px = bg3_material::bc::decode_level(h.format, w, ht, data.get(range)?)?;
    Some(format!(
        "{:?} {}x{}:{}",
        h.format,
        h.width,
        h.height,
        rgba_stats(&px)
    ))
}

/// Per-channel mean and range of RGBA8 pixels.
fn rgba_stats(px: &[u8]) -> String {
    let n = (px.len() / 4).max(1) as f64;
    let mut s = String::new();
    for (c, name) in "RGBA".chars().enumerate() {
        let vals = px.iter().skip(c).step_by(4);
        let mean = vals.clone().map(|&v| v as f64).sum::<f64>() / n / 255.0;
        let (lo, hi) = vals.fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        s += &format!(" {name}={mean:.2}[{lo}..{hi}]");
    }
    s
}

pub fn survey(level_name: &str, verbose: bool) -> Result<()> {
    let vfs = Vfs::from_install(&Install::locate()?)?;
    let level = Level::load(&vfs, level_name)?;
    let templates = RootTemplates::load(&vfs)?;
    let banks = ResourceBanks::load(&vfs)?;
    let vts = bg3_vt::VirtualTextures::new(&vfs);

    // Materials of every LOD0 mesh of every object's visual.
    let mut uses: BTreeMap<&str, usize> = BTreeMap::new();
    let mut example: HashMap<&str, [f32; 3]> = HashMap::new();
    for obj in &level.objects {
        let visual = obj
            .visual_template
            .as_deref()
            .or_else(|| templates.visual_for(obj.template.as_deref()?));
        let Some(visual) = visual.and_then(|v| banks.visuals.get(v)) else {
            continue;
        };
        for m in visual.objects.iter().filter(|o| o.lod == 0) {
            if let Some(id) = m.material_id.as_deref() {
                *uses.entry(id).or_default() += 1;
                example.entry(id).or_insert(obj.transform.position);
            }
        }
    }

    let mut graphs: HashMap<String, Option<ShaderGraph>> = HashMap::new();
    let mut by_graph: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut slot_names: BTreeMap<(Slot, String), usize> = BTreeMap::new();
    let mut texture_ids: HashSet<String> = HashSet::new();
    let (mut missing, mut vt, mut plain, mut no_maps) = (0, 0, 0, 0);
    let mut blends: BTreeMap<String, usize> = BTreeMap::new();
    for (&id, &n) in &uses {
        let Some(mat) = banks.materials.get(id) else {
            missing += 1;
            if verbose {
                println!("{id} ({n}×, e.g. at {:?}): not in any bank", example[id]);
            }
            continue;
        };
        let source = mat.source_file.clone().unwrap_or_default();
        let graph = graphs
            .entry(source.clone())
            .or_insert_with(|| {
                let data = vfs.read(&source).ok()?;
                ShaderGraph::from_resource(&bg3_lsf::parse(&data).ok()?)
            })
            .as_ref();
        let r = resolve(&source, graph, &mat.params);
        let e = by_graph.entry(source.clone()).or_default();
        e.0 += 1;
        e.1 += n;
        *blends
            .entry(format!(
                "{:?} blend={} raster={} discard={}",
                r.blend,
                graph.map_or(255, |g| g.blend_state),
                graph.map_or(255, |g| g.rasterizer_state),
                graph.is_some_and(|g| g.discards_pixels)
            ))
            .or_default() += 1;
        if r.needs_virtual_texture() {
            vt += 1;
        } else if r.texture(Slot::BaseColor).is_some() {
            plain += 1;
        } else {
            no_maps += 1;
        }
        for t in &r.textures {
            let tex = banks.textures.get(&t.id);
            let tname = tex.map_or("?", |t| t.name.as_str());
            let suffix = tname.rsplit('_').next().unwrap_or("");
            *slot_names
                .entry((t.slot, format!("{} ← *_{suffix}", t.param)))
                .or_default() += 1;
            texture_ids.insert(t.id.clone());
        }
        if verbose {
            println!(
                "{} ({}×, e.g. at {:?}) {source} {:?} vt={}",
                mat.name,
                n,
                example[id],
                r.blend,
                r.virtual_textures.len()
            );
            for t in &r.textures {
                let tex = banks.textures.get(&t.id);
                let stats = tex
                    .and_then(|t| t.source_file.as_deref())
                    .and_then(|p| channel_stats(&vfs, p))
                    .unwrap_or_default();
                println!(
                    "    {:?} {} = {} srgb={:?} {stats}",
                    t.slot,
                    t.param,
                    tex.map_or("?", |t| t.name.as_str()),
                    tex.map(|t| t.srgb)
                );
            }
            for id in &r.virtual_textures {
                let Some(res) = banks.virtual_textures.get(id) else {
                    println!("    VT {id}: not in banks");
                    continue;
                };
                for layer in 0..3 {
                    // A coarse level is enough for channel statistics.
                    let stats = vts
                        .extract(res, layer, 4, bg3_vt::OutputFormat::Rgba8)
                        .map(|img| rgba_stats(&img.data))
                        .unwrap_or_else(|e| e.to_string());
                    println!("    VT {} layer {layer}: {stats}", res.name);
                }
            }
            for (k, v) in &r.scalars {
                println!("    {k} = {v}");
            }
            for (k, v) in &r.vectors {
                println!("    {k} = {v:?}");
            }
        }
    }

    println!(
        "{} materials used by LOD0 meshes ({missing} not in banks): {plain} with a plain base colour map, {vt} virtual-textured, {no_maps} neither",
        uses.len()
    );
    println!("\nshader graphs (materials, mesh uses):");
    let mut sorted: Vec<_> = by_graph.into_iter().collect();
    sorted.sort_by_key(|(_, (m, _))| std::cmp::Reverse(*m));
    for (g, (m, n)) in sorted {
        println!("  {m:4} {n:5}  {g}");
    }
    println!("\nblend states:");
    for (k, v) in blends {
        println!("  {v:4}  {k}");
    }
    println!("\ntexture parameters (slot, param ← texture name suffix):");
    for ((slot, name), n) in slot_names {
        println!("  {n:4}  {slot:?} {name}");
    }

    // DDS formats of every referenced texture.
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    for id in &texture_ids {
        let Some(path) = banks
            .textures
            .get(id)
            .and_then(|t| t.source_file.as_deref())
        else {
            *formats.entry("no texture resource".into()).or_default() += 1;
            continue;
        };
        let key = match vfs.read(path) {
            Err(_) => "missing file".into(),
            Ok(data) => match DdsHeader::parse(&data) {
                None => "not DDS".into(),
                Some(h) => format!(
                    "{:?} dx10={} srgb={} cube={} mips>1={} srgb-flag={}",
                    h.format,
                    h.dx10,
                    h.srgb,
                    h.cubemap,
                    h.mip_count > 1,
                    banks.textures[id].srgb
                ),
            },
        };
        *formats.entry(key).or_default() += 1;
    }
    println!("\n{} distinct textures; DDS formats:", texture_ids.len());
    for (k, v) in formats {
        println!("  {v:4}  {k}");
    }
    Ok(())
}
