//! The smaller stats DSLs: boosts, requirements, use costs, functor contexts and description
//! parameters.

use crate::condition::Condition;
use crate::expr::{self, Expr};
use crate::functor::parse_call;
use crate::thoth::SyntaxError;

fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r'])
}

/// Split at `;` outside parentheses. Boost lists need this (`BlockRegainHP(Undead;Construct)`).
pub fn split_semicolons_nested(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth == 0 => {
                out.push(trim(&s[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(trim(&s[start..]));
    out.retain(|p| !p.is_empty());
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FieldError {
    #[error("'{text}': {message}")]
    Syntax { text: String, message: String },
    #[error("'{text}': condition: {error}")]
    Condition { text: String, error: SyntaxError },
    #[error("'{text}': expression: {error}")]
    Expr {
        text: String,
        error: expr::ParseError,
    },
}

fn syntax(text: &str, message: impl Into<String>) -> FieldError {
    FieldError::Syntax {
        text: text.to_owned(),
        message: message.into(),
    }
}

fn is_ident(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// One boost: `[IF(cond):]Name(args)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Boost {
    pub condition: Option<Condition>,
    pub name: String,
    /// Raw arguments; each boost type interprets its own (`eoc::ParseBoostData`).
    pub args: Vec<String>,
}

/// One item of a boost list.
#[derive(Debug, Clone, PartialEq)]
pub enum BoostEntry {
    Boost(Boost),
    /// Not a valid boost call (e.g. a functor context prefix like `AI_IGNORE:Tag(X)`); the
    /// engine can't map the name to a boost type.
    Dropped {
        text: String,
        reason: String,
    },
}

/// `Boosts`, `DefaultBoosts`, `BoostsOnEquipMainHand`, … — `;`-separated (outside
/// parentheses) boost calls, each optionally guarded by `IF(condition):`.
///
/// `(anonymous)::ParseBoosts` (0x10118d188, 4.1.1.7631656) splits at depth-0 `;`, calls
/// `eoc::ParseFunctorString` once for the item and once more after an `IF`, and recurses on
/// leftover text. So text after a call that isn't separated by `;` (`A(1):B(2)`, `A(1)B(2)`)
/// is read as further boosts. The control flow was only skimmed, not traced in full.
pub fn parse_boosts(s: &str) -> Result<Vec<BoostEntry>, FieldError> {
    let mut out = Vec::new();
    for item in split_semicolons_nested(s) {
        parse_boost_item(item, &mut out)?;
    }
    Ok(out)
}

fn parse_boost_item(item: &str, out: &mut Vec<BoostEntry>) -> Result<(), FieldError> {
    let Some(first) = parse_call(item) else {
        return Err(syntax(item, "unclosed '('"));
    };
    let (condition, call) = if first.name == "IF" {
        let Some(cond) = first.args.first() else {
            return Err(syntax(item, "IF without a condition"));
        };
        let c = Condition::parse(cond).map_err(|error| FieldError::Condition {
            text: item.to_owned(),
            error,
        })?;
        let Some(second) = parse_call(first.remainder) else {
            return Err(syntax(item, "unclosed '(' after IF"));
        };
        (Some(c), second)
    } else {
        (None, first)
    };
    if is_ident(call.name) {
        out.push(BoostEntry::Boost(Boost {
            condition,
            name: call.name.to_owned(),
            args: call.args.iter().map(|a| a.to_string()).collect(),
        }));
    } else {
        out.push(BoostEntry::Dropped {
            text: item.to_owned(),
            reason: format!("not a boost name: '{}'", call.name),
        });
    }
    if !call.remainder.is_empty() {
        for rest in split_semicolons_nested(call.remainder) {
            parse_boost_item(rest, out)?;
        }
    }
    Ok(())
}

/// One `Requirements` item: `[!]Name[ param]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub negate: bool,
    pub name: String,
    pub param: Option<String>,
}

/// `Requirements` (`!Combat;Immobile`).
// bg3: ParseRequirements (0x1020fd4fc, 4.1.1.7631656): split on `;`, trim, a leading `!`
// negates, text after a space is the parameter.
pub fn parse_requirements(s: &str) -> Result<Vec<Requirement>, FieldError> {
    let mut out = Vec::new();
    for item in s.split(';').map(trim).filter(|p| !p.is_empty()) {
        let (negate, rest) = match item.strip_prefix('!') {
            Some(r) => (true, r),
            None => (false, item),
        };
        let (name, param) = match rest.split_once(' ') {
            Some((n, p)) => (n, Some(trim(p).to_owned())),
            None => (rest, None),
        };
        if !is_ident(name) {
            return Err(syntax(item, "bad requirement name"));
        }
        out.push(Requirement {
            negate,
            name: name.to_owned(),
            param,
        });
    }
    Ok(out)
}

/// One use cost: `Resource:amount[:extra…]` (`SpellSlotsGroup:1:1:3` = group, amount, level).
#[derive(Debug, Clone, PartialEq)]
pub struct Cost {
    pub resource: String,
    pub amount: String,
    pub extra: Vec<String>,
}

/// `UseCosts`, `HitCosts`, `RitualCosts`, `Cost`, …
pub fn parse_costs(s: &str) -> Result<Vec<Cost>, FieldError> {
    let mut out = Vec::new();
    for item in s.split(';').map(trim).filter(|p| !p.is_empty()) {
        let mut parts = item.split(':').map(trim);
        let resource = parts.next().unwrap_or("");
        let Some(amount) = parts.next() else {
            return Err(syntax(item, "missing amount"));
        };
        if !is_ident(resource) {
            return Err(syntax(item, "bad resource name"));
        }
        // Amounts are numbers, except movement costs like `Movement:Distance*0.5` (a float
        // factor, which stats expressions can't express). Not traced in the binary.
        let formula = amount.split(['*', '/', '+', '-']).all(|t| {
            let t = trim(t);
            is_ident(t) || t.parse::<f64>().is_ok()
        });
        if amount.parse::<f64>().is_err() && !formula {
            return Err(syntax(item, "bad amount"));
        }
        out.push(Cost {
            resource: resource.to_owned(),
            amount: amount.to_owned(),
            extra: parts.map(str::to_owned).collect(),
        });
    }
    Ok(out)
}

/// `StatsFunctorContext`, `BoostContext`, …: `;`-separated event names (`OnDamage;OnCast`).
pub fn parse_name_list(s: &str) -> Result<Vec<String>, FieldError> {
    s.split(';')
        .map(trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            if is_ident(p) {
                Ok(p.to_owned())
            } else {
                Err(syntax(p, "bad name"))
            }
        })
        .collect()
}

/// One description parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum DescriptionParam {
    /// A functor-like call: `DealDamage(1d6, Fire)`, `Distance(3)`, `ApplyStatus(X,100,2)`.
    Call {
        name: String,
        args: Vec<String>,
    },
    /// A GUID (e.g. a tooltip resource).
    Guid(String),
    Expr(Expr),
    /// Anything else, kept as literal text (`50%`, `+4`, an unclosed call).
    Text(String),
}

fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.bytes().all(|c| c.is_ascii_hexdigit()))
}

