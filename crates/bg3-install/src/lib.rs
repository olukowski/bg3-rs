//! Locate a Baldur's Gate 3 install and describe its layout.
//!
//! Two layouts exist (see `docs/platforms.md`):
//!
//! - [`Layout::MacApp`]: the macOS app bundle, `Baldur's Gate 3.app/Contents/{Data,MacOS,…}`.
//! - [`Layout::WindowsStyle`]: `<root>/Data` and `<root>/bin/bg3.exe`, used by the Windows
//!   build and by Linux via Steam Proton.
//!
//! Everything here is read-only: we never write into the install.

mod locate;
mod pe_version;
pub mod vdf;
mod version_scan;

use std::fmt;
use std::path::{Path, PathBuf};

pub use locate::{Candidate, Os, ProbeEnv, STEAM_DIR_NAME, steam_game_dir};
pub use pe_version::{find_version_string, read_exe_version};

/// The build this project is developed and verified against.
pub const REFERENCE_VERSION: &str = "4.1.1.7631656";

/// Environment variable pointing at the install: the `.app` bundle on macOS or the game root
/// (the directory containing `Data` and `bin`) on Windows/Linux.
pub const ENV_INSTALL: &str = "BG3_INSTALL";

/// Older alias of [`ENV_INSTALL`], still honoured.
pub const ENV_APP: &str = "BG3_APP";

/// Version reported when it can't be read from the install.
pub const UNKNOWN_VERSION: &str = "unknown";

/// macOS app bundle name.
pub const APP_BUNDLE: &str = "Baldur's Gate 3.app";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}", not_found_message(.0))]
    NotFound(Vec<(Candidate, String)>),
    #[error("{0} does not look like a BG3 install: {1}")]
    Invalid(PathBuf, String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn not_found_message(tried: &[(Candidate, String)]) -> String {
    let mut s = String::from("BG3 install not found. Looked in:\n");
    if tried.is_empty() {
        s.push_str("  (no candidate locations; is HOME/USERPROFILE set?)\n");
    }
    for (c, why) in tried {
        s.push_str(&format!("  {} [{}]: {why}\n", c.path.display(), c.reason));
    }
    s.push_str(&format!(
        "Set {ENV_INSTALL} to the game directory (\"Baldur's Gate 3.app\" on macOS, the folder \
         containing Data and bin on Windows/Linux)."
    ));
    s
}

pub type Result<T> = std::result::Result<T, Error>;

/// How the install is laid out on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// macOS: `root` is `Baldur's Gate 3.app`; files live under `Contents/`.
    MacApp,
    /// `root` contains `Data/` and `bin/`: Windows, Linux via Proton, and the native Linux build
    /// (`bin/bg3` ELF + `bin/libOsiris.so`).
    WindowsStyle,
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Layout::MacApp => "macOS app bundle",
            Layout::WindowsStyle => "Data/ + bin/ (Windows, Linux)",
        })
    }
}

/// A located BG3 install.
#[derive(Debug, Clone)]
pub struct Install {
    pub layout: Layout,
    /// `Baldur's Gate 3.app` ([`Layout::MacApp`]) or the game root ([`Layout::WindowsStyle`]).
    pub root: PathBuf,
    /// Game version, e.g. `4.1.1.7631656`, or [`UNKNOWN_VERSION`].
    pub version: String,
    /// Non-fatal problems found while opening (e.g. the version couldn't be read).
    pub warnings: Vec<String>,
    data_dir: PathBuf,
    binary: PathBuf,
    osiris: Option<PathBuf>,
}

