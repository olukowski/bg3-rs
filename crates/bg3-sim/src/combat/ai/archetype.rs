//! AI archetypes: the modifier files `Public/<Module>/AI/Archetypes/*.txt`, read from the
//! install at runtime.
//!
//! An archetype is a named table of float modifiers (`SCORE_MOD`, `MULTIPLIER_KILL_ENEMY`,
//! …) that weights every part of the AI's scoring. Characters pick one with the `Archetype`
//! attribute of their template or level object (the Nautiloid imps: `imp_melee`,
//! `imp_ranged`). See `docs/research/ai.md` for the format and how the engine resolves it.
//!
//! Format, as the engine's loader reads it (*read*, see the citations below):
//!
//! - the archetype's name is the file name without directory and extension;
//! - per line, every space and tab is removed first, then everything from `//` on;
//! - a leading `+`, `-`, `*`, `/` or `=` is the operator (default: set);
//! - `USING<name>` names the parent archetype;
//! - otherwise the key runs up to the first digit, `.` or `-`, the rest is the value.
//!
//! Resolution: the parent's values (recursively; an archetype without `USING` builds on
//! `base`, and `base` on zeroes), then the archetype's own entries in file order. Files with
//! the same name from later modules, and from the folder of the active `AI_LETHALITY`
//! ruleset value (`AILETHALITY_BALANCED/` on Balanced), are merged into it: their entries are
//! appended and their `USING` replaces the parent.

use std::collections::HashMap;

use bg3_vfs::Vfs;

/// The `AI_LETHALITY` ruleset value of the Balanced difficulty (`RulesetValues.lsx`).
pub const DEFAULT_LETHALITY: &str = "AILETHALITY_BALANCED";

/// The archetype every other one builds on.
pub const BASE: &str = "base";

/// How an entry changes the inherited value.
// bg3: esv::ai::combat::AiModifiersSystem::Update (0x10507c568): entry ops 0 add, 1 subtract,
// 2 multiply, 3 divide, anything else sets (4.1.1.7631656).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Set,
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    pub fn apply(self, old: f32, v: f32) -> f32 {
        match self {
            Op::Set => v,
            Op::Add => old + v,
            Op::Sub => old - v,
            Op::Mul => old * v,
            Op::Div => old / v,
        }
    }
}

/// One `KEY value` line.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub op: Op,
    pub value: f32,
}

/// The (merged) definition of one archetype.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Definition {
    pub using: Option<String>,
    pub entries: Vec<Entry>,
}

/// The leading float of `s` (`atof`-style; trailing junk ignored).
fn leading_float(s: &str) -> Option<f32> {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let ok = c.is_ascii_digit()
            || c == '.'
            || ((c == '-' || c == '+') && i == 0)
            || ((c == 'e' || c == 'E') && i > 0);
        if !ok {
            break;
        }
        end = i + c.len_utf8();
    }
    (0..=end)
        .rev()
        .find_map(|n| s.get(..n).and_then(|p| p.parse::<f32>().ok()))
}

/// Parse one archetype file.
// bg3: (anonymous)::LoadModifiersFromDisk (0x1050807bc, 4.1.1.7631656)
pub fn parse(text: &str) -> Definition {
    let mut def = Definition::default();
    for line in text.lines() {
        let line: String = line.chars().filter(|c| *c != ' ' && *c != '\t').collect();
        let line = line.split("//").next().unwrap_or("").trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (op, rest) = match line.as_bytes()[0] {
            b'+' => (Op::Add, &line[1..]),
            b'-' => (Op::Sub, &line[1..]),
            b'*' => (Op::Mul, &line[1..]),
            b'/' => (Op::Div, &line[1..]),
            b'=' => (Op::Set, &line[1..]),
            _ => (Op::Set, line),
        };
        if let Some(parent) = rest.strip_prefix("USING") {
            if !parent.is_empty() {
                def.using = Some(parent.to_owned());
            }
            continue;
        }
        let Some(split) = rest.find(|c: char| c.is_ascii_digit() || c == '.' || c == '-') else {
            continue;
        };
        if split == 0 {
            continue;
        }
        if let Some(value) = leading_float(&rest[split..]) {
            def.entries.push(Entry {
                key: rest[..split].to_owned(),
                op,
                value,
            });
        }
    }
    def
}

