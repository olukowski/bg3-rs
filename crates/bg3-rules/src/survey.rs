//! Parse every expression-valued field of a stats database and report counts, so coverage of
//! the languages can be checked against a real install.

use std::collections::BTreeMap;

use bg3_stats::Stats;

use crate::condition::{Condition, RollConditions};
use crate::expr;
use crate::fields;
use crate::functor::{Entry, FunctorList};

/// The language of an expression-valued field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Functors,
    Condition,
    RollConditions,
    Boosts,
    Requirements,
    FunctorContexts,
    Costs,
    DescriptionParams,
    Expression,
}

/// Which language a field uses, from its schema type (`Structure/Modifiers.txt`) and, for
/// fields typed `FixedString`, its name.
pub fn language_for(field: &str, value_type: &str) -> Option<Language> {
    Some(match value_type {
        "StatsFunctors" => Language::Functors,
        "Conditions" | "TargetConditions" => Language::Condition,
        "RollConditions" => Language::RollConditions,
        "StatsFunctorContext" => Language::FunctorContexts,
        "Requirements" | "MemorizationRequirements" | "SpellRequirement" => Language::Requirements,
        "FixedString" => match field {
            "Boosts" | "DefaultBoosts" | "BoostsOnEquipMainHand" | "BoostsOnEquipOffHand" => {
                Language::Boosts
            }
            "UseCosts"
            | "DualWieldingUseCosts"
            | "HitCosts"
            | "RitualCosts"
            | "Cost"
            | "TooltipUseCosts" => Language::Costs,
            "DescriptionParams"
            | "ExtraDescriptionParams"
            | "ShortDescriptionParams"
            | "TooltipUpcastDescriptionParams"
            | "TooltipDamageList"
            | "TooltipStatusApply"
            | "TooltipConditionalDamage"
            | "TooltipDamage" => Language::DescriptionParams,
            "Damage" | "VersatileDamage" | "AmountOfTargets" => Language::Expression,
            // `TargetConditions` is typed FixedString in one modifier list.
            "TargetConditions" => Language::Condition,
            _ => return None,
        },
        _ => return None,
    })
}

/// Per (language, field) tallies.
#[derive(Debug, Clone, Default)]
pub struct Tally {
    /// Non-empty values seen (each entry's effective value once).
    pub values: usize,
    pub ok: usize,
    /// Functor/boost items the engine would drop (unknown name, bad parameters).
    pub dropped_items: usize,
    pub failures: Vec<(String, String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub by_field: BTreeMap<(Language, String), Tally>,
    pub dropped: Vec<(String, String, String)>,
    pub functor_names: BTreeMap<String, usize>,
    pub condition_calls: BTreeMap<String, usize>,
    pub boost_names: BTreeMap<String, usize>,
    pub raw_variables: BTreeMap<String, usize>,
    /// Description parameters that are neither a call, a GUID nor an expression (kept as
    /// literal text).
    pub literal_params: Vec<(String, String)>,
}

impl Survey {
    pub fn total(&self, lang: Language) -> (usize, usize) {
        self.by_field
            .iter()
            .filter(|((l, _), _)| *l == lang)
            .fold((0, 0), |(v, ok), (_, t)| (v + t.values, ok + t.ok))
    }