impl Install {
    /// Locate the install: `BG3_INSTALL` (or `BG3_APP`) if set, else the first valid
    /// [`Install::candidates`] entry.
    pub fn locate() -> Result<Self> {
        for var in [ENV_INSTALL, ENV_APP] {
            if let Some(path) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                let c = Candidate {
                    path: PathBuf::from(path),
                    reason: format!("${var}"),
                };
                return Self::open(&c.path).map_err(|e| match e {
                    Error::Invalid(_, why) => Error::NotFound(vec![(c, why)]),
                    e => e,
                });
            }
        }
        Self::locate_in(&Self::candidates())
    }

    /// Every location [`Install::locate`] probes when no env override is set, in order.
    pub fn candidates() -> Vec<Candidate> {
        locate::candidates(&ProbeEnv::current())
    }

    /// Open the first candidate that is a valid install.
    pub fn locate_in(candidates: &[Candidate]) -> Result<Self> {
        let mut tried = Vec::new();
        for c in candidates {
            match Self::open(&c.path) {
                Ok(install) => return Ok(install),
                Err(e) => {
                    let why = match e {
                        Error::Invalid(_, why) => why,
                        e => e.to_string(),
                    };
                    tried.push((c.clone(), why));
                }
            }
        }
        Err(Error::NotFound(tried))
    }

    /// Open an install at an explicit path.
    ///
    /// Accepts the `.app` bundle, the Steam dir containing it, a Windows-style game root, or its
    /// `bin` directory / an executable in it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::Invalid(path.to_owned(), "does not exist".into()));
        }
        let (layout, root) = detect(path).ok_or_else(|| {
            Error::Invalid(
                path.to_owned(),
                "no Contents/Data (macOS app) or Data directory with .pak files (Windows/Linux)"
                    .into(),
            )
        })?;
        let mut warnings = Vec::new();
        let (data_dir, binary, osiris, version) = match layout {
            Layout::MacApp => {
                let contents = child(&root, "Contents");
                let macos = child(&contents, "MacOS");
                let osiris = child(&child(&contents, "Frameworks"), "libOsiris.dylib");
                let version = read_plist_version(&child(&contents, "Info.plist"))
                    .map_err(|e| warnings.push(e))
                    .ok();
                (
                    child(&contents, "Data"),
                    child(&macos, "Baldur's Gate 3"),
                    osiris.is_file().then_some(osiris),
                    version,
                )
            }
            Layout::WindowsStyle => {
                let bin = child(&root, "bin");
                // Windows (`bg3.exe`, `bg3_dx11.exe`) or the native Linux build (`bg3`, ELF).
                let binary = ["bg3.exe", "bg3_dx11.exe", "bg3"]
                    .iter()
                    .map(|n| child(&bin, n))
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| child(&bin, "bg3.exe"));
                // Windows ships Osiris as `bin/osiris.dll` (BG3 Script Extender loads it by that
                // name; unverified on a real install); the native Linux build as
                // `bin/libOsiris.so` (seen on a real install).
                let osiris = ["osiris.dll", "libOsiris.so"]
                    .iter()
                    .map(|n| child(&bin, n))
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| child(&bin, "osiris.dll"));
                let version = if binary.is_file() {
                    match read_binary_version(&binary) {
                        Ok(Some(v)) => Some(v),
                        Ok(None) => {
                            warnings
                                .push(format!("no version string found in {}", binary.display()));
                            None
                        }
                        Err(e) => {
                            warnings.push(format!("could not read {}: {e}", binary.display()));
                            None
                        }
                    }
                } else {
                    warnings.push(format!(
                        "no bin/bg3.exe, bin/bg3_dx11.exe or bin/bg3 under {}",
                        root.display()
                    ));
                    None
                };
                (
                    child(&root, "Data"),
                    binary,
                    osiris.is_file().then_some(osiris),
                    version,
                )
            }
        };
        Ok(Self {
            layout,
            root,
            version: version.unwrap_or_else(|| UNKNOWN_VERSION.to_owned()),
            warnings,
            data_dir,
            binary,
            osiris,
        })
    }

    pub fn is_reference_version(&self) -> bool {
        self.version == REFERENCE_VERSION
    }

    /// `Data`: main paks.
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    /// `Data/Localization`: language and voice paks.
    pub fn localization_dir(&self) -> PathBuf {
        child(&self.data_dir, "Localization")
    }

    /// The main game executable (`Contents/MacOS/Baldur's Gate 3` or `bin/bg3.exe`, falling
    /// back to `bin/bg3_dx11.exe`). May not exist if the install is incomplete.
    pub fn binary(&self) -> PathBuf {
        self.binary.clone()
    }

    /// The Osiris story VM library (`libOsiris.dylib` on macOS, `bin/osiris.dll` on Windows), if
    /// present.
    pub fn osiris_library(&self) -> Option<PathBuf> {
        self.osiris.clone()
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
            let mut found = list_paks(&dir)?;
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

/// Version of a game executable: the PE version resource for Windows `.exe` files, otherwise (or
/// if that has none) the single version-shaped string embedded in the binary (native Linux ELF).
fn read_binary_version(path: &Path) -> std::io::Result<Option<String>> {
    let mut magic = [0u8; 2];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let _ = f.read(&mut magic)?;
    }
    if &magic == b"MZ"
        && let Some(v) = read_exe_version(path)?
    {
        return Ok(Some(v));
    }
    Ok(version_scan::find_embedded_version(&std::fs::read(path)?))
}

