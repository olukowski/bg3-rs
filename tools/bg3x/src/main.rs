//! `bg3x` — inspect a Baldur's Gate 3 install. Output stays local; see ADR 0001.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use bg3_install::Install;
use bg3_pak::Package;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the located install, its version, and its paks.
    Info,
    /// List files in a pak. PAK is a path or a pak name in the install (e.g. `Gustav`).
    List {
        pak: String,
        /// Only list entries whose path contains this substring.
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Extract files from a pak into a directory (default: ./extracted/<pak>).
    Extract {
        pak: String,
        #[arg(short, long)]
        filter: Option<String>,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Write a single file from a pak to stdout.
    Cat { pak: String, path: String },
    /// Dump an LSF/LSFX/LSX/LSJ resource as LSX. PATH is looked up across all paks if PAK is `-`.
    Lsx { pak: String, path: String },
    /// Look up a localization handle (`h…` or `h…;version`) in a language pak.
    Loca {
        handle: String,
        /// Language pak name in the install.
        #[arg(short, long, default_value = "English")]
        language: String,
    },
    /// Print a stats object with `using` inheritance resolved.
    Stat {
        name: String,
        /// Also load the Honour-mode modules (Honour, HonourX).
        #[arg(long)]
        honour: bool,
        /// Print only the object's own `data` lines instead of the effective fields.
        #[arg(long)]
        own: bool,
    },
    /// Find resources (LSF/LSX/LSJ) under PREFIX with an attribute whose value equals VALUE.
    Find {
        value: String,
        #[arg(short, long, default_value = "")]
        prefix: String,
    },
    /// Decompress every entry of every pak (or one pak) and check sizes.
    Verify { pak: Option<String> },
    /// Report archive paths that occur in more than one pak, and whether their contents differ.
    Dups,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Info => info(),
        Command::List { pak, filter } => {
            let pak = open(&pak)?;
            for e in matching(&pak, filter.as_deref()) {
                println!("{:>12}  {:?}  {}", e.size(), e.compression()?, e.name);
            }
            Ok(())
        }
        Command::Extract { pak, filter, out } => {
            let pak = open(&pak)?;
            let stem = pak
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let out = out.unwrap_or_else(|| PathBuf::from("extracted").join(stem));
            let mut n = 0;
            for e in matching(&pak, filter.as_deref()) {
                let dest = out.join(safe_relative(&e.name)?);
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::write(&dest, pak.read(e)?).with_context(|| dest.display().to_string())?;
                n += 1;
            }
            eprintln!("extracted {n} files to {}", out.display());
            Ok(())
        }
        Command::Cat { pak, path } => {
            use std::io::Write;
            let pak = open(&pak)?;
            let entry = pak
                .find(&path)
                .with_context(|| format!("{path} not found"))?;
            std::io::stdout().write_all(&pak.read(entry)?)?;
            Ok(())
        }
        Command::Verify { pak } => verify(pak),
        Command::Dups => dups(),
        Command::Find { value, prefix } => find(&value, &prefix),
        Command::Loca { handle, language } => loca(&handle, &language),
        Command::Stat { name, honour, own } => stat(&name, honour, own),
        Command::Lsx { pak, path } => {
            use std::io::Write;
            let data = read_file(&pak, &path)?;
            let res = bg3_lsf::parse(&data).with_context(|| path.clone())?;
            std::io::stdout().write_all(bg3_lsf::to_lsx(&res).as_bytes())?;
            Ok(())
        }
    }
}

fn info() -> Result<()> {
    let install = Install::locate()?;
    println!("app:     {}", install.app.display());
    let note = if install.is_reference_version() {
        ""
    } else {
        " (not the reference build)"
    };
    println!("version: {}{note}", install.version);
    for path in install.paks()? {
        let pak = Package::open(&path)?;
        let h = pak.header();
        println!(
            "  {:<28} files={:<7} parts={:<3} flags={:#04x} priority={}",
            path.file_name().unwrap().to_string_lossy(),
            pak.entries().len(),
            h.num_parts,
            h.flags,
            h.priority
        );
    }
    Ok(())
}

fn verify(only: Option<String>) -> Result<()> {
    let paks = match only {
        Some(name) => vec![resolve(&name)?],
        None => Install::locate()?.paks()?,
    };
    let mut failures = 0;
    for path in paks {
        let pak = Package::open(&path)?;
        let (mut files, mut bytes) = (0u64, 0u64);
        for e in pak.entries() {
            match pak.read(e) {
                Ok(data) => {
                    files += 1;
                    bytes += data.len() as u64;
                }
                Err(err) => {
                    failures += 1;
                    eprintln!("FAIL {}: {err}", path.display());
                }
            }
        }
        println!(
            "{:<28} {files:>7} files {:>10.1} MiB",
            path.file_name().unwrap().to_string_lossy(),
            bytes as f64 / 1048576.0
        );
    }
    if failures > 0 {
        bail!("{failures} entries failed");
    }
    Ok(())
}