/// All archetype definitions by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Archetypes {
    pub defs: HashMap<String, Definition>,
}

impl Archetypes {
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Merge a file into the archetype `name` (later files append; their `USING` wins).
    pub fn add(&mut self, name: &str, text: &str) {
        let parsed = parse(text);
        let d = self.defs.entry(name.to_owned()).or_default();
        if parsed.using.is_some() {
            d.using = parsed.using;
        }
        d.entries.extend(parsed.entries);
    }

    /// Load `Public/<module>/AI/Archetypes/*.txt` for each module, each followed by the
    /// files of the `lethality` folder (`AI_LETHALITY` ruleset value).
    pub fn load(vfs: &Vfs, modules: &[&str], lethality: &str) -> Self {
        let mut out = Self::default();
        for m in modules {
            for dir in [
                format!("Public/{m}/AI/Archetypes/"),
                format!("Public/{m}/AI/Archetypes/{lethality}/"),
            ] {
                let mut files: Vec<String> = vfs
                    .files_under(&dir)
                    .map(|f| f.entry.name.clone())
                    .filter(|n| {
                        n.len() > dir.len() && !n[dir.len()..].contains('/') && n.ends_with(".txt")
                    })
                    .collect();
                files.sort();
                for f in files {
                    let Ok(bytes) = vfs.read(&f) else {
                        continue;
                    };
                    let stem = f[dir.len()..].trim_end_matches(".txt").to_owned();
                    out.add(&stem, &String::from_utf8_lossy(&bytes));
                }
            }
        }
        out
    }

    /// The resolved modifiers of an archetype (unknown names resolve as `base`).
    pub fn resolve(&self, name: &str) -> Modifiers {
        let mut chain: Vec<&str> = Vec::new();
        let mut cur = if self.defs.contains_key(name) {
            name
        } else {
            BASE
        };
        while chain.len() < 16 && !chain.contains(&cur) {
            chain.push(cur);
            let Some(d) = self.defs.get(cur) else {
                break;
            };
            cur = match &d.using {
                Some(p) => p.as_str(),
                None if cur != BASE => BASE,
                None => break,
            };
        }
        let mut values: HashMap<String, f32> = HashMap::new();
        for n in chain.iter().rev() {
            if let Some(d) = self.defs.get(*n) {
                for e in &d.entries {
                    let v = values.entry(e.key.clone()).or_insert(0.0);
                    *v = e.op.apply(*v, e.value);
                }
            }
        }
        Modifiers {
            archetype: chain.first().copied().unwrap_or(BASE).to_owned(),
            values,
        }
    }
}

/// The resolved modifier values of one archetype; missing keys read as 0 (as the engine's
/// zero-initialised table).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Modifiers {
    pub archetype: String,
    pub values: HashMap<String, f32>,
}

impl Modifiers {
    pub fn get(&self, key: &str) -> f32 {
        self.values.get(key).copied().unwrap_or(0.0)
    }

    pub fn set(&mut self, key: &str, v: f32) {
        self.values.insert(key.to_owned(), v);
    }