/// Find `name` in `dir`, preferring an exact match and falling back to a case-insensitive one
/// (Linux filesystems are case-sensitive; installs copied from Windows may differ in case).
/// Returns `dir/name` if nothing matches.
fn child(dir: &Path, name: &str) -> PathBuf {
    let exact = dir.join(name);
    if exact.exists() {
        return exact;
    }
    std::fs::read_dir(dir)
        .ok()
        .and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .find(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
                .map(|e| e.path())
        })
        .unwrap_or(exact)
}

fn is_pak(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
}

fn list_paks(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| is_pak(p) && p.is_file())
        .collect())
}

fn has_paks(dir: &Path) -> bool {
    dir.is_dir() && list_paks(dir).is_ok_and(|p| !p.is_empty())
}

/// Work out which layout `path` is, and the install root.
fn detect(path: &Path) -> Option<(Layout, PathBuf)> {
    // An executable (or other file) inside the install: start from its directory.
    let path = if path.is_file() { path.parent()? } else { path };
    let is_mac_app = |p: &Path| has_paks(&child(&child(p, "Contents"), "Data"));
    if is_mac_app(path) {
        return Some((Layout::MacApp, path.to_owned()));
    }
    // The Steam game dir on macOS contains the bundle.
    let bundle = child(path, APP_BUNDLE);
    if is_mac_app(&bundle) {
        return Some((Layout::MacApp, bundle));
    }
    if has_paks(&child(path, "Data")) {
        return Some((Layout::WindowsStyle, path.to_owned()));
    }
    // `<root>/bin` or `<root>/Contents/MacOS`.
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    if name.eq_ignore_ascii_case("bin") && has_paks(&child(parent, "Data")) {
        return Some((Layout::WindowsStyle, parent.to_owned()));
    }
    if name.eq_ignore_ascii_case("MacOS") || name.eq_ignore_ascii_case("Contents") {
        let app = if name.eq_ignore_ascii_case("MacOS") {
            parent.parent()?
        } else {
            parent
        };
        if is_mac_app(app) {
            return Some((Layout::MacApp, app.to_owned()));
        }
    }
    None
}

fn read_plist_version(info_plist: &Path) -> std::result::Result<String, String> {
    let value = plist::Value::from_file(info_plist)
        .map_err(|e| format!("could not read version from {}: {e}", info_plist.display()))?;
    value
        .as_dictionary()
        .and_then(|d| d.get("CFBundleShortVersionString"))
        .and_then(|v| v.as_string())
        .map(str::to_owned)
        .ok_or_else(|| format!("no CFBundleShortVersionString in {}", info_plist.display()))
}

/// `Foo_3.pak` is a part of `Foo.pak` if the latter exists alongside it.
fn is_part_of_other(path: &Path, all: &[PathBuf]) -> bool {
    part_base(path).is_some_and(|base| all.iter().any(|p| p == &base))
}

