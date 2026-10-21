//! Tests against the real BG3 install. Run with `cargo test -p bg3-vt --release -- --ignored
//! --nocapture`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use bg3_vfs::Vfs;
use bg3_vt::{
    LayerConfig, OutputFormat, PageFile, PageSource, ParameterKind, TileRef,
    VirtualTextureResource, VirtualTextures, material_virtual_textures,
};
use rayon::prelude::*;

fn open_vfs() -> Option<Vfs> {
    match bg3_install::Install::locate() {
        Ok(install) => Some(Vfs::from_install(&install).expect("mount paks")),
        Err(e) => {
            eprintln!("skipping: BG3 install not found ({e})");
            None
        }
    }
}

/// Tiles decoded per page file; override with `BG3_VT_SAMPLES=<n>` (`all` decodes every tile).
fn samples_per_page_file() -> usize {
    match std::env::var("BG3_VT_SAMPLES").as_deref() {
        Ok("all") => usize::MAX,
        Ok(n) => n.parse().expect("BG3_VT_SAMPLES"),
        Err(_) => 8,
    }
}

#[derive(Default)]
struct Stats {
    tile_sets: usize,
    textures: usize,
    page_files: usize,
    flat_tiles: usize,
    fallback_entries: usize,
    empty_entries: usize,
    decoded: usize,
    chunk_codecs: BTreeMap<String, usize>,
    index_kinds: BTreeMap<&'static str, usize>,
    param_kinds: BTreeMap<String, usize>,
    layer_types: BTreeMap<String, usize>,
    geometry: BTreeMap<String, usize>,
}

