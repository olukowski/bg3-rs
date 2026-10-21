//! Inventory of the engine API a trace exercises: every engine call, query and event in a
//! libOsiris runtime log, with counts and an example, grouped by subsystem, plus how many of
//! the story's declared engine functions it uses. `bg3x sim-replay --inventory` prints it as
//! Markdown; `docs/research/osiris-engine-api.md` is built from that output.
//!
//! Examples are abbreviated: the UUID part of every GUIDSTRING is replaced by `…`, so the
//! output names objects without copying identifiers from the game data.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use bg3_osiris::trace::{ActionKind, LogData, LogValue, Record, parse_log};
use bg3_story::{FunctionKind, Story};

use crate::host::implements;
use crate::replay::{EventClass, classify};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApiKind {
    Call,
    Query,
    Event,
}

impl ApiKind {
    fn name(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Query => "query",
            Self::Event => "event",
        }
    }
}

/// `(kind, name, arity)`.
pub type ApiKey = (ApiKind, String, usize);

#[derive(Clone, Debug, Default)]
pub struct ApiUse {
    pub count: u64,
    /// Calls: logged `call failed!`; queries: logged `Query failed!`; events: logged as
    /// ignored (no rules use them).
    pub failed: u64,
    /// First occurrence, abbreviated.
    pub example: String,
}

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub uses: BTreeMap<ApiKey, ApiUse>,
    /// Declared engine functions in the story: `(kind, declared, used in the trace)`.
    pub declared: Vec<(ApiKind, usize, usize)>,
}

