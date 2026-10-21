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
    /// Decompress every entry of every pak (or one pak) and check sizes.
    Verify { pak: Option<String> },
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
