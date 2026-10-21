//! Parses every LSF/LSX/LSJ (and LSFX) resource in the main data paks of the user's install.
//! Run with `cargo test -p bg3-lsf --release -- --ignored --nocapture`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

use bg3_install::Install;
use bg3_lsf::{
    LsfWriteOptions, Node, Resource, SourceFormat, Value, parse, parse_lsx, to_lsf, to_lsx,
};
use bg3_pak::Package;

const PAKS: &[&str] = &["Gustav", "GustavX", "Shared", "Engine", "Game"];
const EXTENSIONS: &[&str] = &["lsf", "lsx", "lsj", "lsfx"];

fn install() -> Option<Install> {
    match Install::locate() {
        Ok(i) => Some(i),
        Err(e) => {
            eprintln!("skipping: no BG3 install ({e})");
            None
        }
    }
}

fn open_paks(install: &Install) -> Vec<Package> {
    let paks = install.paks().expect("list paks");
    PAKS.iter()
        .filter_map(|name| {
            let path = paks
                .iter()
                .find(|p| p.file_stem().is_some_and(|s| s == *name))?;
            Some(Package::open(path).expect("open pak"))
        })
        .collect()
}

fn extension(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    EXTENSIONS.iter().copied().find(|e| *e == ext)
}

#[derive(Default)]
struct Stats {
    files: BTreeMap<String, usize>,
    failures: Vec<String>,
    roundtrip_failures: Vec<String>,
    /// LSF: (container version, compression flags, metadata format) -> count.
    lsf_variants: BTreeMap<(u32, u8, u32), usize>,
    engine_versions: BTreeMap<(SourceFormat, String), usize>,
    types: BTreeMap<&'static str, usize>,
    nodes: usize,
    keyed_nodes: usize,
    bytes: u64,
}

fn walk(node: &Node, stats: &mut Stats) {
    stats.nodes += 1;
    stats.keyed_nodes += usize::from(node.key.is_some());
    for a in &node.attributes {
        *stats.types.entry(a.value.ty().name()).or_default() += 1;
    }
    for c in &node.children {
        walk(c, stats);
    }
}

fn format_key(f: SourceFormat) -> &'static str {
    match f {
        SourceFormat::Lsf => "lsf",
        SourceFormat::Lsx => "lsx",
        SourceFormat::Lsj => "lsj",
    }
}

/// Writers must reproduce the tree. Compared via LSX text (exact for floats, and NaN-safe;
/// a few `_merged.lsf` / timeline files contain NaN components).
fn check_roundtrip(res: &Resource) -> Result<(), String> {
    let text = to_lsx(res);
    let lsx = parse_lsx(text.as_bytes()).map_err(|e| format!("lsx reparse: {e}"))?;
    if to_lsx(&lsx) != text {
        return Err("lsx roundtrip differs".into());
    }
    if res.metadata.format == SourceFormat::Lsf {
        let info = res.metadata.lsf.unwrap();
        let opts = LsfWriteOptions {
            version: info.version,
            extended: info.extended,
            ..Default::default()
        };
        let bytes = to_lsf(res, opts).map_err(|e| format!("lsf write: {e}"))?;
        let back = parse(&bytes).map_err(|e| format!("lsf reparse: {e}"))?;
        if to_lsx(&back) != text {
            return Err("lsf roundtrip differs".into());
        }
    }
    Ok(())
}

