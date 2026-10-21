//! Loading all stats text files from a set of paks in module load order.

use std::collections::BTreeMap;

use bg3_pak::Package;

use crate::{Error, Stats};

/// Module load order for the base game (no Honour mode), lowest priority first.
///
/// Derived from `Mods/*/meta.lsx` dependencies in the reference build: `Shared` and `SharedDev`
/// are not listed as anyone's dependency (always-loaded engine-level modules) and go first;
/// `Gustav` depends on the dice sets, `MainUI`, `ModBrowser`, `PhotoMode` and `CrossplayUI`;
/// `GustavDev` → `Gustav`; `GustavX` → `GustavDev`. Of those, only `PhotoMode` ships stats.
/// See `docs/formats/stats.md` for the uncertainty around this order.
pub const BASE_LOAD_ORDER: &[&str] = &[
    "Shared",
    "SharedDev",
    "PhotoMode",
    "Gustav",
    "GustavDev",
    "GustavX",
];

/// Additional modules for Honour mode, appended after [`BASE_LOAD_ORDER`]
/// (`Honour` → `GustavDev`; `HonourX` → `GustavX`, `Honour`).
pub const HONOUR_MODULES: &[&str] = &["Honour", "HonourX"];

/// A stats text file found in a pak.
#[derive(Debug, Clone)]
pub struct ModuleFile {
    pub module: String,
    /// Path inside the pak, e.g. `Public/Shared/Stats/Generated/Data/Armor.txt`.
    pub path: String,
}

/// The module folder of a stats text file path, if it is one
/// (`Public/<Module>/Stats/Generated/**.txt`).
pub fn stats_module(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("Public/")?;
    let (module, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("Stats/Generated/")?;
    rest.to_ascii_lowercase()
        .ends_with(".txt")
        .then_some(module)
}

/// Order of files inside one module: `Structure/` first (modifier lists / value lists), then all
/// other files sorted case-insensitively by path.
fn file_order_key(path: &str) -> (bool, String) {
    let lower = path.to_ascii_lowercase();
    (!lower.contains("/stats/generated/structure/"), lower)
}

/// Every stats text file of the given modules in `paks`, in load order. When the same path is in
/// several paks, the later pak wins.
pub fn collect_files<'a>(
    paks: &'a [Package],
    load_order: &[&str],
) -> Vec<(&'a Package, ModuleFile)> {
    let mut by_path: BTreeMap<String, (usize, &Package, ModuleFile)> = BTreeMap::new();
    for pak in paks {
        for entry in pak.entries() {
            let Some(module) = stats_module(&entry.name) else {
                continue;
            };
            let Some(rank) = load_order.iter().position(|m| *m == module) else {
                continue;
            };
            by_path.insert(
                entry.name.to_ascii_lowercase(),
                (
                    rank,
                    pak,
                    ModuleFile {
                        module: module.to_owned(),
                        path: entry.name.clone(),
                    },
                ),
            );
        }
    }
    let mut files: Vec<_> = by_path.into_values().collect();
    files.sort_by_key(|a| (a.0, file_order_key(&a.2.path)));
    files.into_iter().map(|(_, pak, f)| (pak, f)).collect()
}

/// Modules that ship stats files in `paks` but are not in `load_order` (for diagnostics).
pub fn unlisted_modules(paks: &[Package], load_order: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = paks
        .iter()
        .flat_map(|p| p.entries())
        .filter_map(|e| stats_module(&e.name))
        .filter(|m| !load_order.contains(m))
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Load, merge and resolve all stats of `load_order` from `paks`.
pub fn load(paks: &[Package], load_order: &[&str]) -> Result<Stats, Error> {
    let mut stats = Stats::new();
    for (pak, file) in collect_files(paks, load_order) {
        let entry = pak.find(&file.path).expect("entry listed by collect_files");
        let bytes = pak.read(entry).map_err(|e| Error::Pak {
            path: file.path.clone(),
            source: e,
        })?;
        let text = String::from_utf8(bytes).map_err(|_| Error::InFile {
            path: file.path.clone(),
            source: Box::new(Error::NotUtf8),
        })?;
        stats.load_str(&file.module, &file.path, &text)?;
    }
    stats.finish();
    Ok(stats)
}
