//! Finding and loading dialogs in the install's virtual file system.
//!
//! The game identifies a dialog by its **dialog resource** (`DialogBank` entries in
//! `Public/<module>/Content/Assets/Dialogs/**/_merged.lsf`): a resource `ID` and `Name` (Osiris
//! prints a dialog as `Name_ID`), the editor `SourceFile` (`Mods/<m>/Story/Dialogs/….lsj`) and
//! switches such as `automated`. The compiled file is the source path with `Story/Dialogs`
//! replaced by `Story/DialogsBinary` and the extension by `.lsf`.
// bg3: LSDialogs::DialogManager::GetBinaryDialogPath, LSDialogs::DialogManager::LoadDialog
//      (4.1.1.7631656)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bg3_lsf::{Guid, Node, Value};
use bg3_vfs::Vfs;

use crate::{Dialog, Error, Result};

/// Something that can hand out dialogs by dialog-resource id (what `Nested Dialog` nodes refer
/// to). Implemented by [`DialogLibrary`] and by a plain map, for tests.
pub trait DialogProvider {
    fn dialog_by_resource(&self, id: &Guid) -> Option<Arc<Dialog>>;
}

impl DialogProvider for HashMap<Guid, Arc<Dialog>> {
    fn dialog_by_resource(&self, id: &Guid) -> Option<Arc<Dialog>> {
        self.get(id).cloned()
    }
}

/// A provider without any dialogs (nested dialogs are then skipped by the runner).
impl DialogProvider for () {
    fn dialog_by_resource(&self, _: &Guid) -> Option<Arc<Dialog>> {
        None
    }
}

/// A `DialogBank` resource.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogResource {
    /// Resource id: the GUID part of the Osiris `DIALOGRESOURCE` value, and what nested-dialog
    /// nodes refer to.
    pub id: Guid,
    /// Resource name, e.g. `TUT_Start_Brinepool`.
    pub name: String,
    /// Editor source (`Mods/<m>/Story/Dialogs/….lsj`).
    pub source_file: String,
    pub automated: bool,
    pub is_world: bool,
    pub is_behaviour: bool,
    pub is_subbed_dialog: bool,
    pub is_private_dialog: bool,
    pub enable_timeline: bool,
    pub allow_dead_speakers: bool,
    pub is_allowing_join_combat: bool,
    pub block_ftb: bool,
    /// `childResources`: nested dialogs this one can play.
    pub child_resources: Vec<Guid>,
}

impl DialogResource {
    /// The Osiris form `Name_ID`.
    pub fn osiris_name(&self) -> String {
        format!("{}_{}", self.name, self.id)
    }

    /// The compiled dialog's path: `Story/Dialogs` → `Story/DialogsBinary`, extension `.lsf`.
    pub fn binary_path(&self) -> String {
        binary_path(&self.source_file)
    }

    /// Whether the game reports this dialog as `AutomatedDialogStarted` / `…Ended`.
    // bg3: esv::StoryDialogEventManager::OnDialogStarted (4.1.1.7631656)
    pub fn reported_as_automated(&self) -> bool {
        self.automated || self.is_behaviour || self.is_world
    }
}

/// `Mods/X/Story/Dialogs/a/b.lsj` → `Mods/X/Story/DialogsBinary/a/b.lsf`.
pub fn binary_path(source: &str) -> String {
    let source = source.replace('\\', "/");
    let lower = source.to_ascii_lowercase();
    let replaced = match lower.find("/story/dialogs/") {
        Some(i) => format!(
            "{}/Story/DialogsBinary/{}",
            &source[..i],
            &source[i + "/story/dialogs/".len()..]
        ),
        None => source.clone(),
    };
    match replaced.rfind('.') {
        Some(dot) if !replaced[dot..].contains('/') => format!("{}.lsf", &replaced[..dot]),
        _ => format!("{replaced}.lsf"),
    }
}

/// One compiled dialog file found in the VFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFile {
    /// File stem, e.g. `TUT_Start_Brinepool`.
    pub name: String,
    /// Archive path as stored in the pak.
    pub path: String,
    /// Module directory (`Gustav`, `GustavDev`, `Shared`, …).
    pub module: String,
}

/// Every dialog resource and compiled dialog file in the install, with a cache of parsed
/// dialogs.
pub struct DialogLibrary {
    vfs: Arc<Vfs>,
    files: Vec<DialogFile>,
    resources: Vec<DialogResource>,
    by_id: HashMap<Guid, usize>,
    by_name: HashMap<String, usize>,
    cache: Mutex<HashMap<usize, Arc<Dialog>>>,
}

impl std::fmt::Debug for DialogLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogLibrary")
            .field("files", &self.files.len())
            .field("resources", &self.resources.len())
            .finish()
    }
}

