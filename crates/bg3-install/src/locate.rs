//! Where to look for an install: platform default Steam roots, their extra library folders
//! (`steamapps/libraryfolders.vdf`) and GOG defaults.
//!
//! Candidate generation is a pure function of a [`ProbeEnv`] (OS, home dir, …) plus reads of
//! `libraryfolders.vdf`, so every platform's search order can be unit-tested on any host.

use std::path::{Path, PathBuf};

use crate::vdf;

/// Steam's directory name for BG3 under `steamapps/common`.
pub const STEAM_DIR_NAME: &str = "Baldurs Gate 3";

/// A path that [`crate::Install::locate`] tries, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub reason: String,
}

impl Candidate {
    fn new(path: PathBuf, reason: impl Into<String>) -> Self {
        Self {
            path,
            reason: reason.into(),
        }
    }
}

/// Which platform's defaults to probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// Inputs to candidate generation, normally read from the environment by [`ProbeEnv::current`].
#[derive(Debug, Clone, Default)]
pub struct ProbeEnv {
    pub os: Option<Os>,
    /// `HOME` (Unix) or `USERPROFILE` (Windows).
    pub home: Option<PathBuf>,
    /// `XDG_DATA_HOME` (Linux).
    pub xdg_data_home: Option<PathBuf>,
    /// `ProgramFiles(x86)` (Windows).
    pub program_files_x86: Option<PathBuf>,
    /// `ProgramFiles` (Windows).
    pub program_files: Option<PathBuf>,
    /// Steam roots found outside the filesystem (Windows registry), with a reason.
    pub extra_steam_roots: Vec<(PathBuf, String)>,
    /// Game dirs found outside the filesystem (GOG registry), with a reason.
    pub extra_game_dirs: Vec<(PathBuf, String)>,
}

impl ProbeEnv {
    pub fn current() -> Self {
        let os = Os::current();
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = match os {
            Os::Windows => var("USERPROFILE").or_else(|| var("HOME")),
            _ => var("HOME"),
        };
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut env = ProbeEnv {
            os: Some(os),
            home,
            xdg_data_home: var("XDG_DATA_HOME"),
            program_files_x86: var("ProgramFiles(x86)"),
            program_files: var("ProgramFiles"),
            ..Default::default()
        };
        #[cfg(windows)]
        registry::add_registry_paths(&mut env);
        env
    }
}

/// Default Steam roots for a platform, in probe order.
pub fn steam_roots(env: &ProbeEnv) -> Vec<Candidate> {
    let mut out = Vec::new();
    let home = env.home.as_deref();
    match env.os.unwrap_or_else(Os::current) {
        Os::MacOs => {
            if let Some(h) = home {
                out.push(Candidate::new(
                    h.join("Library").join("Application Support").join("Steam"),
                    "Steam default (macOS)",
                ));
            }
        }
        Os::Windows => {
            for (root, reason) in &env.extra_steam_roots {
                out.push(Candidate::new(root.clone(), reason.clone()));
            }
            let pf86 = env
                .program_files_x86
                .clone()
                .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
            out.push(Candidate::new(
                pf86.join("Steam"),
                "Steam default (Windows)",
            ));
            if let Some(pf) = &env.program_files {
                out.push(Candidate::new(pf.join("Steam"), "Steam (Program Files)"));
            }
        }
        Os::Linux => {
            if let Some(h) = home {
                out.push(Candidate::new(
                    h.join(".steam").join("steam"),
                    "Steam default (Linux)",
                ));
                out.push(Candidate::new(
                    h.join(".steam").join("root"),
                    "Steam default (Linux)",
                ));
            }
            if let Some(x) = &env.xdg_data_home {
                out.push(Candidate::new(x.join("Steam"), "Steam ($XDG_DATA_HOME)"));
            }
            if let Some(h) = home {
                out.push(Candidate::new(
                    h.join(".local").join("share").join("Steam"),
                    "Steam default (Linux)",
                ));
                out.push(Candidate::new(
                    h.join(".var")
                        .join("app")
                        .join("com.valvesoftware.Steam")
                        .join(".local")
                        .join("share")
                        .join("Steam"),
                    "Steam (Flatpak)",
                ));
                out.push(Candidate::new(
                    h.join("snap")
                        .join("steam")
                        .join("common")
                        .join(".local")
                        .join("share")
                        .join("Steam"),
                    "Steam (Snap)",
                ));
            }
        }
    }
    out
}

