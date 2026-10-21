//! Finding and loading compiled dialogs (`Mods/<module>/Story/DialogsBinary/**/*.lsf`) in the
//! install's virtual file system.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bg3_lsf::Guid;
use bg3_vfs::Vfs;

use crate::{Dialog, Error, Result};

/// Something that can hand out dialogs by their dialog UUID (what `Nested Dialog` nodes refer
/// to). Implemented by [`DialogLibrary`] and by a plain map, for tests.
pub trait DialogProvider {
    fn dialog_by_uuid(&self, uuid: &Guid) -> Option<Arc<Dialog>>;
}

impl DialogProvider for HashMap<Guid, Arc<Dialog>> {
    fn dialog_by_uuid(&self, uuid: &Guid) -> Option<Arc<Dialog>> {
        self.get(uuid).cloned()
    }
}

/// A provider without any dialogs (nested dialogs are then skipped by the runner).
impl DialogProvider for () {
    fn dialog_by_uuid(&self, _: &Guid) -> Option<Arc<Dialog>> {
        None
    }
}

/// One compiled dialog file found in the VFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFile {
    /// File stem, e.g. `TUT_Start_Brinepool`. This is also the name part of the Osiris
    /// `DIALOGRESOURCE` value (`TUT_Start_Brinepool_<resource uuid>`).
    pub name: String,
    /// Archive path as stored in the pak.
    pub path: String,
    /// Module directory (`Gustav`, `GustavDev`, `Shared`, …).
    pub module: String,
}

/// Index of every compiled dialog in the install, with a cache of parsed dialogs.
pub struct DialogLibrary {
    vfs: Arc<Vfs>,
    files: Vec<DialogFile>,
    by_name: HashMap<String, usize>,
    cache: Mutex<HashMap<usize, Arc<Dialog>>>,
    by_uuid: Mutex<Option<HashMap<Guid, usize>>>,
}

impl std::fmt::Debug for DialogLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogLibrary")
            .field("files", &self.files.len())
            .finish()
    }
}

/// Module priority for picking between same-named dialogs: later modules override earlier ones.
fn module_rank(module: &str) -> usize {
    let order = bg3_vfs::modules::BASE_LOAD_ORDER
        .iter()
        .chain(bg3_vfs::modules::HONOUR_MODULES);
    order
        .enumerate()
        .find(|(_, m)| m.eq_ignore_ascii_case(module))
        .map(|(i, _)| i + 1)
        .unwrap_or(0)
}

impl DialogLibrary {
    /// Indexes every `Mods/*/Story/DialogsBinary/**/*.lsf` in the VFS (no parsing yet).
    pub fn new(vfs: Arc<Vfs>) -> Self {
        let mut files: Vec<DialogFile> = vfs
            .files_under("mods/")
            .filter_map(|f| {
                let path = f.entry.name.replace('\\', "/");
                let lower = path.to_ascii_lowercase();
                if !lower.contains("/story/dialogsbinary/") || !lower.ends_with(".lsf") {
                    return None;
                }
                let module = path.split('/').nth(1)?.to_owned();
                let name = path.rsplit('/').next()?.strip_suffix(".lsf")?.to_owned();
                Some(DialogFile { name, path, module })
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            let key = f.name.to_ascii_lowercase();
            match by_name.get(&key) {
                Some(&j) if module_rank(&files[j].module) >= module_rank(&f.module) => {}
                _ => {
                    by_name.insert(key, i);
                }
            }
        }
        DialogLibrary {
            vfs,
            files,
            by_name,
            cache: Mutex::new(HashMap::new()),
            by_uuid: Mutex::new(None),
        }
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    /// Every dialog file, sorted by path (same-named files in several modules all appear).
    pub fn files(&self) -> &[DialogFile] {
        &self.files
    }

    /// Looks a dialog up by file stem (case-insensitive). Also accepts an Osiris
    /// `DIALOGRESOURCE` string (`Name_<uuid>`): the trailing UUID is ignored.
    pub fn find(&self, name: &str) -> Option<&DialogFile> {
        self.index_of(name).map(|i| &self.files[i])
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        let key = name.to_ascii_lowercase();
        if let Some(&i) = self.by_name.get(&key) {
            return Some(i);
        }
        let stripped = strip_uuid_suffix(&key)?;
        self.by_name.get(stripped).copied()
    }

    /// Loads (and caches) a dialog by name, see [`DialogLibrary::find`].
    pub fn load(&self, name: &str) -> Result<Arc<Dialog>> {
        let i = self
            .index_of(name)
            .ok_or_else(|| Error::NotFound(name.to_owned()))?;
        self.load_index(i)
    }

    /// Loads one file of [`DialogLibrary::files`] (uncached).
    pub fn load_file(&self, file: &DialogFile) -> Result<Dialog> {
        Dialog::parse(&self.vfs.read(&file.path)?)
    }

    fn load_index(&self, i: usize) -> Result<Arc<Dialog>> {
        if let Some(d) = self.cache.lock().unwrap().get(&i) {
            return Ok(d.clone());
        }
        let d = Arc::new(self.load_file(&self.files[i])?);
        self.cache.lock().unwrap().insert(i, d.clone());
        Ok(d)
    }

    /// Loads a dialog by its dialog UUID. The first call parses every dialog once to build the
    /// index (a few seconds in release builds).
    pub fn load_by_uuid(&self, uuid: &Guid) -> Result<Arc<Dialog>> {
        let i = {
            let mut index = self.by_uuid.lock().unwrap();
            let index = index.get_or_insert_with(|| self.build_uuid_index());
            *index
                .get(uuid)
                .ok_or_else(|| Error::NotFound(uuid.to_string()))?
        };
        self.load_index(i)
    }

    fn build_uuid_index(&self) -> HashMap<Guid, usize> {
        let mut out = HashMap::new();
        let mut ranked: Vec<usize> = self.by_name.values().copied().collect();
        ranked.sort_unstable();
        for i in ranked {
            if let Ok(d) = self.load_file(&self.files[i]) {
                out.insert(d.uuid, i);
            }
        }
        out
    }
}

impl DialogProvider for DialogLibrary {
    fn dialog_by_uuid(&self, uuid: &Guid) -> Option<Arc<Dialog>> {
        self.load_by_uuid(uuid).ok()
    }
}

/// `Name_0123…` → `Name` when the string ends in `_` + a 36-character UUID.
fn strip_uuid_suffix(s: &str) -> Option<&str> {
    let (name, id) = s.rsplit_once('_')?;
    (id.len() == 36 && Guid::parse(id).is_some()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_osiris_resource_suffix() {
        assert_eq!(
            strip_uuid_suffix("foo_bar_2a464bcd-c513-d282-e54e-17e6bdbf67b6"),
            Some("foo_bar")
        );
        assert_eq!(strip_uuid_suffix("foo_bar"), None);
    }
}