    /// Our own neutral table for sims without the install's archetypes (synthetic tests):
    /// a score unit of 1, every relation multiplier 1, harm to allies and neutrals negative,
    /// hit chances taken at face value. Not game data.
    pub fn neutral() -> Self {
        let mut m = Self {
            archetype: "(neutral)".to_owned(),
            values: HashMap::new(),
        };
        m.set(SCORE_MOD, 1.0);
        for kind in ["DAMAGE", "HEAL", "DOT", "HOT", "CONTROL", "BOOST"] {
            for who in ["SELF", "ENEMY", "ALLY", "NEUTRAL"] {
                for sign in ["POS", "NEG"] {
                    m.set(&format!("MULTIPLIER_{kind}_{who}_{sign}"), 1.0);
                }
            }
        }
        for k in [
            MULTIPLIER_KILL_ENEMY,
            MULTIPLIER_KILL_ENEMY_SUMMON,
            MULTIPLIER_KILL_ALLY,
            MULTIPLIER_KILL_ALLY_SUMMON,
            MULTIPLIER_TARGET_MY_ENEMY,
            MULTIPLIER_TARGET_MY_HOSTILE,
            MULTIPLIER_TARGET_SUMMON,
            MULTIPLIER_TARGET_HOSTILE_COUNT_ONE,
            MULTIPLIER_TARGET_HOSTILE_COUNT_TWO_OR_MORE,
            MULTIPLIER_TARGET_IN_SIGHT,
            MULTIPLIER_TARGET_ENEMY_DOWNED,
            MULTIPLIER_TARGET_ALLY_DOWNED,
            MULTIPLIER_TARGET_NEUTRAL_DOWNED,
            MULTIPLIER_TARGET_PREFERRED,
            MULTIPLIER_TARGET_UNPREFERRED,
            MULTIPLIER_CANNOT_EXECUTE_THIS_TURN,
            MULTIPLIER_RESISTANCE_STUPIDITY,
            MULTIPLIER_RESOURCE_REPLENISH_TYPE_TURN,
            ENABLE_MOVEMENT_AVOID_AOO,
        ] {
            m.set(k, 1.0);
        }
        m.set(MULTIPLIER_SCORE_ON_ALLY, -1.0);
        m.set(MULTIPLIER_SCORE_ON_NEUTRAL, -1.0);
        m
    }
}

// The modifier names the scoring reads (the engine's `EAiModifier` names, as in the files).
pub const SCORE_MOD: &str = "SCORE_MOD";
pub const BASE_NEARBY_SCORE: &str = "BASE_NEARBY_SCORE";
pub const MULTIPLIER_FREE_ACTION: &str = "MULTIPLIER_FREE_ACTION";
pub const ENABLE_MOVEMENT_AVOID_AOO: &str = "ENABLE_MOVEMENT_AVOID_AOO";
pub const MULTIPLIER_COOLDOWN_MULTIPLIER: &str = "MULTIPLIER_COOLDOWN_MULTIPLIER";
pub const MULTIPLIER_CANNOT_EXECUTE_THIS_TURN: &str = "MULTIPLIER_CANNOT_EXECUTE_THIS_TURN";
pub const MULTIPLIER_ACTION_RESOURCE_COST: &str = "MULTIPLIER_ACTION_RESOURCE_COST";
pub const MULTIPLIER_TARGET_MY_ENEMY: &str = "MULTIPLIER_TARGET_MY_ENEMY";
pub const MULTIPLIER_TARGET_MY_HOSTILE: &str = "MULTIPLIER_TARGET_MY_HOSTILE";
pub const MULTIPLIER_TARGET_SUMMON: &str = "MULTIPLIER_TARGET_SUMMON";
pub const MULTIPLIER_TARGET_HOSTILE_COUNT_ONE: &str = "MULTIPLIER_TARGET_HOSTILE_COUNT_ONE";
pub const MULTIPLIER_TARGET_HOSTILE_COUNT_TWO_OR_MORE: &str =
    "MULTIPLIER_TARGET_HOSTILE_COUNT_TWO_OR_MORE";
