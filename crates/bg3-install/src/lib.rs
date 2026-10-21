//! Locate a Baldur's Gate 3 install and describe its layout.
//!
//! Everything here is read-only: we never write into the install.

use std::path::{Path, PathBuf};

/// The build this project is developed and verified against.
pub const REFERENCE_VERSION: &str = "4.1.1.7631656";

/// Environment variable overriding the location of `Baldur's Gate 3.app`.
pub const ENV_APP: &str = "BG3_APP";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("BG3 install not found at {0} (set {ENV_APP} to the path of \"Baldur's Gate 3.app\")")]
    NotFound(PathBuf),
    #[error("{0} does not look like a BG3 install: missing {1}")]
    Invalid(PathBuf, &'static str),
    #[error("could not read version from {path}: {source}")]
    Version {
        path: PathBuf,
        #[source]
        source: plist::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A located BG3 install (currently the macOS app bundle layout).
#[derive(Debug, Clone)]
pub struct Install {
    /// Path to `Baldur's Gate 3.app`.
    pub app: PathBuf,
    /// Game version, e.g. `4.1.1.7631656`.
    pub version: String,
}

impl Install {
    /// Locate the install via `BG3_APP`, falling back to the default Steam location.
    pub fn locate() -> Result<Self> {
        match std::env::var_os(ENV_APP) {
            Some(path) => Self::open(path),
            None => Self::open(default_app_path().ok_or_else(|| Error::NotFound(PathBuf::new()))?),
        }
    }

    /// Open an install at an explicit `Baldur's Gate 3.app` path.
    pub fn open(app: impl Into<PathBuf>) -> Result<Self> {
        let app = app.into();
        if !app.is_dir() {
            return Err(Error::NotFound(app));
        }
        let contents = app.join("Contents");
        if !contents.join("Data").is_dir() {
            return Err(Error::Invalid(app, "Contents/Data"));
        }
        let info_plist = contents.join("Info.plist");
        let version = read_version(&info_plist)?;
        Ok(Self { app, version })
    }

    pub fn is_reference_version(&self) -> bool {
        self.version == REFERENCE_VERSION
    }

    fn contents(&self) -> PathBuf {
        self.app.join("Contents")
    }

    /// `Contents/Data`: main paks.
    pub fn data_dir(&self) -> PathBuf {
        self.contents().join("Data")
    }

    /// `Contents/Data/Localization`: language and voice paks.
    pub fn localization_dir(&self) -> PathBuf {
        self.data_dir().join("Localization")
    }

    /// The main game executable.
    pub fn binary(&self) -> PathBuf {
        self.contents().join("MacOS").join("Baldur's Gate 3")
    }

    /// The Osiris story VM library.
    pub fn osiris_library(&self) -> PathBuf {
        self.contents().join("Frameworks").join("libOsiris.dylib")
    }

    /// All primary `.pak` files (data dir, then localization dir), sorted by path.
    ///
    /// Archive *parts* (`Textures_1.pak`, …) are excluded: they are opened through their
    /// primary pak.
    pub fn paks(&self) -> Result<Vec<PathBuf>> {
        let mut paks = Vec::new();
        for dir in [self.data_dir(), self.localization_dir()] {
            if !dir.is_dir() {
                continue;
            }
            let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "pak"))
                .collect();
            found.sort();
            let primary: Vec<PathBuf> = found
                .iter()
                .filter(|p| !is_part_of_other(p, &found))
                .cloned()
                .collect();
            paks.extend(primary);
        }
        Ok(paks)
    }
}

/// Default macOS Steam location.
pub fn default_app_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        Path::new(&home)
            .join("Library/Application Support/Steam/steamapps/common/Baldurs Gate 3")
            .join("Baldur's Gate 3.app"),
    )
}

fn read_version(info_plist: &Path) -> Result<String> {
    let value = plist::Value::from_file(info_plist).map_err(|source| Error::Version {
        path: info_plist.to_owned(),
        source,
    })?;
    value
        .as_dictionary()
        .and_then(|d| d.get("CFBundleShortVersionString"))
        .and_then(|v| v.as_string())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(info_plist.to_owned(), "CFBundleShortVersionString"))
}

/// `Foo_3.pak` is a part of `Foo.pak` if the latter exists alongside it.
fn is_part_of_other(path: &Path, all: &[PathBuf]) -> bool {
    part_base(path).is_some_and(|base| all.iter().any(|p| p == &base))
}

/// For `dir/Foo_3.pak` returns `dir/Foo.pak`.
pub fn part_base(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let (base, n) = stem.rsplit_once('_')?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(path.with_file_name(format!("{base}.pak")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_base_detects_numbered_parts() {
        assert_eq!(
            part_base(Path::new("/d/Textures_3.pak")),
            Some(PathBuf::from("/d/Textures.pak"))
        );
        assert_eq!(part_base(Path::new("/d/Gustav_Textures.pak")), None);
        assert_eq!(part_base(Path::new("/d/Gustav.pak")), None);
    }

    #[test]
    fn parts_only_excluded_when_base_exists() {
        let all = [
            PathBuf::from("/d/Gustav_Textures.pak"),
            PathBuf::from("/d/Textures.pak"),
            PathBuf::from("/d/Textures_1.pak"),
        ];
        assert!(!is_part_of_other(&all[0], &all));
        assert!(is_part_of_other(&all[2], &all));
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn locate_real_install() {
        let install = Install::locate().expect("locate install");
        let paks = install.paks().unwrap();
        assert!(paks.iter().any(|p| p.ends_with("Gustav.pak")));
        assert!(!paks.iter().any(|p| p.ends_with("Textures_1.pak")));
        assert!(install.binary().is_file());
        eprintln!("BG3 {} ({} paks)", install.version, paks.len());
    }
}
