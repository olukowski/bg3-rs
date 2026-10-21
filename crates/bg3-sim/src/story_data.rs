//! Install-backed data for the small story APIs and item interactions.
use std::collections::HashMap;

use bg3_lsf::{Node, Value};
use bg3_vfs::{Vfs, modules::BASE_LOAD_ORDER};

use crate::{
    Guid,
    load::{LoadError, game_objects, guid_attr, parse, s},
};

#[derive(Clone, Debug, Default)]
pub struct ObjectData {
    pub display_name: Option<String>,
    pub can_interact: Option<bool>,
    pub use_peace: Option<Vec<UseAction>>,
    pub use_combat: Option<Vec<UseAction>>,
}

/// The action type and its attributes, retained for a front end/action executor. Unknown
/// actions are not executed as if they succeeded.
#[derive(Clone, Debug, PartialEq)]
pub struct UseAction {
    pub kind: i32,
    pub attributes: Option<Node>,
}

#[derive(Clone, Debug, Default)]
pub struct OriginData {
    pub really: Vec<Guid>,
    pub background: Vec<Guid>,
    pub god: Vec<Guid>,
}

#[derive(Clone, Debug, Default)]
pub struct StoryData {
    /// Modifier defaults with the Balanced preset's ruleset overrides applied.
    pub ruleset: HashMap<Guid, String>,
    /// Root template and placed-object metadata, by MapKey.
    pub objects: HashMap<Guid, ObjectData>,
    /// Origin character's GlobalTemplate → character-creation tags (not current tags).
    pub origins: HashMap<Guid, OriginData>,
}

fn nodes<'a>(n: &'a Node, name: &str, out: &mut Vec<&'a Node>) {
    if n.name == name {
        out.push(n);
    }
    for c in &n.children {
        nodes(c, name, out);
    }
}

fn read_nodes(vfs: &Vfs, path: &str, name: &str) -> Result<Vec<Node>, LoadError> {
    if !vfs.exists(path) {
        return Ok(Vec::new());
    }
    let res = parse(vfs, path)?;
    let mut out = Vec::new();
    for r in &res.regions {
        nodes(&r.node, name, &mut out);
    }
    Ok(out.into_iter().cloned().collect())
}

fn tags(n: &Node, child: &str) -> Vec<Guid> {
    n.children_named(child)
        .filter_map(|n| guid_attr(n, "Object"))
        .collect()
}

pub(crate) fn object_data(n: &Node) -> ObjectData {
    let actions = |name| {
        n.child(name).map(|list| {
            list.children_named("Action")
                .filter_map(|a| {
                    Some(UseAction {
                        kind: a.attr("ActionType")?.as_i64()? as i32,
                        attributes: a.child("Attributes").cloned(),
                    })
                })
                .collect()
        })
    };
    ObjectData {
        display_name: match n.attr("DisplayName") {
            Some(Value::TranslatedString(t)) if !t.handle.is_empty() => Some(t.handle.clone()),
            _ => None,
        },
        can_interact: match n.attr("CanUse") {
            Some(Value::Bool(v)) => Some(*v),
            _ => None,
        },
        use_peace: actions("OnUsePeaceActions"),
        use_combat: actions("OnUseCombatActions"),
    }
}

impl StoryData {
    // bg3: esv::OsirisGameFunctions::GetRulesetModifierString (4.1.1.7631656)
    pub fn load(vfs: &Vfs, levels: &[String]) -> Result<Self, LoadError> {
        let mut out = Self::default();
        let mut rulesets = HashMap::new();
        let mut values = HashMap::new();
        let mut backgrounds = HashMap::new();
        let mut gods = HashMap::new();
        let mut origins = HashMap::new();
        for m in BASE_LOAD_ORDER {
            for n in read_nodes(vfs, &format!("Public/{m}/Ruleset/Rulesets.lsx"), "Ruleset")? {
                if let (Some(id), Some(name)) = (guid_attr(&n, "UUID"), s(&n, "Name")) {
                    rulesets.insert(id, name);
                }
            }
            for n in read_nodes(
                vfs,
                &format!("Public/{m}/Ruleset/RulesetModifiers.lsx"),
                "RulesetModifier",
            )? {
                if let (Some(id), Some(value)) = (guid_attr(&n, "UUID"), s(&n, "Default")) {
                    out.ruleset.insert(id, value);
                }
            }
            for n in read_nodes(
                vfs,
                &format!("Public/{m}/Ruleset/RulesetValues.lsx"),
                "RulesetValue",
            )? {
                if let (Some(r), Some(k), Some(v)) = (
                    guid_attr(&n, "Ruleset"),
                    guid_attr(&n, "Modifier"),
                    s(&n, "Value"),
                ) {
                    values.insert((r, k), v);
                }
            }
            for (kind, map) in [("Background", &mut backgrounds), ("God", &mut gods)] {
                for n in read_nodes(vfs, &format!("Public/{m}/{kind}s/{kind}s.lsx"), kind)? {
                    if let Some(id) = guid_attr(&n, "UUID") {
                        map.insert(id, tags(&n, "Tags"));
                    }
                }
            }
            for n in read_nodes(vfs, &format!("Public/{m}/Origins/Origins.lsx"), "Origin")? {
                if let Some(id) = guid_attr(&n, "GlobalTemplate").filter(|id| !id.is_null()) {
                    origins.insert(id, n);
                }
            }
            let p = format!("Public/{m}/RootTemplates/_merged.lsf");
            if vfs.exists(&p) {
                let res = parse(vfs, &p)?;
                for n in game_objects(&res) {
                    if let Some(id) = guid_attr(n, "MapKey") {
                        out.objects.insert(id, object_data(n));
                    }
                }
            }
            let prefix = format!("Mods/{m}/Globals/");
            let mut paths: Vec<String> = vfs
                .files_under(&prefix)
                .map(|f| f.entry.name.clone())
                .filter(|p| {
                    p.ends_with("/_merged.lsf")
                        && (p.contains("/Characters/") || p.contains("/Items/"))
                })
                .collect();
            for level in levels {
                for kind in ["Characters", "Items"] {
                    let p = format!("Mods/{m}/Levels/{level}/{kind}/_merged.lsf");
                    if vfs.exists(&p) {
                        paths.push(p);
                    }
                }
            }
            paths.sort();
            paths.dedup();
            for p in paths {
                let res = parse(vfs, &p)?;
                for n in game_objects(&res) {
                    if let Some(id) = guid_attr(n, "MapKey") {
                        out.objects.insert(id, object_data(n));
                    }
                }
            }
        }
        for name in ["RULESET_LARIAN", "DIFFICULTY_MEDIUM"] {
            for ((r, k), v) in &values {
                if rulesets.get(r).is_some_and(|s| s == name) {
                    out.ruleset.insert(*k, v.clone());
                }
            }
        }
        for (id, n) in origins {
            out.origins.insert(
                id,
                OriginData {
                    really: tags(&n, "ReallyTags"),
                    background: guid_attr(&n, "BackgroundUUID")
                        .and_then(|id| backgrounds.get(&id))
                        .cloned()
                        .unwrap_or_default(),
                    god: guid_attr(&n, "GodUUID")
                        .and_then(|id| gods.get(&id))
                        .cloned()
                        .unwrap_or_default(),
                },
            );
        }
        Ok(out)
    }
}