#[test]
#[ignore = "needs BG3 install"]
fn parse_every_resource_in_install() {
    let Some(install) = install() else { return };
    let paks = open_paks(&install);
    let start = Instant::now();
    let total = Mutex::new(Stats::default());
    for pak in &paks {
        let pak_name = pak
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let entries: Vec<_> = pak
            .entries()
            .iter()
            .filter(|e| extension(&e.name).is_some())
            .collect();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = entries.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            for part in entries.chunks(chunk) {
                let total = &total;
                let pak_name = &pak_name;
                s.spawn(move || {
                    let mut stats = Stats::default();
                    for e in part {
                        let ext = extension(&e.name).unwrap();
                        *stats.files.entry(ext.to_owned()).or_default() += 1;
                        let data = match pak.read(e) {
                            Ok(d) => d,
                            Err(err) => {
                                stats
                                    .failures
                                    .push(format!("{pak_name}:{} read: {err}", e.name));
                                continue;
                            }
                        };
                        stats.bytes += data.len() as u64;
                        let res = match parse(&data) {
                            Ok(r) => r,
                            Err(err) => {
                                stats.failures.push(format!("{pak_name}:{}: {err}", e.name));
                                continue;
                            }
                        };
                        let m = &res.metadata;
                        if let Some(info) = m.lsf {
                            *stats
                                .lsf_variants
                                .entry((info.version, info.compression, info.metadata_format))
                                .or_default() += 1;
                        }
                        *stats
                            .engine_versions
                            .entry((m.format, format!("{}.{}", m.version.major, m.version.minor)))
                            .or_default() += 1;
                        for r in &res.regions {
                            walk(&r.node, &mut stats);
                        }
                        if let Err(err) = check_roundtrip(&res) {
                            stats
                                .roundtrip_failures
                                .push(format!("{pak_name}:{}: {err}", e.name));
                        }
                    }
                    let mut t = total.lock().unwrap();
                    for (k, v) in stats.files {
                        *t.files.entry(k).or_default() += v;
                    }
                    for (k, v) in stats.lsf_variants {
                        *t.lsf_variants.entry(k).or_default() += v;
                    }
                    for (k, v) in stats.engine_versions {
                        *t.engine_versions.entry(k).or_default() += v;
                    }
                    for (k, v) in stats.types {
                        *t.types.entry(k).or_default() += v;
                    }
                    t.failures.extend(stats.failures);
                    t.roundtrip_failures.extend(stats.roundtrip_failures);
                    t.nodes += stats.nodes;
                    t.keyed_nodes += stats.keyed_nodes;
                    t.bytes += stats.bytes;
                });
            }
        });
    }
    let t = total.into_inner().unwrap();
    let elapsed = start.elapsed();
    println!(
        "parsed in {elapsed:.2?} ({:.1} MiB)",
        t.bytes as f64 / 1048576.0
    );
    println!("files by extension: {:?}", t.files);
    println!(
        "LSF (version, compression flags, metadata format): {:?}",
        t.lsf_variants
    );
    for ((format, version), n) in &t.engine_versions {
        println!("  {} engine {version}.x: {n}", format_key(*format));
    }
    println!("nodes: {} (keyed: {})", t.nodes, t.keyed_nodes);
    println!("attribute types: {:?}", t.types);
    for f in t.failures.iter().take(30) {
        println!("FAIL {f}");
    }
    for f in t.roundtrip_failures.iter().take(30) {
        println!("ROUNDTRIP {f}");
    }
    println!(
        "failures: {}  roundtrip failures: {}",
        t.failures.len(),
        t.roundtrip_failures.len()
    );
    assert!(t.files.values().sum::<usize>() > 0, "no resources found");
    assert!(
        t.failures.is_empty(),
        "{} files failed to parse",
        t.failures.len()
    );
    assert!(
        t.roundtrip_failures.is_empty(),
        "{} files failed to roundtrip",
        t.roundtrip_failures.len()
    );
}

fn collect(node: &Node, guids: &mut HashSet<String>, strings: &mut HashSet<String>) {
    for a in &node.attributes {
        match &a.value {
            Value::Uuid(g) => {
                guids.insert(g.to_string());
            }
            Value::TranslatedString(t) => {
                strings.insert(t.handle.clone());
            }
            v => {
                if let Some(s) = v.as_str() {
                    strings.insert(s.to_owned());
                }
            }
        }
    }
    for c in &node.children {
        collect(c, guids, strings);
    }
}

/// Story dialogs ship both as compiled `Story/DialogsBinary/**.lsf` and editor-side
/// `Story/Dialogs/**.lsj`. The trees are not identical (the compiled form is restructured), but
/// every GUID and TranslatedString handle in the LSF should also occur in the LSJ. This pins
/// the LSF GUID byte order and translated-string decoding against independent text.
#[test]
#[ignore = "needs BG3 install"]
fn dialog_lsf_ids_match_lsj() {
    let Some(install) = install() else { return };
    let (mut pairs, mut checked, mut missing) = (0, 0usize, Vec::new());
    for pak in open_paks(&install) {
        for e in pak.entries() {
            let Some(lsj_name) = e
                .name
                .strip_suffix(".lsf")
                .filter(|n| n.contains("/Story/DialogsBinary/"))
                .map(|n| {
                    format!(
                        "{}.lsj",
                        n.replace("/Story/DialogsBinary/", "/Story/Dialogs/")
                    )
                })
            else {
                continue;
            };
            let Some(lsj) = pak.find(&lsj_name) else {
                continue;
            };
            let a = parse(&pak.read(e).unwrap()).unwrap();
            let b = parse(&pak.read(lsj).unwrap()).unwrap();
            pairs += 1;
            let (mut ga, mut sa) = (HashSet::new(), HashSet::new());
            let (mut gb, mut sb) = (HashSet::new(), HashSet::new());
            a.regions
                .iter()
                .for_each(|r| collect(&r.node, &mut ga, &mut sa));
            b.regions
                .iter()
                .for_each(|r| collect(&r.node, &mut gb, &mut sb));
            for g in &ga {
                checked += 1;
                if !gb.contains(g) && !sb.contains(g) {
                    missing.push(format!("{}: guid {g}", e.name));
                }
            }
            for h in sa
                .iter()
                .filter(|s| s.starts_with('h') && s.contains('g') && s.len() == 37)
            {
                checked += 1;
                if !sb.contains(h) {
                    missing.push(format!("{}: handle {h}", e.name));
                }
            }
        }
    }
    println!(
        "dialog pairs: {pairs}, ids checked: {checked}, missing in LSJ: {}",
        missing.len()
    );
    for m in missing.iter().take(10) {
        println!("MISSING {m}");
    }
    assert!(pairs > 0, "no DialogsBinary/Dialogs pairs found");
    assert!(missing.is_empty());
}