/// For `dir/Foo_3.pak` returns `dir/Foo.pak`.
pub fn part_base(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?;
    let (base, n) = stem.rsplit_once('_')?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(path.with_file_name(format!("{base}.{ext}")))
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A uniquely named directory under the system temp dir, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "bg3-install-test-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{TempDir, touch};
    use super::*;

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>4.1.1.7631656</string>
</dict></plist>"#;

    /// `<dir>/Baldur's Gate 3.app/Contents/…`
    fn make_mac(dir: &Path) -> PathBuf {
        let app = dir.join(APP_BUNDLE);
        let c = app.join("Contents");
        touch(&c.join("Data").join("Gustav.pak"), b"");
        touch(&c.join("Data").join("Textures.pak"), b"");
        touch(&c.join("Data").join("Textures_1.pak"), b"");
        touch(
            &c.join("Data").join("Localization").join("English.pak"),
            b"",
        );
        touch(&c.join("MacOS").join("Baldur's Gate 3"), b"");
        touch(&c.join("Frameworks").join("libOsiris.dylib"), b"");
        touch(&c.join("Info.plist"), PLIST.as_bytes());
        app
    }

    /// `<dir>/Data`, `<dir>/bin/bg3.exe` with a version resource.
    fn make_windows(dir: &Path, exe: Option<&str>) {
        touch(&dir.join("Data").join("Gustav.pak"), b"");
        touch(
            &dir.join("Data").join("Localization").join("English.pak"),
            b"",
        );
        if let Some(name) = exe {
            let res = pe_version::tests::version_resource(&[("ProductVersion", "4.1.1.7631656")]);
            touch(
                &dir.join("bin").join(name),
                &pe_version::tests::pe_image(&res),
            );
        }
    }

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
    fn opens_mac_app_bundle() {
        let tmp = TempDir::new("mac");
        let app = make_mac(tmp.path());
        for path in [
            app.clone(),
            tmp.path().to_owned(),
            app.join("Contents").join("MacOS"),
            app.join("Contents").join("MacOS").join("Baldur's Gate 3"),
        ] {
            let i = Install::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(i.layout, Layout::MacApp);
            assert_eq!(i.root, app);
        }
        let i = Install::open(&app).unwrap();
        assert_eq!(i.version, "4.1.1.7631656");
        assert!(i.is_reference_version());
        assert!(i.warnings.is_empty());
        assert_eq!(i.data_dir(), app.join("Contents").join("Data"));
        assert_eq!(
            i.localization_dir(),
            app.join("Contents").join("Data").join("Localization")
        );
        assert_eq!(
            i.binary(),
            app.join("Contents").join("MacOS").join("Baldur's Gate 3")
        );
        assert_eq!(
            i.osiris_library(),
            Some(
                app.join("Contents")
                    .join("Frameworks")
                    .join("libOsiris.dylib")
            )
        );
        let names: Vec<_> = i
            .paks()
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Gustav.pak", "Textures.pak", "English.pak"]);
    }

    #[test]
    fn mac_without_plist_is_unknown_version() {
        let tmp = TempDir::new("mac-noplist");
        let app = make_mac(tmp.path());
        std::fs::remove_file(app.join("Contents").join("Info.plist")).unwrap();
        let i = Install::open(&app).unwrap();
        assert_eq!(i.version, UNKNOWN_VERSION);
        assert_eq!(i.warnings.len(), 1);
    }

    #[test]
    fn opens_windows_style_root() {
        let tmp = TempDir::new("win");
        make_windows(tmp.path(), Some("bg3.exe"));
        touch(&tmp.path().join("bin").join("osiris.dll"), b"");
        for path in [
            tmp.path().to_owned(),
            tmp.path().join("bin"),
            tmp.path().join("bin").join("bg3.exe"),
        ] {
            let i = Install::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(i.layout, Layout::WindowsStyle);
            assert_eq!(i.root, tmp.path());
        }
        let i = Install::open(tmp.path()).unwrap();
        assert_eq!(i.version, "4.1.1.7631656");
        assert!(i.warnings.is_empty(), "{:?}", i.warnings);
        assert_eq!(i.data_dir(), tmp.path().join("Data"));
        assert_eq!(
            i.localization_dir(),
            tmp.path().join("Data").join("Localization")
        );
        assert_eq!(i.binary(), tmp.path().join("bin").join("bg3.exe"));
        assert_eq!(
            i.osiris_library(),
            Some(tmp.path().join("bin").join("osiris.dll"))
        );
        assert_eq!(i.paks().unwrap().len(), 2);
    }

    #[test]
    fn windows_dx11_only_and_no_osiris() {
        let tmp = TempDir::new("win-dx11");
        make_windows(tmp.path(), Some("bg3_dx11.exe"));
        let i = Install::open(tmp.path()).unwrap();
        assert_eq!(i.binary(), tmp.path().join("bin").join("bg3_dx11.exe"));
        assert_eq!(i.version, "4.1.1.7631656");
        assert_eq!(i.osiris_library(), None);
    }

    #[test]
    fn windows_without_binary_opens_with_unknown_version() {
        let tmp = TempDir::new("win-nobin");
        make_windows(tmp.path(), None);
        let i = Install::open(tmp.path()).unwrap();
        assert_eq!(i.version, UNKNOWN_VERSION);
        assert_eq!(i.warnings.len(), 1);
        assert!(!i.is_reference_version());
    }

    #[test]
    fn finds_dirs_case_insensitively() {
        let tmp = TempDir::new("case");
        touch(&tmp.path().join("DATA").join("Gustav.PAK"), b"");
        touch(
            &tmp.path()
                .join("DATA")
                .join("localization")
                .join("English.pak"),
            b"",
        );
        let i = Install::open(tmp.path()).unwrap();
        assert_eq!(i.layout, Layout::WindowsStyle);
        let paks = i.paks().unwrap();
        assert_eq!(paks.len(), 2, "{paks:?}");
    }

    #[test]
    fn rejects_non_installs() {
        let tmp = TempDir::new("bad");
        std::fs::create_dir_all(tmp.path().join("Data")).unwrap(); // no paks
        assert!(matches!(
            Install::open(tmp.path()),
            Err(Error::Invalid(_, _))
        ));
        assert!(matches!(
            Install::open(tmp.path().join("missing")),
            Err(Error::Invalid(_, _))
        ));
    }

    #[test]
    fn locate_in_reports_every_candidate() {
        let tmp = TempDir::new("locate");
        let good = tmp.path().join("good");
        make_windows(&good, Some("bg3.exe"));
        let cands = vec![
            Candidate {
                path: tmp.path().join("missing"),
                reason: "first".into(),
            },
            Candidate {
                path: good.clone(),
                reason: "second".into(),
            },
        ];
        assert_eq!(Install::locate_in(&cands).unwrap().root, good);
        let err = Install::locate_in(&cands[..1]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing"), "{msg}");
        assert!(msg.contains("first"), "{msg}");
        assert!(msg.contains(ENV_INSTALL), "{msg}");
    }

    #[test]
    fn locate_follows_steam_library_to_install() {
        let tmp = TempDir::new("e2e");
        let home = tmp.path().join("home");
        let steam = home.join(".local").join("share").join("Steam");
        let lib = tmp.path().join("other-drive").join("SteamLibrary");
        make_windows(&steam_game_dir(&lib), Some("bg3.exe"));
        touch(
            &steam.join("steamapps").join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}",
                steam.display().to_string().replace('\\', "\\\\"),
                lib.display().to_string().replace('\\', "\\\\")
            )
            .as_bytes(),
        );
        let env = ProbeEnv {
            os: Some(Os::Linux),
            home: Some(home),
            ..Default::default()
        };
        let i = Install::locate_in(&locate::candidates(&env)).unwrap();
        assert_eq!(i.root, steam_game_dir(&lib));
        assert_eq!(i.layout, Layout::WindowsStyle);
    }

    #[test]
    #[ignore = "needs BG3 install"]
    fn locate_real_install() {
        let install = match Install::locate() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let paks = install.paks().unwrap();
        assert!(paks.iter().any(|p| p.ends_with("Gustav.pak")));
        assert!(!paks.iter().any(|p| p.ends_with("Textures_1.pak")));
        assert!(install.binary().is_file());
        assert_ne!(install.version, UNKNOWN_VERSION, "{:?}", install.warnings);
        // The embedded-string scan (used for the native Linux build) must agree with the
        // authoritative version source on every platform we can check.
        let scanned = read_binary_version(&install.binary()).unwrap();
        assert_eq!(scanned.as_deref(), Some(install.version.as_str()));
        eprintln!(
            "BG3 {} ({}, {} paks) at {}",
            install.version,
            install.layout,
            paks.len(),
            install.root.display()
        );
    }
}