fn dups() -> Result<()> {
    use std::collections::BTreeMap;
    use std::hash::{DefaultHasher, Hash, Hasher};

    let paks: Vec<Package> = Install::locate()?
        .paks()?
        .iter()
        .map(Package::open)
        .collect::<Result<_, _>>()?;
    let mut by_name: BTreeMap<&str, Vec<(usize, &bg3_pak::Entry)>> = BTreeMap::new();
    for (i, pak) in paks.iter().enumerate() {
        for e in pak.entries() {
            by_name.entry(&e.name).or_default().push((i, e));
        }
    }
    let (mut same, mut differ) = (0, 0);
    for (name, hits) in by_name.iter().filter(|(_, hits)| hits.len() > 1) {
        let mut hashes = Vec::new();
        for &(i, e) in hits {
            let mut h = DefaultHasher::new();
            paks[i].read(e)?.hash(&mut h);
            hashes.push(h.finish());
        }
        let identical = hashes.windows(2).all(|w| w[0] == w[1]);
        if identical {
            same += 1;
        } else {
            differ += 1;
        }
        let owners: Vec<_> = hits
            .iter()
            .map(|&(i, _)| paks[i].path().file_stem().unwrap().to_string_lossy())
            .collect();
        println!(
            "{}\t{}\t{name}",
            if identical { "same" } else { "DIFF" },
            owners.join(",")
        );
    }
    eprintln!("{same} identical duplicates, {differ} with differing contents");
    Ok(())
}

/// Accept either a filesystem path or a pak stem in the install (`Gustav` → `.../Data/Gustav.pak`).
fn resolve(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(name);
    if path.exists() {
        return Ok(path);
    }
    let want = name.trim_end_matches(".pak");
    Install::locate()?
        .paks()?
        .into_iter()
        .find(|p| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(want)))
        .with_context(|| format!("no pak named {name} in the install"))
}

fn loca(handle: &str, language: &str) -> Result<()> {
    let pak = open(language)?;
    let mut table = bg3_loca::Loca::new();
    for e in pak.entries().iter().filter(|e| e.name.ends_with(".loca")) {
        table
            .merge_bytes(&pak.read(e)?)
            .with_context(|| e.name.clone())?;
    }
    let entry = table
        .get(handle)
        .with_context(|| format!("{handle} not found in {language}"))?;
    println!("{}", entry.text);
    eprintln!("(version {})", entry.version);
    Ok(())
}

fn stat(name: &str, honour: bool, own: bool) -> Result<()> {
    use bg3_stats::install::{BASE_LOAD_ORDER, HONOUR_MODULES, load};

    let paks: Vec<Package> = Install::locate()?
        .paks()?
        .iter()
        .map(Package::open)
        .collect::<Result<_, _>>()?;
    let mut order = BASE_LOAD_ORDER.to_vec();
    if honour {
        order.extend_from_slice(HONOUR_MODULES);
    }
    let stats = load(&paks, &order)?;
    let obj = stats
        .get(name)
        .or_else(|| stats.kinds().find_map(|(_, t)| t.get(name)))
        .with_context(|| format!("no stats object named {name}"))?;
    println!("new {} \"{}\"", obj.kind, obj.name);
    if let Some(t) = &obj.type_name {
        println!("type \"{t}\"");
    }
    if let Some(u) = &obj.using {
        println!("using \"{u}\"");
    }
    let fields = if own { &obj.own } else { &obj.fields };
    for (k, v) in fields {
        let inherited = !own && !obj.own.iter().any(|(ok, _)| ok == k);
        let mark = if inherited { "  // inherited" } else { "" };
        println!("data \"{k}\" \"{v}\"{mark}");
    }
    eprintln!("// defined at {} ({})", obj.source, obj.source.module);
    for prev in &obj.previous {
        eprintln!("// replaces {} ({})", prev, prev.module);
    }
    Ok(())
}

fn find(value: &str, prefix: &str) -> Result<()> {
    fn walk(node: &bg3_lsf::Node, path: &mut Vec<String>, value: &str, hits: &mut Vec<String>) {
        path.push(node.name.clone());
        for attr in &node.attributes {
            if attr.value.to_string() == value {
                hits.push(format!("{} @{}", path.join("/"), attr.name));
            }
        }
        for child in &node.children {
            walk(child, path, value, hits);
        }
        path.pop();
    }

    let vfs = bg3_vfs::Vfs::from_install(&Install::locate()?)?;
    let resource_ext = [".lsf", ".lsx", ".lsj", ".lsfx"];
    for file in vfs.files_under(prefix) {
        let name = file.entry.name.to_ascii_lowercase();
        if !resource_ext.iter().any(|e| name.ends_with(e)) {
            continue;
        }
        let Ok(res) = bg3_lsf::parse(&file.read()?) else {
            eprintln!("skipping unparsable {}", file.entry.name);
            continue;
        };
        let mut hits = Vec::new();
        for region in &res.regions {
            walk(&region.node, &mut Vec::new(), value, &mut hits);
        }
        for hit in hits {
            println!("{}: {hit}", file.entry.name);
        }
    }
    Ok(())
}

/// Read `path` from a named pak, or from the whole VFS when `pak` is `-`.
fn read_file(pak: &str, path: &str) -> Result<Vec<u8>> {
    if pak == "-" {
        let vfs = bg3_vfs::Vfs::from_install(&Install::locate()?)?;
        return Ok(vfs.read(path)?);
    }
    let pak = open(pak)?;
    let entry = pak
        .find(path)
        .with_context(|| format!("{path} not found"))?;
    Ok(pak.read(entry)?)
}

fn open(name: &str) -> Result<Package> {
    let path = resolve(name)?;
    Package::open(&path).with_context(|| path.display().to_string())
}

fn matching<'a>(
    pak: &'a Package,
    filter: Option<&'a str>,
) -> impl Iterator<Item = &'a bg3_pak::Entry> {
    pak.entries()
        .iter()
        .filter(move |e| filter.is_none_or(|f| e.name.contains(f)))
}

/// Reject absolute paths and `..` so extraction can't escape the output directory.
fn safe_relative(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(name);
    if path
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("refusing unsafe archive path {name:?}");
    }
    Ok(path)
}