/// `DescriptionParams`, `TooltipDamageList`, `TooltipStatusApply`, … — values that fill `[1]`,
/// `[2]`, … in loca text, `;`-separated (outside parentheses) and optionally grouped by text
/// key like functors (`…;Cast2[DealDamage(…)]`).
///
/// Each item is tried as a GUID, a stats expression, then a call; anything else is literal
/// text. The engine's own parser for these fields was not traced.
pub fn parse_description_params(
    s: &str,
) -> Result<Vec<(String, Vec<DescriptionParam>)>, FieldError> {
    let groups = crate::textkey::split(s).map_err(|e| syntax(s, e.to_string()))?;
    let mut out = Vec::new();
    for (key, content) in groups {
        let mut params = Vec::new();
        for item in split_semicolons_nested(&content) {
            if is_guid(item) {
                params.push(DescriptionParam::Guid(item.to_owned()));
                continue;
            }
            if let Ok(e) = expr::parse(item) {
                params.push(DescriptionParam::Expr(e));
                continue;
            }
            match parse_call(item).filter(|c| c.remainder.is_empty() && is_ident(c.name)) {
                Some(c) => params.push(DescriptionParam::Call {
                    name: c.name.to_owned(),
                    args: c.args.iter().map(|a| a.to_string()).collect(),
                }),
                None => params.push(DescriptionParam::Text(item.to_owned())),
            }
        }
        out.push((key, params));
    }
    Ok(out)
}
