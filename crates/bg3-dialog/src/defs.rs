//! Definition resources that dialogs refer to by UUID: flags (`Public/*/Flags/*.lsf`), tags
//! (`Public/*/Tags/*.lsf`) and script flags (`Mods/*/Story/Dialogs/ScriptFlags/ScriptFlags.lsx`).

use std::collections::HashMap;

use bg3_lsf::{Guid, Node, Value};
use bg3_vfs::Vfs;

use crate::model::FlagKind;
use crate::{Error, Result};

/// A flag resource (`Flags` region).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagDefinition {
    pub uuid: Guid,
    pub name: String,
    pub description: String,
    /// `Usage`: which kind of flag this is (see [`FlagDefinition::kind`]).
    pub usage: u8,
}

impl FlagDefinition {
    /// The flag group type that uses flags with this `Usage` value.
    ///
    /// Derived from the reference build: every flag referenced from a dialog flag group of a
    /// given type has the same `Usage` (Local 1, User 2, Party 3, Object 4, Global 5, Dialog 6,
    /// UserShape 7; all 128 k references in the reference build except 30 that point to undefined ids). `0` occurs on
    /// one unused flag.
    pub fn kind(&self) -> Option<FlagKind> {
        Some(match self.usage {
            1 => FlagKind::Local,
            2 => FlagKind::User,
            3 => FlagKind::Party,
            4 => FlagKind::Object,
            5 => FlagKind::Global,
            6 => FlagKind::Dialog,
            7 => FlagKind::UserShape,
            _ => return None,
        })
    }

    /// The name Osiris uses for the flag: `Name_uuid`.
    pub fn osiris_name(&self) -> String {
        format!("{}_{}", self.name, self.uuid)
    }
}

/// A tag resource (`Tags` region).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDefinition {
    pub uuid: Guid,
    pub name: String,
    pub description: String,
    /// `DisplayName` loca handle (may be empty).
    pub display_name: String,
    /// `Categories` (e.g. `Dialog`, `DialogHidden`, `Race`, `Story`).
    pub categories: Vec<String>,
}

/// A script flag: a small condition script evaluated by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFlag {
    pub uuid: Guid,
    pub name: String,
    pub description: String,
    /// The raw script: `CONDITION Keyword(args)` lines followed by `CHECK "c1|c2"`.
    pub script: String,
}

impl ScriptFlag {
    /// The script's `CONDITION` lines as `(keyword, raw argument text)`.
    pub fn conditions(&self) -> Vec<(&str, &str)> {
        self.script
            .lines()
            .filter_map(|l| l.trim().strip_prefix("CONDITION "))
            .filter_map(|c| {
                let (kw, rest) = c.split_once('(')?;
                Some((kw.trim(), rest.trim_end().strip_suffix(')').unwrap_or(rest)))
            })
            .collect()
    }

    /// The `CHECK` expression, e.g. `c1|c2`.
    pub fn check(&self) -> Option<&str> {
        self.script
            .lines()
            .find_map(|l| l.trim().strip_prefix("CHECK "))
            .map(|c| c.trim().trim_matches('"'))
    }
}

/// A speaker group (`Public/*/Voice/SpeakerGroups.lsf`), e.g. `GROUP_Players`. Speaker-list
/// entries name either a character or a speaker group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerGroup {
    pub uuid: Guid,
    pub name: String,
    pub description: String,
}

/// All flag, tag, script-flag and speaker-group definitions of the install, keyed by UUID.
#[derive(Debug, Clone, Default)]
pub struct Definitions {
    pub flags: HashMap<Guid, FlagDefinition>,
    pub tags: HashMap<Guid, TagDefinition>,
    pub script_flags: HashMap<Guid, ScriptFlag>,
    pub speaker_groups: HashMap<Guid, SpeakerGroup>,
}