#[test]
#[ignore = "needs BG3 install"]
fn every_tile_set_parses_and_page_files_decode() {
    let Some(vfs) = open_vfs() else { return };
    let vt = VirtualTextures::new(&vfs);
    let paths = vt.tile_set_paths();
    assert!(!paths.is_empty(), "no .gts in the install");
    let samples = samples_per_page_file();
    let stats = Mutex::new(Stats::default());
    let failures = Mutex::new(Vec::<String>::new());

    paths.par_iter().for_each(|path| {
        let fail = |m: String| failures.lock().unwrap().push(format!("{path}: {m}"));
        let ts = match vt.tile_set(path) {
            Ok(ts) => ts,
            Err(e) => return fail(format!("parse: {e}")),
        };
        let pages = vt.pages(path);

        // GTex naming rule: <Config>_<c>.gts holds exactly the textures whose name starts with c.
        let stem = path.rsplit('/').next().unwrap().trim_end_matches(".gts");
        let suffix = stem.rsplit('_').next().unwrap().to_ascii_lowercase();
        for t in &ts.textures {
            let want = LayerConfig::AlbedoNormalPhysical.tile_set_path(&t.name);
            if want.as_deref().map(str::to_ascii_lowercase) != Some(path.to_ascii_lowercase()) {
                fail(format!("texture {} (suffix {suffix}) breaks the naming rule", t.name));
            }
        }

        // Level index consistency, for tiles covered by some texture (entries outside every
        // texture point at arbitrary placeholder tiles).
        let mut fallback = 0;
        let mut empty = 0;
        let mut index_kinds: BTreeMap<&str, usize> = BTreeMap::new();
        let (cw, ch) = (ts.content_width(), ts.content_height());
        for (li, lv) in ts.levels.iter().enumerate() {
            let mut covered = vec![false; (lv.width * lv.height) as usize];
            for t in &ts.textures {
                let (rx, ry, rw, rh) = ts.texture_rect(t, li);
                for y in ry / ch..=(ry + rh - 1) / ch {
                    for x in rx / cw..=(rx + rw - 1) / cw {
                        covered[(y * lv.width + x) as usize] = true;
                    }
                }
            }
            for y in 0..lv.height {
                for x in 0..lv.width {
                    if !covered[(y * lv.width + x) as usize] {
                        continue;
                    }
                    for layer in 0..ts.layers.len() {
                        let cat = match ts.tile(li, layer, x, y) {
                            Some(TileRef::Stored(f)) => {
                                let p = ts.packed_tiles[ts.flat_tiles[f as usize].packed_tile as usize];
                                if p.layer() != layer as u32 {
                                    fail(format!("level {li} ({x},{y}) layer {layer}: tile of layer {}", p.layer()));
                                }
                                if (p.level(), p.x(), p.y()) == (li as u32, x, y) {
                                    "stored at its own position"
                                } else {
                                    "stored, shared with an identical tile elsewhere"
                                }
                            }
                            Some(TileRef::Fallback(f)) => {
                                fallback += 1;
                                // The referenced tile is the nearest coarser level's stored tile
                                // covering this position.
                                let ancestor = (li + 1..ts.levels.len()).find_map(|l| {
                                    let s = l - li;
                                    match ts.tile(l, layer, x >> s, y >> s) {
                                        Some(TileRef::Stored(g)) => Some((l, g)),
                                        _ => None,
                                    }
                                });
                                match ancestor {
                                    Some((l, g)) if g == f && l == li + 1 => "fallback to the parent tile",
                                    Some((_, g)) if g == f => "fallback to a coarser ancestor tile",
                                    _ => {
                                        fail(format!("level {li} ({x},{y}) layer {layer}: fallback {f} is not the nearest stored ancestor {ancestor:?}"));
                                        "fallback: unexplained"
                                    }
                                }
                            }
                            None => {
                                empty += 1;
                                "empty"
                            }
                        };
                        *index_kinds.entry(cat).or_default() += 1;
                    }
                }
            }
        }

        // Page files: presence, header GUID, page count, chunk tables for every flat tile.
        let mut by_file: HashMap<u16, Vec<u32>> = HashMap::new();
        for (i, f) in ts.flat_tiles.iter().enumerate() {
            by_file.entry(f.page_file).or_default().push(i as u32);
        }
        let mut chunk_codecs: BTreeMap<String, usize> = BTreeMap::new();
        let mut decoded = 0;
        for (pf, info) in ts.page_files.iter().enumerate() {
            let bytes = match pages.page_file(&info.name) {
                Ok(b) => b,
                Err(e) => {
                    fail(format!("page file {}: {e}", info.name));
                    continue;
                }
            };
            let gtp = match PageFile::parse(&bytes, ts.page_size) {
                Ok(g) => g,
                Err(e) => {
                    fail(format!("{}: {e}", info.name));
                    continue;
                }
            };
            if gtp.guid != info.guid || gtp.num_pages() != info.num_pages as usize {
                fail(format!("{}: GUID or page count mismatch", info.name));
            }
            let tiles = by_file.get(&(pf as u16)).map(Vec::as_slice).unwrap_or(&[]);
            let mut tables: HashMap<u16, usize> = HashMap::new();
            for &t in tiles {
                let f = ts.flat_tiles[t as usize];
                let n = *tables.entry(f.page).or_insert_with(|| {
                    gtp.chunk_offsets(f.page as usize).map(|o| o.len()).unwrap_or(0)
                });
                if f.chunk as usize >= n {
                    fail(format!("{}: flat tile {t} → missing chunk {}:{}", info.name, f.page, f.chunk));
                }
            }
            // Decode an evenly spread sample.
            let step = (tiles.len() / samples).max(1);
            for &t in tiles.iter().step_by(step).take(samples) {
                let f = ts.flat_tiles[t as usize];
                let res = gtp
                    .chunk(f.page as usize, f.chunk as usize)
                    .and_then(|c| {
                        *chunk_codecs.entry(format!("{:?}", c.codec)).or_default() += 1;
                        ts.decode_chunk(&c)
                    });
                match res {
                    Ok(_) => decoded += 1,
                    Err(e) => fail(format!("{}: flat tile {t}: {e}", info.name)),
                }
            }
        }

        let mut s = stats.lock().unwrap();
        s.tile_sets += 1;
        s.textures += ts.textures.len();
        s.page_files += ts.page_files.len();
        s.flat_tiles += ts.flat_tiles.len();
        s.fallback_entries += fallback;
        s.empty_entries += empty;
        s.decoded += decoded;
        for (k, v) in index_kinds {
            *s.index_kinds.entry(k).or_default() += v;
        }
        for (k, v) in chunk_codecs {
            *s.chunk_codecs.entry(k).or_default() += v;
        }
        for p in &ts.parameter_blocks {
            let k = match &p.kind {
                ParameterKind::Bc(b) => format!(
                    "BC {:?} {}/{} {:?} mip={}",
                    b.format, b.compression, b.compressor, b.data_type, b.embedded_mip
                ),
                ParameterKind::Uniform { data_type, .. } => format!("Uniform {data_type:?}"),
                ParameterKind::Other(_) => format!("other {:?}", p.codec),
            };
            *s.param_kinds.entry(k).or_default() += 1;
        }
        *s.layer_types
            .entry(format!("{:?}", ts.layers.iter().map(|l| l.data_type).collect::<Vec<_>>()))
            .or_default() += 1;
        *s.geometry
            .entry(format!(
                "tile {}x{} border {} page {:#x} levels {} ({}x{} tiles)",
                ts.tile_width,
                ts.tile_height,
                ts.tile_border,
                ts.page_size,
                ts.levels.len(),
                ts.levels[0].width,
                ts.levels[0].height
            ))
            .or_default() += 1;
    });

    let s = stats.into_inner().unwrap();
    eprintln!(
        "{} tile sets, {} textures, {} page files, {} flat tiles, {} fallback / {} empty index entries, {} tiles decoded",
        s.tile_sets,
        s.textures,
        s.page_files,
        s.flat_tiles,
        s.fallback_entries,
        s.empty_entries,
        s.decoded
    );
    eprintln!("level index entries inside textures: {:#?}", s.index_kinds);
    eprintln!("chunk codecs (sampled): {:?}", s.chunk_codecs);
    eprintln!("parameter blocks: {:#?}", s.param_kinds);
    eprintln!("layers: {:?}", s.layer_types);
    eprintln!("geometry: {:?}", s.geometry);
    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} failures, e.g. {:#?}",
        failures.len(),
        &failures[..failures.len().min(10)]
    );
    assert_eq!(s.tile_sets, paths.len());
    assert!(s.decoded > 0);
}