pub const MULTIPLIER_TARGET_IN_SIGHT: &str = "MULTIPLIER_TARGET_IN_SIGHT";
pub const MULTIPLIER_TARGET_INCAPACITATED: &str = "MULTIPLIER_TARGET_INCAPACITATED";
pub const MULTIPLIER_TARGET_KNOCKED_DOWN: &str = "MULTIPLIER_TARGET_KNOCKED_DOWN";
pub const MULTIPLIER_TARGET_PREFERRED: &str = "MULTIPLIER_TARGET_PREFERRED";
pub const MULTIPLIER_TARGET_UNPREFERRED: &str = "MULTIPLIER_TARGET_UNPREFERRED";
pub const MULTIPLIER_TARGET_HEALTH_BIAS: &str = "MULTIPLIER_TARGET_HEALTH_BIAS";
pub const MULTIPLIER_TARGET_ENEMY_DOWNED: &str = "MULTIPLIER_TARGET_ENEMY_DOWNED";
pub const MULTIPLIER_TARGET_ALLY_DOWNED: &str = "MULTIPLIER_TARGET_ALLY_DOWNED";
pub const MULTIPLIER_TARGET_NEUTRAL_DOWNED: &str = "MULTIPLIER_TARGET_NEUTRAL_DOWNED";
pub const MULTIPLIER_ENDPOS_ALLIES_NEARBY: &str = "MULTIPLIER_ENDPOS_ALLIES_NEARBY";
pub const ENDPOS_ALLIES_NEARBY_MIN_DISTANCE: &str = "ENDPOS_ALLIES_NEARBY_MIN_DISTANCE";
pub const ENDPOS_ALLIES_NEARBY_MAX_DISTANCE: &str = "ENDPOS_ALLIES_NEARBY_MAX_DISTANCE";
pub const MULTIPLIER_ENDPOS_ENEMIES_NEARBY: &str = "MULTIPLIER_ENDPOS_ENEMIES_NEARBY";
pub const ENDPOS_ENEMIES_NEARBY_MIN_DISTANCE: &str = "ENDPOS_ENEMIES_NEARBY_MIN_DISTANCE";
pub const ENDPOS_ENEMIES_NEARBY_MAX_DISTANCE: &str = "ENDPOS_ENEMIES_NEARBY_MAX_DISTANCE";
pub const MAX_DISTANCE_TO_CLOSEST_ENEMY: &str = "MAX_DISTANCE_TO_CLOSEST_ENEMY";
pub const MULTIPLIER_NO_ENEMIES_IN_MAX_DISTANCE: &str = "MULTIPLIER_NO_ENEMIES_IN_MAX_DISTANCE";
pub const MULTIPLIER_FALLBACK_ALLIES_NEARBY: &str = "MULTIPLIER_FALLBACK_ALLIES_NEARBY";
pub const FALLBACK_ALLIES_NEARBY_MIN_DISTANCE: &str = "FALLBACK_ALLIES_NEARBY_MIN_DISTANCE";
pub const FALLBACK_ALLIES_NEARBY_MAX_DISTANCE: &str = "FALLBACK_ALLIES_NEARBY_MAX_DISTANCE";
pub const MULTIPLIER_FALLBACK_ENEMIES_NEARBY: &str = "MULTIPLIER_FALLBACK_ENEMIES_NEARBY";
pub const FALLBACK_ENEMIES_NEARBY_MIN_DISTANCE: &str = "FALLBACK_ENEMIES_NEARBY_MIN_DISTANCE";
pub const FALLBACK_ENEMIES_NEARBY_MAX_DISTANCE: &str = "FALLBACK_ENEMIES_NEARBY_MAX_DISTANCE";
pub const MULTIPLIER_SCORE_ON_NEUTRAL: &str = "MULTIPLIER_SCORE_ON_NEUTRAL";
pub const MULTIPLIER_SCORE_ON_ALLY: &str = "MULTIPLIER_SCORE_ON_ALLY";
pub const MULTIPLIER_SCORE_OUT_OF_COMBAT: &str = "MULTIPLIER_SCORE_OUT_OF_COMBAT";
pub const MAX_HEAL_MULTIPLIER: &str = "MAX_HEAL_MULTIPLIER";
pub const MAX_HEAL_SELF_MULTIPLIER: &str = "MAX_HEAL_SELF_MULTIPLIER";
pub const MULTIPLIER_KILL_ENEMY: &str = "MULTIPLIER_KILL_ENEMY";
pub const MULTIPLIER_KILL_ENEMY_SUMMON: &str = "MULTIPLIER_KILL_ENEMY_SUMMON";
pub const MULTIPLIER_KILL_ALLY: &str = "MULTIPLIER_KILL_ALLY";
pub const MULTIPLIER_KILL_ALLY_SUMMON: &str = "MULTIPLIER_KILL_ALLY_SUMMON";
pub const MULTIPLIER_KILL_TARGET_HEALTH_BIAS: &str = "MULTIPLIER_KILL_TARGET_HEALTH_BIAS";
pub const INSTAKILL_BASE_SCORE: &str = "INSTAKILL_BASE_SCORE";
pub const MULTIPLIER_INSTAKILL_TARGET_HEALTH_BIAS: &str = "MULTIPLIER_INSTAKILL_TARGET_HEALTH_BIAS";
pub const MULTIPLIER_RESISTANCE_STUPIDITY: &str = "MULTIPLIER_RESISTANCE_STUPIDITY";
pub const MULTIPLIER_IMMUNITY_STUPIDITY: &str = "MULTIPLIER_IMMUNITY_STUPIDITY";
pub const BUFF_DIST_MAX: &str = "BUFF_DIST_MAX";
pub const BUFF_DIST_MIN: &str = "BUFF_DIST_MIN";
pub const MULTIPLIER_RESOURCE_REPLENISH_TYPE_NEVER: &str =
    "MULTIPLIER_RESOURCE_REPLENISH_TYPE_NEVER";