/// Where a game dir sits inside a Steam library.
pub fn steam_game_dir(library: &Path) -> PathBuf {
    library
        .join("steamapps")
        .join("common")
        .join(STEAM_DIR_NAME)
}

/// Every path to try, in order, without duplicates.
///
/// For each Steam root we try its own game dir, then every library in its
/// `steamapps/libraryfolders.vdf`. GOG defaults come last.
pub fn candidates(env: &ProbeEnv) -> Vec<Candidate> {
    let os = env.os.unwrap_or_else(Os::current);
    let mut out: Vec<Candidate> = Vec::new();
    for root in steam_roots(env) {
        out.push(Candidate::new(
            steam_game_dir(&root.path),
            format!("{} at {}", root.reason, root.path.display()),
        ));
        let vdf_path = root.path.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(&vdf_path) {
            for lib in vdf::library_folders(&text) {
                out.push(Candidate::new(
                    steam_game_dir(Path::new(&lib)),
                    format!("Steam library listed in {}", vdf_path.display()),
                ));
            }
        }
    }
    for (dir, reason) in &env.extra_game_dirs {
        out.push(Candidate::new(dir.clone(), reason.clone()));
    }
    match os {
        Os::MacOs => out.push(Candidate::new(
            PathBuf::from("/Applications/Baldur's Gate 3.app"),
            "GOG / standalone (macOS)",
        )),
        Os::Windows => {
            out.push(Candidate::new(
                PathBuf::from(r"C:\GOG Games\Baldur's Gate 3"),
                "GOG default",
            ));
            let pf86 = env
                .program_files_x86
                .clone()
                .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
            out.push(Candidate::new(
                pf86.join("GOG Galaxy")
                    .join("Games")
                    .join("Baldur's Gate 3"),
                "GOG Galaxy default",
            ));
        }
        Os::Linux => {}
    }
    dedup(out)
}

/// Drop repeats, comparing canonical paths where they exist (`~/.steam/steam` is usually a
/// symlink to `~/.local/share/Steam`) and case-insensitively on Windows-style paths.
fn dedup(list: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for c in list {
        let canonical = std::fs::canonicalize(&c.path).unwrap_or_else(|_| c.path.clone());
        let mut key = canonical.to_string_lossy().replace('\\', "/");
        while key.ends_with('/') && key.len() > 1 {
            key.pop();
        }
        if cfg!(windows) || key.as_bytes().get(1) == Some(&b':') {
            key = key.to_ascii_lowercase();
        }
        if !seen.contains(&key) {
            seen.push(key);
            out.push(c);
        }
    }
    out
}

/// Parse the value of `reg query <key> /v <name>` output (`    SteamPath    REG_SZ    c:/…`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_reg_query(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("REG_SZ")?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(windows)]
mod registry {
    use super::{ProbeEnv, parse_reg_query};
    use std::path::PathBuf;