impl DialogLibrary {
    /// Indexes every `DialogBank` resource and every `Mods/*/Story/DialogsBinary/**/*.lsf` in
    /// the VFS (dialogs themselves are parsed on demand).
    pub fn new(vfs: Arc<Vfs>) -> Result<Self> {
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

        let mut banks: Vec<_> = vfs
            .files_under("public/")
            .filter(|f| {
                let p = f.entry.name.to_ascii_lowercase();
                p.contains("/content/") && p.contains("/dialogs/") && p.ends_with(".lsf")
            })
            .collect();
        banks.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
        let mut resources = Vec::new();
        for f in banks {
            let res = bg3_lsf::parse(&f.read()?)?;
            if let Some(bank) = res.region("DialogBank") {
                for r in bank.children_named("Resource") {
                    resources.push(load_resource(r)?);
                }
            }
        }
        let mut by_id = HashMap::new();
        let mut by_name = HashMap::new();
        for (i, r) in resources.iter().enumerate() {
            // Later banks (sorted by path: GustavDev after Gustav) override earlier ones.
            by_id.insert(r.id, i);
            by_name.insert(r.name.to_ascii_lowercase(), i);
        }
        Ok(DialogLibrary {
            vfs,
            files,
            resources,
            by_id,
            by_name,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    /// Every compiled dialog file, sorted by path.
    pub fn files(&self) -> &[DialogFile] {
        &self.files
    }

    /// Every dialog resource (bank entries in path order; overridden duplicates included).
    pub fn resources(&self) -> &[DialogResource] {
        &self.resources
    }

    /// Looks a dialog resource up by name (case-insensitive), by Osiris `Name_ID` string, or by
    /// id.
    pub fn find(&self, name: &str) -> Option<&DialogResource> {
        self.index_of(name).map(|i| &self.resources[i])
    }

    /// The resource with this id.
    pub fn resource(&self, id: &Guid) -> Option<&DialogResource> {
        self.by_id.get(id).map(|&i| &self.resources[i])
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        if let Some(id) = Guid::parse(name) {
            return self.by_id.get(&id).copied();
        }
        if let Some((_, id)) = split_uuid_suffix(name)
            && let Some(&i) = self.by_id.get(&id)
        {
            return Some(i);
        }
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    /// Loads (and caches) a dialog by resource name, Osiris name or id.
    pub fn load(&self, name: &str) -> Result<Arc<Dialog>> {
        let i = self
            .index_of(name)
            .ok_or_else(|| Error::NotFound(name.to_owned()))?;
        self.load_index(i)
    }

    /// Loads (and caches) a dialog by resource id.
    pub fn load_resource(&self, id: &Guid) -> Result<Arc<Dialog>> {
        let i = *self
            .by_id
            .get(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        self.load_index(i)
    }

    /// Parses one compiled file (uncached, without resource data).
    pub fn load_file(&self, file: &DialogFile) -> Result<Dialog> {
        Dialog::parse(&self.vfs.read(&file.path)?)
    }

    fn load_index(&self, i: usize) -> Result<Arc<Dialog>> {
        if let Some(d) = self.cache.lock().unwrap().get(&i) {
            return Ok(d.clone());
        }
        let r = &self.resources[i];
        let path = r.binary_path();
        let data = self
            .vfs
            .read(&path)
            .map_err(|_| Error::NotFound(format!("{} ({path})", r.name)))?;
        let mut d = Dialog::parse(&data)?;
        d.resource = Some(r.clone());
        let d = Arc::new(d);
        self.cache.lock().unwrap().insert(i, d.clone());
        Ok(d)
    }
}

impl DialogProvider for DialogLibrary {
    fn dialog_by_resource(&self, id: &Guid) -> Option<Arc<Dialog>> {
        self.load_resource(id).ok()
    }
}

fn load_resource(n: &Node) -> Result<DialogResource> {
    let flag = |name: &str| n.attr(name).and_then(Value::as_bool).unwrap_or(false);
    let id = n
        .attr_str("ID")
        .and_then(Guid::parse)
        .ok_or_else(|| Error::Malformed("DialogBank resource without ID".into()))?;
    Ok(DialogResource {
        id,
        name: n.attr_str("Name").unwrap_or_default().to_owned(),
        source_file: n.attr_str("SourceFile").unwrap_or_default().to_owned(),
        automated: flag("automated"),
        is_world: flag("isWorld"),
        is_behaviour: flag("isBehaviour"),
        is_subbed_dialog: flag("isSubbedDialog"),
        is_private_dialog: flag("isPrivateDialog"),
        enable_timeline: flag("EnableTimeline"),
        allow_dead_speakers: flag("AllowDeadSpeakers"),
        is_allowing_join_combat: flag("IsAllowingJoinCombat"),
        block_ftb: flag("BlockFTB"),
        child_resources: n
            .children_named("childResources")
            .filter_map(|c| c.attr("Object").and_then(Value::as_guid))
            .collect(),
    })
}

/// `Name_0123…` → (`Name`, uuid) when the string ends in `_` + a 36-character UUID.
pub fn split_uuid_suffix(s: &str) -> Option<(&str, Guid)> {
    let (name, id) = s.rsplit_once('_')?;
    if id.len() != 36 {
        return None;
    }
    Some((name, Guid::parse(id)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_osiris_resource_suffix() {
        let (name, id) = split_uuid_suffix("foo_bar_2a464bcd-c513-d282-e54e-17e6bdbf67b6").unwrap();
        assert_eq!(name, "foo_bar");
        assert_eq!(id.to_string(), "2a464bcd-c513-d282-e54e-17e6bdbf67b6");
        assert_eq!(split_uuid_suffix("foo_bar"), None);
    }

    #[test]
    fn binary_path_from_source() {
        assert_eq!(
            binary_path("Mods/GustavDev/Story/Dialogs/Tutorial/X_Y.lsj"),
            "Mods/GustavDev/Story/DialogsBinary/Tutorial/X_Y.lsf"
        );
    }
}