pub const MULTIPLIER_RESOURCE_REPLENISH_TYPE_COMBAT: &str =
    "MULTIPLIER_RESOURCE_REPLENISH_TYPE_COMBAT";
pub const MULTIPLIER_RESOURCE_REPLENISH_TYPE_REST: &str = "MULTIPLIER_RESOURCE_REPLENISH_TYPE_REST";
pub const MULTIPLIER_RESOURCE_REPLENISH_TYPE_SHORT_REST: &str =
    "MULTIPLIER_RESOURCE_REPLENISH_TYPE_SHORT_REST";
pub const MULTIPLIER_RESOURCE_REPLENISH_TYPE_TURN: &str = "MULTIPLIER_RESOURCE_REPLENISH_TYPE_TURN";
pub const MODIFIER_HIT_CHANCE_STUPIDITY: &str = "MODIFIER_HIT_CHANCE_STUPIDITY";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operators_using_and_comments() {
        let d = parse(
            "// comment\nUSING melee\n\nMULTIPLIER_KILL_ENEMY \t\t 0.90 // trailing\n\
             - MODIFIER_HIT_CHANCE_STUPIDITY 0.15\n+MULTIPLIER_X 0.25\n\
             MULTIPLIER_SCORE_ON_ALLY -1.1\nTURNS_CAP 4.0\r\nNO_VALUE\n",
        );
        assert_eq!(d.using.as_deref(), Some("melee"));
        let e: Vec<(&str, Op, f32)> = d
            .entries
            .iter()
            .map(|e| (e.key.as_str(), e.op, e.value))
            .collect();
        assert_eq!(
            e,
            [
                ("MULTIPLIER_KILL_ENEMY", Op::Set, 0.9),
                ("MODIFIER_HIT_CHANCE_STUPIDITY", Op::Sub, 0.15),
                ("MULTIPLIER_X", Op::Add, 0.25),
                ("MULTIPLIER_SCORE_ON_ALLY", Op::Set, -1.1),
                ("TURNS_CAP", Op::Set, 4.0),
            ]
        );
    }

    #[test]
    fn resolves_through_parents_and_merges_overrides() {
        let mut a = Archetypes::default();
        a.add(
            "base",
            "SCORE_MOD 100\nMODIFIER_HIT_CHANCE_STUPIDITY 1.0\nMULTIPLIER_A 2",
        );
        a.add(
            "melee",
            "- MODIFIER_HIT_CHANCE_STUPIDITY 0.15\n* MULTIPLIER_A 3",
        );
        a.add("imp", "USING melee\nMULTIPLIER_KILL_ENEMY 0.9");
        // A difficulty folder file of the same name is merged into `base`.
        a.add("base", "MODIFIER_HIT_CHANCE_STUPIDITY 0.65");
        let m = a.resolve("imp");
        assert_eq!(m.archetype, "imp");
        assert_eq!(m.get(SCORE_MOD), 100.0);
        assert!((m.get(MODIFIER_HIT_CHANCE_STUPIDITY) - 0.5).abs() < 1e-6);
        assert_eq!(m.get("MULTIPLIER_A"), 6.0);
        assert_eq!(m.get(MULTIPLIER_KILL_ENEMY), 0.9);
        assert_eq!(m.get("UNKNOWN"), 0.0);
        assert_eq!(a.resolve("nope").archetype, "base");
    }

    #[test]
    fn a_using_cycle_terminates() {
        let mut a = Archetypes::default();
        a.add("x", "USING y\nK 1");
        a.add("y", "USING x\n+K 1");
        // y (+1 on 0) first, then x sets 1.
        assert_eq!(a.resolve("x").get("K"), 1.0);
    }
}