    pub fn failures(&self) -> usize {
        self.by_field.values().map(|t| t.values - t.ok).sum()
    }
}

fn count_condition(s: &mut Survey, c: &Condition) {
    for n in c.called_functions() {
        *s.condition_calls.entry(n.to_string()).or_default() += 1;
    }
}

fn count_expr(s: &mut Survey, e: &expr::Expr) {
    e.walk(&mut |n| {
        if let expr::Expr::Var(v) = n
            && let expr::VarKind::Raw(r) = &v.kind
        {
            *s.raw_variables.entry(r.clone()).or_default() += 1;
        }
    });
}

/// Parse one value; `Err` is a failure message. Updates the name tallies.
fn parse_value(
    s: &mut Survey,
    entry: &str,
    field: &str,
    lang: Language,
    v: &str,
) -> Result<(), String> {
    match lang {
        Language::Functors => {
            let list = FunctorList::parse(v).map_err(|e| e.to_string())?;
            for e in list.entries() {
                match e {
                    Entry::Functor(f) => {
                        *s.functor_names.entry(f.name.clone()).or_default() += 1;
                        if let Some(c) = &f.condition {
                            count_condition(s, c);
                        }
                        match &f.params {
                            crate::functor::Params::DealDamage(d) => count_expr(s, &d.amount),
                            crate::functor::Params::ApplyStatus(a) => {
                                if let Some(c) = &a.condition {
                                    count_condition(s, c);
                                }
                            }
                            _ => {}
                        }
                    }
                    Entry::Dropped { text, reason } => {
                        s.by_field
                            .entry((lang, field.to_owned()))
                            .or_default()
                            .dropped_items += 1;
                        s.dropped
                            .push((entry.to_owned(), text.clone(), reason.clone()));
                    }
                }
            }
        }
        Language::Condition => {
            let c = Condition::parse(v).map_err(|e| e.to_string())?;
            count_condition(s, &c);
        }
        Language::RollConditions => {
            let r = RollConditions::parse(v).map_err(|e| e.to_string())?;
            for (_, c) in &r.groups {
                count_condition(s, c);
            }
        }
        Language::Boosts => {
            for b in fields::parse_boosts(v).map_err(|e| e.to_string())? {
                match b {
                    fields::BoostEntry::Boost(b) => {
                        *s.boost_names.entry(b.name.clone()).or_default() += 1;
                        if let Some(c) = &b.condition {
                            count_condition(s, c);
                        }
                    }
                    fields::BoostEntry::Dropped { text, reason } => {
                        s.by_field
                            .entry((lang, field.to_owned()))
                            .or_default()
                            .dropped_items += 1;
                        s.dropped.push((entry.to_owned(), text, reason));
                    }
                }
            }
        }
        Language::Requirements => {
            fields::parse_requirements(v).map_err(|e| e.to_string())?;
        }
        Language::FunctorContexts => {
            fields::parse_name_list(v).map_err(|e| e.to_string())?;
        }
        Language::Costs => {
            fields::parse_costs(v).map_err(|e| e.to_string())?;
        }
        Language::DescriptionParams => {
            for (_, params) in fields::parse_description_params(v).map_err(|e| e.to_string())? {
                for p in params {
                    match p {
                        fields::DescriptionParam::Expr(e) => count_expr(s, &e),
                        fields::DescriptionParam::Text(t) => {
                            s.literal_params.push((entry.to_owned(), t))
                        }
                        _ => {}
                    }
                }
            }
        }
        Language::Expression => {
            let e = expr::parse(v).map_err(|e| e.to_string())?;
            count_expr(s, &e);
        }
    }
    Ok(())
}

/// Parse every expression field of every `new entry` object.
///
/// Each object's own `data` lines are parsed (inherited values are parsed on the parent), so a
/// value is counted once per definition.
pub fn survey(stats: &Stats) -> Survey {
    let mut types: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for ml in stats.modifier_lists.iter() {
        for (field, ty) in &ml.attributes {
            types.insert((ml.name.as_str(), field.as_str()), ty.as_str());
        }
    }
    let mut s = Survey::default();
    for obj in stats.entries() {
        let Some(ty) = obj.type_name.as_deref() else {
            continue;
        };
        for (field, value) in &obj.own {
            let Some(vt) = types.get(&(ty, field.as_str())) else {
                continue;
            };
            let Some(lang) = language_for(field, vt) else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }
            let result = parse_value(&mut s, &obj.name, field, lang, value);
            let t = s.by_field.entry((lang, field.clone())).or_default();
            t.values += 1;
            match result {
                Ok(()) => t.ok += 1,
                Err(e) => t.failures.push((obj.name.clone(), value.clone(), e)),
            }
        }
    }
    s
}
