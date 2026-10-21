//! Unified read-only view over the paks of a BG3 install.
//!
//! Paths are matched case-insensitively (the game's data refers to files with inconsistent case,
//! e.g. `.GR2` vs `.gr2`). When the same path exists in several paks, the pak mounted **last**
//! wins. See `docs/research/vfs-load-order.md` for what is and isn't known about the original order.

pub mod modules;

use std::collections::HashMap;
use std::path::Path;

use bg3_install::Install;
use bg3_pak::{Entry, Package};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Install(#[from] bg3_install::Error),
    #[error(transparent)]
    Pak(#[from] bg3_pak::Error),
    #[error("file not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy)]
struct Location {
    pak: u32,
    entry: u32,
}

/// A file found in the VFS.
#[derive(Clone, Copy)]
pub struct FileRef<'a> {
    pub package: &'a Package,
    pub entry: &'a Entry,
}

impl FileRef<'_> {
    pub fn read(&self) -> Result<Vec<u8>> {
        Ok(self.package.read(self.entry)?)
    }
}

#[derive(Default)]
pub struct Vfs {
    paks: Vec<Package>,
    index: HashMap<String, Location>,
}

impl std::fmt::Debug for Vfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vfs")
            .field("paks", &self.paks.len())
            .field("files", &self.index.len())
            .finish()
    }
}

/// Open every primary pak of the install in **mount order** (see [`sort_by_priority`]).
/// Use this whenever a later pak should override an earlier one.
pub fn open_paks(install: &Install) -> Result<Vec<Package>> {
    let mut packages = install
        .paks()?
        .into_iter()
        .map(Package::open)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    sort_by_priority(&mut packages);
    Ok(packages)
}

/// Order packages so that higher header priority mounts later (and therefore wins).
/// The sort is stable, keeping the caller's order among equal priorities.
pub fn sort_by_priority(packages: &mut [Package]) {
    packages.sort_by_key(|p| p.header().priority);
}

/// Normalise an archive path for lookup: `/` separators, lowercase.
pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount every primary pak in the install in [`Install::paks`] order.
    ///
    /// Paks are mounted in ascending header `priority` (stable within equal priority), so a
    /// higher-priority pak overrides lower ones. The Windows build relies on this for its hotfix
    /// paks (`Patch8_HotFix9` = 1, `Patch8_HotFix10` = 3, `PhysXOverlay` = 100); the macOS build
    /// ships every pak at priority 0.
    pub fn from_install(install: &Install) -> Result<Self> {
        let mut vfs = Self::new();
        for package in open_paks(install)? {
            vfs.mount_package(package);
        }
        Ok(vfs)
    }

    /// Mount a pak on top of everything mounted so far.
    pub fn mount(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let package = Package::open(path)?;
        self.mount_package(package);
        Ok(())
    }

    pub fn mount_package(&mut self, package: Package) {
        let pak = self.paks.len() as u32;
        for (i, entry) in package.entries().iter().enumerate() {
            self.index.insert(
                normalize(&entry.name),
                Location {
                    pak,
                    entry: i as u32,
                },
            );
        }
        self.paks.push(package);
    }

    pub fn packages(&self) -> &[Package] {
        &self.paks
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn get(&self, path: &str) -> Option<FileRef<'_>> {
        let loc = self.index.get(&normalize(path))?;
        let package = &self.paks[loc.pak as usize];
        Some(FileRef {
            package,
            entry: &package.entries()[loc.entry as usize],
        })
    }

    pub fn exists(&self, path: &str) -> bool {
        self.index.contains_key(&normalize(path))
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.get(path)
            .ok_or_else(|| Error::NotFound(path.to_owned()))?
            .read()
    }

    /// All visible files whose normalised path starts with `prefix` (case-insensitive).
    pub fn files_under<'a>(&'a self, prefix: &str) -> impl Iterator<Item = FileRef<'a>> + 'a {
        let prefix = normalize(prefix);
        self.index
            .iter()
            .filter(move |(k, _)| k.starts_with(&prefix))
            .map(|(_, loc)| {
                let package = &self.paks[loc.pak as usize];
                FileRef {
                    package,
                    entry: &package.entries()[loc.entry as usize],
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_is_case_and_separator_insensitive() {
        assert_eq!(normalize(r"Public\Shared\Foo.GR2"), "public/shared/foo.gr2");
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn mounts_whole_install() {
        let install = Install::locate().unwrap();
        let vfs = Vfs::from_install(&install).unwrap();
        assert!(vfs.len() > 1_000_000, "{vfs:?}");
        let story = vfs.read("mods/gustavdev/story/STORY.div.osi").unwrap();
        assert!(!story.is_empty());
        assert!(
            vfs.files_under("Public/Shared/Stats/Generated/Data/")
                .count()
                > 20
        );
    }
}