/// Every `VirtualTextureBank` resource and every material's virtual-texture parameters.
fn scan_banks(
    vfs: &Vfs,
) -> (
    HashMap<String, VirtualTextureResource>,
    HashMap<String, Vec<String>>,
) {
    let mut paths: Vec<String> = Vec::new();
    for module in bg3_vfs::modules::BASE_LOAD_ORDER {
        let mut p: Vec<String> = vfs
            .files_under(&format!("Public/{module}/"))
            .map(|f| f.entry.name.clone())
            .filter(|n| n.ends_with("_merged.lsf") && !n.contains("/RootTemplates/"))
            .collect();
        p.sort_by_key(|p| p.to_ascii_lowercase());
        paths.extend(p);
    }
    let parsed: Vec<_> = paths
        .par_iter()
        .filter_map(|p| {
            let res = bg3_lsf::parse(&vfs.read(p).ok()?).ok()?;
            let vts = VirtualTextureResource::read_bank(&res);
            let mats: Vec<(String, Vec<String>)> = res
                .region("MaterialBank")
                .into_iter()
                .flat_map(|n| n.children_named("Resource"))
                .filter_map(|m| {
                    let ids: Vec<String> = material_virtual_textures(m)
                        .into_iter()
                        .filter(|p| p.enabled)
                        .map(|p| p.id)
                        .collect();
                    Some((m.attr("ID")?.to_string(), ids)).filter(|(_, ids)| !ids.is_empty())
                })
                .collect();
            Some((vts, mats))
        })
        .collect();
    let mut vts = HashMap::new();
    let mut mats = HashMap::new();
    for (v, m) in parsed {
        vts.extend(v.into_iter().map(|r| (r.id.clone(), r)));
        mats.extend(m);
    }
    (vts, mats)
}