impl Definitions {
    /// Loads every definition visible in the VFS. Same-UUID resources in several modules: the
    /// path that sorts last wins (later modules such as `GustavDev` sort after `Gustav`).
    pub fn load(vfs: &Vfs) -> Result<Self> {
        let mut defs = Definitions::default();
        let mut paths: Vec<(String, bg3_vfs::FileRef<'_>)> = vfs
            .files_under("")
            .map(|f| (f.entry.name.to_ascii_lowercase(), f))
            .filter(|(p, _)| {
                (p.starts_with("public/") && (p.contains("/flags/") || p.contains("/tags/")))
                    && p.ends_with(".lsf")
                    || p.ends_with("/story/dialogs/scriptflags/scriptflags.lsx")
                    || p.starts_with("public/") && p.ends_with("/voice/speakergroups.lsf")
            })
            .collect();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        for (p, f) in paths {
            let res = bg3_lsf::parse(&f.read()?)?;
            if p.ends_with("speakergroups.lsf") {
                if let Some(root) = res.region("SpeakerGroups") {
                    for g in root.children_named("SpeakerGroup") {
                        let g = SpeakerGroup {
                            uuid: req_guid(g, "UUID")?,
                            name: text(g, "Name"),
                            description: text(g, "Description"),
                        };
                        defs.speaker_groups.insert(g.uuid, g);
                    }
                }
            } else if p.ends_with("scriptflags.lsx") {
                if let Some(root) = res.region("ScriptFlags") {
                    for s in root
                        .children
                        .iter()
                        .flat_map(|c| c.children_named("ScriptFlag"))
                        .chain(root.children_named("ScriptFlag"))
                    {
                        let sf = load_script_flag(s)?;
                        defs.script_flags.insert(sf.uuid, sf);
                    }
                }
            } else if let Some(n) = res.region("Flags") {
                let d = FlagDefinition {
                    uuid: req_guid(n, "UUID")?,
                    name: text(n, "Name"),
                    description: text(n, "Description"),
                    usage: n.attr("Usage").and_then(Value::as_i64).unwrap_or(0) as u8,
                };
                defs.flags.insert(d.uuid, d);
            } else if let Some(n) = res.region("Tags") {
                let d = TagDefinition {
                    uuid: req_guid(n, "UUID")?,
                    name: text(n, "Name"),
                    description: text(n, "Description"),
                    display_name: text(n, "DisplayName"),
                    categories: n
                        .child("Categories")
                        .map(|c| {
                            c.children_named("Category")
                                .filter_map(|c| c.attr_str("Name").map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                defs.tags.insert(d.uuid, d);
            }
        }
        Ok(defs)
    }

    /// A human-readable name for a flag/tag/script-flag UUID (`Name` if known, else the UUID).
    pub fn name_of(&self, uuid: &Guid) -> String {
        if let Some(f) = self.flags.get(uuid) {
            f.name.clone()
        } else if let Some(t) = self.tags.get(uuid) {
            t.name.clone()
        } else if let Some(s) = self.script_flags.get(uuid) {
            s.name.clone()
        } else if let Some(g) = self.speaker_groups.get(uuid) {
            g.name.clone()
        } else {
            uuid.to_string()
        }
    }
}

fn load_script_flag(n: &Node) -> Result<ScriptFlag> {
    Ok(ScriptFlag {
        uuid: req_guid(n, "UUID")?,
        name: text(n, "name"),
        description: text(n, "Description"),
        script: text(n, "Script").replace("\r\n", "\n"),
    })
}

fn text(n: &Node, name: &str) -> String {
    n.attr_str(name).unwrap_or_default().to_owned()
}

fn req_guid(n: &Node, name: &str) -> Result<Guid> {
    match n.attr(name) {
        Some(Value::Uuid(g)) => Ok(*g),
        Some(v) => v
            .as_str()
            .and_then(Guid::parse)
            .ok_or_else(|| Error::Malformed(format!("{}: bad {name}", n.name))),
        None => Err(Error::Malformed(format!("{} without {name}", n.name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_flag_parts() {
        let sf = ScriptFlag {
            uuid: Guid::default(),
            name: "x".into(),
            description: String::new(),
            script: "CONDITION IsFlag(1,\"A_0\",0)\nCONDITION IsTagged(2,\"T\")\nCHECK \"c1|c2\""
                .into(),
        };
        assert_eq!(
            sf.conditions(),
            vec![("IsFlag", "1,\"A_0\",0"), ("IsTagged", "2,\"T\"")]
        );
        assert_eq!(sf.check(), Some("c1|c2"));
    }
}