/// Replace every UUID (`8-4-4-4-12` hex) and loca handle (`h` + 8-4-4-4-12 hex with `g`
/// separators) inside a text by `…`.
fn scrub(text: &str) -> String {
    let b = text.as_bytes();
    let is_uuid_at = |i: usize, sep: u8| -> bool {
        b.len() >= i + 36
            && b[i..i + 36].iter().enumerate().all(|(k, &c)| {
                if matches!(k, 8 | 13 | 18 | 23) {
                    c == sep
                } else {
                    c.is_ascii_hexdigit()
                }
            })
    };
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if is_uuid_at(i, b'-') {
            out.push('…');
            i += 36;
        } else if b[i] == b'h' && is_uuid_at(i + 1, b'g') {
            out.push_str("h…");
            i += 37;
        } else {
            let ch = text[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Abbreviate a logged value: `Name_<uuid>` → `Name_…`, a bare UUID → `…`, UUIDs and loca
/// handles inside strings → `…`.
fn abbreviate(v: &LogValue) -> String {
    let text = v.to_string();
    let LogData::Guid(s) = &v.v else {
        return scrub(&text);
    };
    if bg3_osiris::parse_uuid_suffix(s).is_none() {
        return scrub(&text);
    }
    let prefix = &s[..s.len() - 36];
    let ty = v.ty.as_ref().map_or(String::new(), |t| format!("({t})"));
    format!("{ty}{prefix}…")
}

fn example(name: &str, args: impl Iterator<Item = String>) -> String {
    let a: Vec<String> = args.collect();
    format!("{name}({})", a.join(", "))
}

impl Inventory {
    pub fn from_log(story: &Story, log: &str) -> Self {
        let mut inv = Self::default();
        let lines = parse_log(log);
        let mut last_call: Option<(ApiKind, String, usize)> = None;
        let mut last_query: Option<(ApiKind, String, usize)> = None;
        for l in &lines {
            match &l.record {
                Record::Action {
                    kind: ActionKind::DivCall,
                    name,
                    args,
                    ..
                } => {
                    let k = (ApiKind::Call, name.clone(), args.len());
                    let u = inv.uses.entry(k.clone()).or_default();
                    u.count += 1;
                    if u.example.is_empty() {
                        u.example = example(name, args.iter().map(abbreviate));
                    }
                    last_call = Some(k);
                }
                Record::CallFailed => {
                    if let Some(k) = last_call.take() {
                        inv.uses.entry(k).or_default().failed += 1;
                    }
                }
                Record::QueryExec { kind, name, args } if kind == "DIV query" => {
                    let k = (ApiKind::Query, name.clone(), args.len());
                    let u = inv.uses.entry(k.clone()).or_default();
                    u.count += 1;
                    if u.example.is_empty() {
                        u.example = example(
                            name,
                            args.iter().map(|a| {
                                if a.out {
                                    format!("[out] {}", abbreviate(&a.value))
                                } else {
                                    abbreviate(&a.value)
                                }
                            }),
                        );
                    }
                    last_query = Some(k);
                }
                // `Query failed!` belongs to the query right before it; a user query's QRY rules
                // run in between, so anything else resets.
                Record::QueryFailed => {
                    if let Some(k) = last_query.take() {
                        inv.uses.entry(k).or_default().failed += 1;
                    }
                }
                Record::QueryExec { .. } | Record::QueryReturns { .. } => last_query = None,
                Record::Event { name, args } => {
                    let k = (ApiKind::Event, name.clone(), args.len());
                    let u = inv.uses.entry(k).or_default();
                    u.count += 1;
                    if u.example.is_empty() {
                        u.example = example(name, args.iter().map(abbreviate));
                    }
                }
                Record::EventIgnored { name, arity } => {
                    let k = (ApiKind::Event, name.clone(), *arity as usize);
                    let u = inv.uses.entry(k).or_default();
                    u.count += 1;
                    u.failed += 1;
                    if u.example.is_empty() {
                        u.example = format!("{name}/{arity}");
                    }
                }
                _ => {}
            }
        }
        for (kind, fk) in [
            (ApiKind::Call, FunctionKind::Call),
            (ApiKind::Query, FunctionKind::Query),
            (ApiKind::Event, FunctionKind::Event),
        ] {
            let decl: Vec<_> = story.functions.iter().filter(|f| f.kind == fk).collect();
            let used = decl
                .iter()
                .filter(|f| {
                    inv.uses.contains_key(&(
                        kind,
                        f.signature.name.clone(),
                        f.signature.params.len(),
                    ))
                })
                .count();
            inv.declared.push((kind, decl.len(), used));
        }
        inv
    }

    /// Markdown tables, one per subsystem.
    pub fn markdown(&self) -> String {
        let mut by_sub: BTreeMap<&str, Vec<(&ApiKey, &ApiUse)>> = BTreeMap::new();
        for (k, u) in &self.uses {
            by_sub.entry(subsystem(&k.1)).or_default().push((k, u));
        }
        let mut s = String::new();
        for (kind, decl, used) in &self.declared {
            let _ = writeln!(
                s,
                "- {} {}s declared by the story, {used} used in the trace",
                decl,
                kind.name()
            );
        }
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "| Subsystem | calls (uses) | queries (uses) | events (deliveries) | in bg3-sim |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|");
        for (sub, list) in &by_sub {
            let sum = |kind: ApiKind| -> (usize, u64) {
                let l: Vec<_> = list.iter().filter(|(k, _)| k.0 == kind).collect();
                (l.len(), l.iter().map(|(_, u)| u.count).sum())
            };
            let (c, cu) = sum(ApiKind::Call);
            let (q, qu) = sum(ApiKind::Query);
            let (e, eu) = sum(ApiKind::Event);
            let sim = list.iter().filter(|(k, _)| in_sim(k)).count();
            let _ = writeln!(
                s,
                "| {sub} | {c} ({cu}) | {q} ({qu}) | {e} ({eu}) | {sim} of {} |",
                list.len()
            );
        }
        let _ = writeln!(s);
        for (sub, mut list) in by_sub {
            list.sort_by(|a, b| a.0.0.cmp(&b.0.0).then(b.1.count.cmp(&a.1.count)));
            let _ = writeln!(s, "### {sub}\n");
            let _ = writeln!(
                s,
                "| | Function | Count | Failed / ignored | Sim | Example |"
            );
            let _ = writeln!(s, "|---|---|---:|---:|---|---|");
            for ((kind, name, arity), u) in list {
                let sim = match kind {
                    ApiKind::Event => match classify(name) {
                        EventClass::SimOwned => "raises",
                        EventClass::External => "external",
                    },
                    _ if implements(name, *arity) => "yes",
                    _ => "",
                };
                let _ = writeln!(
                    s,
                    "| {} | `{name}/{arity}` | {} | {} | {sim} | `{}` |",
                    kind.name(),
                    u.count,
                    if u.failed > 0 {
                        u.failed.to_string()
                    } else {
                        String::new()
                    },
                    u.example.replace('|', "\\|")
                );
            }
            let _ = writeln!(s);
        }
        s
    }
}

fn in_sim(k: &ApiKey) -> bool {
    match k.0 {
        ApiKind::Event => classify(&k.1) == EventClass::SimOwned,
        _ => implements(&k.1, k.2),
    }
}

/// The subsystem a function belongs to, for grouping (by name; first match wins).
pub fn subsystem(name: &str) -> &'static str {
    const EXACT: &[(&str, &str)] = &[
        ("Exists", "Entities & templates"),
        ("IsCharacter", "Entities & templates"),
        ("IsItem", "Entities & templates"),
        ("GetTemplate", "Entities & templates"),
        ("EnteredLevel", "Levels & game flow"),
        ("LeftLevel", "Levels & game flow"),
        ("GetRegion", "Levels & game flow"),
        ("TagEvent", "Tags"),
        ("CharacterTagEvent", "Tags"),
        ("StatusTagSet", "Statuses"),
        ("RequestProcessed", "Party & users"),
        ("InitializePlayerData", "Character creation"),
        ("LeveledUp", "Character creation"),
        ("IterateUsers", "Party & users"),
        ("UnlockedRecipe", "Inventory & items"),
        ("UnlockRecipe", "Inventory & items"),
        ("Random", "Math & strings"),
        ("AnimationEvent", "Entities & templates"),
        ("UserEvent", "Entities & templates"),
        ("EntityEvent", "Entities & templates"),
        ("SetEntityEvent", "Entities & templates"),
        ("OnStoryOverride", "Entities & templates"),
        ("QuestAccepted", "Quests & journal"),
        ("QuestUpdateUnlocked", "Quests & journal"),
        ("QuestUpdate", "Quests & journal"),
        ("AddEntryToCustomBook", "Quests & journal"),
        ("PlayHUDSound", "Presentation"),
        ("PlayLoopEffect", "Presentation"),
        ("StopLoopEffect", "Presentation"),
        ("LookAtEntity", "Presentation"),
        ("TriggerSetSoundState", "Presentation"),
        ("SetTadpoleTreeState", "Presentation"),
        ("MovedBy", "Movement"),
        ("Teleported", "Movement"),
        ("ItemTeleported", "Movement"),
    ];
    if let Some((_, s)) = EXACT.iter().find(|(n, _)| *n == name) {
        return s;
    }
    const KEYWORDS: &[(&str, &[&str])] = &[
        ("Flags", &["Flag"]),
        ("Timers", &["Timer"]),
        ("Statuses", &["Status", "Repose"]),
        ("Tags", &["Tag"]),
        (
            "Character creation",
            &["CharacterCreation", "SelectedForUser"],
        ),
        ("Tutorials", &["Tutorial"]),
        ("Crime", &["Crime", "Investigat"]),
        ("Dialog", &["Dialog", "Speaker", "Voice", "Bark"]),
        (
            "Math & strings",
            &["Integer", "Real", "Concatenate", "GUIDToString", "String"],
        ),
        (
            "Party & users",
            &[
                "Party",
                "User",
                "Player",
                "Follow",
                "Companion",
                "Host",
                "Control",
                "Avatar",
                "Summon",
                "MakeNPC",
                "Origin",
                "Reserved",
                "DLC",
                "Profile",
                "Hireling",
            ],
        ),
        (
            "Levels & game flow",
            &[
                "Level",
                "GameMode",
                "Movie",
                "AutoSave",
                "Module",
                "Ruleset",
                "GameState",
                "MapMarker",
                "Activity",
                "CharacterCreation",
            ],
        ),
        (
            "Combat, AI & perception",
            &[
                "Combat",
                "Died",
                "Dying",
                "Die",
                "Hitpoints",
                "Faction",
                "Relation",
                "Fight",
                "Attack",
                "Ai",
                "Flee",
                "Immortal",
                "Saw",
                "LostSightOf",
                "Reaction",
                "Swarm",
                "Roll",
                "Spell",
                "Passive",
                "Casted",
                "Obscured",
                "Capab",
                "Fall",
                "Fell",
                "Initiative",
                "Attitude",
                "Rest",
            ],
        ),
        (
            "Inventory & items",
            &[
                "AddedTo",
                "Equip",
                "Inventory",
                "Item",
                "Gold",
                "Weapon",
                "Stack",
                "Template",
                "Camp",
                "Supply",
                "PickUp",
                "Loot",
                "Trade",
                "Use",
                "Open",
                "Close",
                "Lock",
                "Sit",
                "Ladder",
                "Platform",
                "Pocket",
                "Owner",
                "Domain",
            ],
        ),
        (
            "Entities & templates",
            &[
                "Stage",
                "Activated",
                "Deactivated",
                "Visible",
                "Detached",
                "Create",
                "Transform",
                "Position",
                "Display",
                "Identity",
                "Trigger",
                "Waypoint",
                "Interact",
                "Dead",
                "Active",
                "Generics",
                "DefaultState",
                "CanJoin",
                "Hint",
            ],
        ),
        ("Movement", &["Teleport", "Move", "Platform"]),
    ];
    for (sub, keys) in KEYWORDS {
        if keys.iter().any(|k| name.contains(k)) {
            return sub;
        }
    }
    "Other"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_carry_no_identifiers() {
        let u = "1a2b3c4d-5e6f-4a1b-8c2d-3e4f5a6b7c8d";
        assert_eq!(
            abbreviate(&LogValue::parse(&format!("S_Hero_{u}"))),
            "S_Hero_…"
        );
        assert_eq!(
            abbreviate(&LogValue::parse(&format!("(CHARACTER)S_Hero_{u}"))),
            "(CHARACTER)S_Hero_…"
        );
        assert_eq!(abbreviate(&LogValue::parse(u)), "…");
        assert_eq!(
            abbreviate(&LogValue::parse(&format!("\"Timer_{u}3\""))),
            "\"Timer_…3\""
        );
        assert_eq!(scrub("h1a2b3c4dg5e6fg4a1bg8c2dg3e4f5a6b7c8d;1"), "h…;1");
        assert_eq!(scrub("plain"), "plain");
    }
}