#[test]
#[ignore = "needs BG3 install"]
fn nautiloid_virtual_textures_extract() {
    let Some(vfs) = open_vfs() else { return };
    let level = bg3_world::Level::load(&vfs, "TUT_Avernus_C").unwrap();
    let templates = bg3_world::RootTemplates::load(&vfs).unwrap();
    let banks = bg3_world::ResourceBanks::load(&vfs).unwrap();
    let (vt_banks, material_vts) = scan_banks(&vfs);
    eprintln!(
        "{} VirtualTextureBank resources, {} materials with virtual textures",
        vt_banks.len(),
        material_vts.len()
    );
    let configs: HashSet<u32> = vt_banks.values().map(|r| r.layer_config).collect();
    eprintln!("layer configs in banks: {configs:?}");

    // Materials of every Nautiloid object's visual.
    let mut materials = HashSet::new();
    for o in &level.objects {
        let visual = o
            .visual_template
            .as_deref()
            .or_else(|| templates.visual_for(o.template.as_deref()?));
        if let Some(v) = visual.and_then(|v| banks.visuals.get(v)) {
            materials.extend(v.objects.iter().filter_map(|m| m.material_id.clone()));
        }
    }
    let used: HashSet<&String> = materials
        .iter()
        .filter_map(|m| material_vts.get(m))
        .flatten()
        .collect();
    eprintln!(
        "Nautiloid: {} materials, {} distinct virtual textures",
        materials.len(),
        used.len()
    );
    assert!(!used.is_empty());

    let vt = VirtualTextures::new(&vfs);
    let failures: Vec<String> = used
        .par_iter()
        .flat_map(|id| {
            let Some(res) = vt_banks.get(*id) else {
                return vec![format!("{id}: not in any VirtualTextureBank")];
            };
            (0..3)
                .filter_map(|layer| {
                    vt.extract(res, layer, 0, OutputFormat::Bc).err().map(|e| {
                        format!("{} ({}) layer {layer}: {e}", res.name, res.gtex_file_name)
                    })
                })
                .collect()
        })
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");

    // Finest stored level per layer, and RGBA upsampling where a layer isn't stored at level 0.
    let mut finest: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut upsampled = None;
    for id in &used {
        let res = &vt_banks[*id];
        let loc = vt.locate(res).unwrap();
        for layer in 0..3 {
            let l = loc
                .tile_set
                .finest_stored_level(&loc.texture, layer)
                .unwrap();
            *finest.entry((layer, l)).or_default() += 1;
            if l > 0 && upsampled.is_none() {
                let img = loc
                    .tile_set
                    .extract(
                        &vt.pages(&loc.tile_set_path),
                        &loc.texture,
                        layer,
                        0,
                        OutputFormat::Rgba8,
                    )
                    .unwrap();
                assert_eq!(
                    (img.width, img.height),
                    (loc.texture.width, loc.texture.height)
                );
                upsampled = Some((res.name.clone(), layer, l));
            }
        }
    }
    eprintln!("(layer, finest stored level) → textures: {finest:?}; upsampled {upsampled:?}");

    // The example from the docs: WALL_MF_Nautiloid_Interior_Ground_A (ENV_DetailMap_VT).
    let ground = vt_banks
        .values()
        .find(|r| r.name == "WALL_MF_Nautiloid_Interior_Ground_A")
        .expect("ground VT resource");
    let loc = vt.locate(ground).unwrap();
    assert_eq!(loc.tile_set.layer_names(), ["Albedo", "Normal", "Physical"]);
    let mips = vt.extract_mips(ground, 0, 0, OutputFormat::Bc).unwrap();
    eprintln!(
        "Ground_A: {:?} → {} mips, top {}x{}",
        loc.texture,
        mips.len(),
        mips[0].width,
        mips[0].height
    );
    assert_eq!(
        (mips[0].width, mips[0].height),
        (loc.texture.width, loc.texture.height)
    );
    for w in mips.windows(2) {
        assert_eq!(w[1].width, w[0].width / 2);
    }
    // Real content: many distinct blocks.
    let distinct: HashSet<&[u8]> = mips[0].data.chunks(16).collect();
    assert!(distinct.len() > 1000, "{} distinct blocks", distinct.len());
}