    /// Query one registry value with the stock `reg.exe` (no extra dependencies).
    fn query(key: &str, name: &str) -> Option<String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("reg")
            .args(["query", key, "/v", name])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_reg_query(&String::from_utf8_lossy(&out.stdout))
    }

    pub fn add_registry_paths(env: &mut ProbeEnv) {
        for (key, name) in [
            (r"HKCU\Software\Valve\Steam", "SteamPath"),
            (r"HKLM\SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"),
            (r"HKLM\SOFTWARE\Valve\Steam", "InstallPath"),
        ] {
            if let Some(v) = query(key, name) {
                env.extra_steam_roots
                    .push((PathBuf::from(v), format!("Steam (registry {key}\\{name})")));
            }
        }
        // GOG Galaxy records installs per product id; 1456460669 is BG3's GOG id.
        for key in [
            r"HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\1456460669",
            r"HKLM\SOFTWARE\GOG.com\Games\1456460669",
        ] {
            if let Some(v) = query(key, "path") {
                env.extra_game_dirs
                    .push((PathBuf::from(v), format!("GOG (registry {key})")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn paths(c: &[Candidate]) -> Vec<PathBuf> {
        c.iter().map(|c| c.path.clone()).collect()
    }

    #[test]
    fn macos_defaults() {
        let env = ProbeEnv {
            os: Some(Os::MacOs),
            home: Some(PathBuf::from("/nonexistent-home")),
            ..Default::default()
        };
        let c = paths(&candidates(&env));
        assert_eq!(
            c,
            vec![
                PathBuf::from(
                    "/nonexistent-home/Library/Application Support/Steam/steamapps/common/Baldurs Gate 3"
                ),
                PathBuf::from("/Applications/Baldur's Gate 3.app"),
            ]
        );
    }

    #[test]
    fn linux_defaults() {
        let home = PathBuf::from("/nonexistent-home");
        let env = ProbeEnv {
            os: Some(Os::Linux),
            home: Some(home.clone()),
            ..Default::default()
        };
        let c = paths(&candidates(&env));
        let game = |p: &str| steam_game_dir(&home.join(p));
        assert_eq!(
            c,
            vec![
                game(".steam/steam"),
                game(".steam/root"),
                game(".local/share/Steam"),
                game(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
                game("snap/steam/common/.local/share/Steam"),
            ]
        );
    }

    #[test]
    fn windows_defaults() {
        let pf86 = PathBuf::from(r"C:\Program Files (x86)");
        let env = ProbeEnv {
            os: Some(Os::Windows),
            home: Some(PathBuf::from(r"C:\Users\me")),
            program_files_x86: Some(pf86.clone()),
            extra_steam_roots: vec![(PathBuf::from(r"E:\Steam"), "registry".into())],
            ..Default::default()
        };
        let c = candidates(&env);
        assert_eq!(
            paths(&c),
            vec![
                steam_game_dir(Path::new(r"E:\Steam")),
                steam_game_dir(&pf86.join("Steam")),
                PathBuf::from(r"C:\GOG Games\Baldur's Gate 3"),
                pf86.join("GOG Galaxy")
                    .join("Games")
                    .join("Baldur's Gate 3"),
            ]
        );
        assert!(c[0].reason.contains("registry"));
    }

    #[test]
    fn follows_library_folders() {
        let tmp = TempDir::new("vdf");
        let home = tmp.path().join("home");
        let steam = home.join(".local").join("share").join("Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        let lib = tmp.path().join("mnt").join("games").join("SteamLibrary");
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            steam.display().to_string().replace('\\', "\\\\"),
            lib.display().to_string().replace('\\', "\\\\"),
        );
        std::fs::write(steam.join("steamapps").join("libraryfolders.vdf"), vdf).unwrap();
        let env = ProbeEnv {
            os: Some(Os::Linux),
            home: Some(home.clone()),
            ..Default::default()
        };
        let c = candidates(&env);
        let p = paths(&c);
        let own = steam_game_dir(&steam);
        let extra = steam_game_dir(&lib);
        assert!(p.contains(&own));
        assert!(p.contains(&extra));
        // Listed once even though the vdf repeats the root itself.
        assert_eq!(p.iter().filter(|x| **x == own).count(), 1);
        let extra_c = c.iter().find(|c| c.path == extra).unwrap();
        assert!(extra_c.reason.contains("libraryfolders.vdf"));
    }

    #[test]
    #[cfg(unix)]
    fn dedups_symlinked_roots() {
        let tmp = TempDir::new("symlink");
        let home = tmp.path().join("home");
        let real = home.join(".local").join("share").join("Steam");
        std::fs::create_dir_all(steam_game_dir(&real)).unwrap();
        std::fs::create_dir_all(home.join(".steam")).unwrap();
        std::os::unix::fs::symlink(&real, home.join(".steam").join("steam")).unwrap();
        let env = ProbeEnv {
            os: Some(Os::Linux),
            home: Some(home.clone()),
            ..Default::default()
        };
        let c = paths(&candidates(&env));
        assert_eq!(c[0], steam_game_dir(&home.join(".steam").join("steam")));
        assert!(!c.contains(&steam_game_dir(&real)));
    }

    #[test]
    fn dedups_windows_paths_case_insensitively() {
        let list = vec![
            Candidate::new(PathBuf::from(r"C:\Steam\x"), "a"),
            Candidate::new(PathBuf::from(r"c:\steam\X"), "b"),
        ];
        assert_eq!(dedup(list).len(), 1);
    }

    #[test]
    fn reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    c:/program files (x86)/steam\r\n\r\n";
        assert_eq!(
            parse_reg_query(out).as_deref(),
            Some("c:/program files (x86)/steam")
        );
        assert_eq!(parse_reg_query("ERROR: not found"), None);
    }
}
