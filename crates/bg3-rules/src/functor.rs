//! Stats functors: the effect language of `SpellProperties`, `SpellSuccess`, `OnApplyFunctors`,
//! `TickFunctors`, … (field type `StatsFunctors`).
//!
//! ```text
//! list    = group ( ";" group )*            -- after text-key splitting (see textkey)
//! functor = ( context ":" )* [ "IF(" condition ")" [":"]* ] name "(" args ")"
//! ```
//!
//! Parsing follows the engine closely (4.1.1.7631656):
//! - each text-key group is split on **every** `;` (no nesting awareness), trimmed, empties
//!   skipped (`eoc::StatsFunctorListManager::CreateStatsFunctorList` 0x101fab168);
//! - a call is split by `eoc::ParseFunctorString` (0x1012b29d0): name = text before the first
//!   `(`, args at depth-1 commas (trimmed; an empty last arg is dropped), the rest after the
//!   closing `)` is a "remainder" stripped of whitespace, `,`, `:` and `;`;
//! - `eoc::StatsFunctorBase::ParseContext` (0x101fa30bc) splits the name at `:`; every piece but
//!   the last is a context flag (case-insensitive, unknown ones ignored);
//! - if there is a remainder and at least one argument, the first call was an `IF`: its first
//!   argument is the condition and the remainder is parsed again as the functor (its name is not
//!   context-split; the engine never checks that the first name is literally `IF`);
//! - an unknown name or a failed parameter parse makes the engine silently drop that functor —
//!   we keep it as [`Entry::Dropped`] with the reason;
//! - a leading `SELF`/`OWNER`/`SWAP`/`OBSERVER_*` argument selects the target
//!   (`eoc::StatsFunctorBase::Parse` 0x101fa2a20).

use std::fmt;

use crate::condition::Condition;
use crate::context::DamageType;
use crate::expr::{self, Expr};
use crate::textkey;

/// Functor names the engine knows (`GetStatsFunctorStrings` 0x101fa54ec builds 62 of them).
pub const FUNCTOR_NAMES: &[&str] = &[
    "ApplyStatus",
    "ApplyEquipmentStatus",
    "RemoveStatus",
    "RemoveUniqueStatus",
    "RemoveStatusByLevel",
    "RemoveAuraByChildStatus",
    "DealDamage",
    "RegainHitPoints",
    "RegainTemporaryHitPoints",
    "GainTemporaryHitPoints",
    "UseSpell",
    "UseAttack",
    "UseActionResource",
    "RestoreResource",
    "TutorialEvent",
    "TriggerRandomCast",
    "TeleportSource",
    "SwitchDeathType",
    "SwapPlaces",
    "SurfaceClearLayer",
    "SurfaceChange",
    "SummonInInventory",
    "Summon",
    "SpawnInInventory",
    "SpawnExtraProjectiles",
    "Spawn",
    "SetStatusDuration",
    "SetRoll",
    "SetReroll",
    "SetDamageResistance",
    "SetAdvantage",
    "SetDisadvantage",
    "Sabotage",
    "Resurrect",
    "ResetCooldowns",
    "ResetCombatTurn",
    "Pickup",
    "ModifySpellCameraFocus",
    "MaximizeRoll",
    "Force",
    "FireProjectile",
    "ExecuteWeaponFunctors",
    "Drop",
    "Douse",
    "DoTeleport",
    "DisarmWeapon",
    "DisarmAndStealWeapon",
    "CustomDescription",
    "CreateZone",
    "CreateWall",
    "CreateSurface",
    "CreateExplosion",
    "CreateConeSurface",
    "Counterspell",
    "CameraWait",
    "BreakConcentration",
    "AdjustRoll",
    "Unsummon",
    "Unlock",
    "Stabilize",
    "ShortRest",
    "Kill",
];

/// Functor context flags (`EStatsFunctorContext`), in bit order.
// bg3: bit assignment from eoc::StatsFunctorBase::ParseContext and the static init in
// __GLOBAL__sub_I_StatsFunctorEnums.cpp (4.1.1.7631656). Bit 25 is unused.
pub const CONTEXT_NAMES: &[(&str, u64)] = &[
    ("TARGET", 1 << 0),
    ("AOE", 1 << 1),
    ("CAST", 1 << 2),
    ("EQUIP", 1 << 3),
    ("GROUND", 1 << 4),
    ("LEAVE_ATTACK_RANGE", 1 << 5),
    ("ATTACKED_IN_MELEE_RANGE", 1 << 6),
    ("ATTACKING_IN_MELEE_RANGE", 1 << 7),
    ("PROFICIENCY_CHANGED", 1 << 8),
    ("STATUS_APPLY", 1 << 9),
    ("STATUS_APPLIED", 1 << 10),
    ("STATUS_REMOVE", 1 << 11),
    ("MOVED_DISTANCE", 1 << 12),
    ("AI_ONLY", 1 << 13),
    ("AI_IGNORE", 1 << 14),
    ("ATTACK", 1 << 15),
    ("ATTACKED", 1 << 16),
    ("DAMAGE", 1 << 17),
    ("HEAL", 1 << 18),
    ("STATUS_REMOVED", 1 << 19),
    ("OBSCURITY_CHANGED", 1 << 20),
    ("SHORT_REST", 1 << 21),
    ("DAMAGED", 1 << 22),
    ("HEALED", 1 << 23),
    ("ABILITY_CHECK", 1 << 24),
    ("LONG_REST", 1 << 26),
    ("CREATE", 1 << 27),
    ("PUSH", 1 << 28),
    ("PUSHED", 1 << 29),
    ("INVENTORY_CHANGED", 1 << 30),
    ("ENTER_ATTACK_RANGE", 1 << 31),
    ("PROJECTILE", 1 << 32),
    ("COMBAT_ENDED", 1 << 33),
    ("TURN", 1 << 34),
    ("ACTION_RESOURCES_CHANGED", 1 << 35),
    ("SURFACE_ENTER", 1 << 36),
    ("LOCKPICKING_SUCCEEDED", 1 << 37),
    ("INTERRUPT_USED", 1 << 38),
    ("DAMAGE_PREVENTED", 1 << 39),
    ("DAMAGED_PREVENTED", 1 << 40),
    ("ROUND", 1 << 41),
    ("COMBAT_STARTED", 1 << 42),
    ("CONSUMED", 1 << 43),
];

pub const CTX_TARGET: u64 = 1;
pub const CTX_AOE: u64 = 2;
pub const CTX_EQUIP: u64 = 1 << 3;
pub const CTX_GROUND: u64 = 1 << 4;
pub const CTX_AI_ONLY: u64 = 1 << 13;
pub const CTX_AI_IGNORE: u64 = 1 << 14;

/// Context flags of a functor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Contexts(pub u64);

impl Contexts {
    pub fn has(self, bit: u64) -> bool {
        self.0 & bit != 0
    }
}

impl fmt::Display for Contexts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = CONTEXT_NAMES
            .iter()
            .filter(|(_, b)| self.0 & b != 0)
            .map(|(n, _)| *n)
            .collect();
        write!(f, "{}", names.join("|"))
    }
}

/// Which entity a functor acts on, from a leading keyword argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetSelector {
    /// The current target of the pass.
    #[default]
    Target,
    /// `SELF`: the source.
    Source,
    /// `OWNER`.
    Owner,
    /// `SWAP`: source and target swapped.
    Swap,
    ObserverTarget,
    ObserverSource,
    ObserverObserver,
}

/// The weapon keyword in a `DealDamage` amount (`StringToDealDamageDamageRollValue`).
pub const WEAPON_KEYWORDS: &[&str] = &[
    "OffhandMeleeWeapon",
    "OffhandRangedWeapon",
    "MainMeleeWeapon",
    "MainRangedWeapon",
    "OffhandWeapon",
    "SourceWeapon",
    "ThrownWeapon",
    "MainWeapon",
];

/// `DealDamage` damage type argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageTypeArg {
    Type(DamageType),
    /// `MainMeleeWeaponDamageType` etc.: the weapon's own damage type (name without the
    /// `DamageType` suffix, e.g. `MainMeleeWeapon`).
    Weapon(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DealDamage {
    /// Amount; a weapon keyword has been replaced by `Placeholder0`.
    pub amount: Expr,
    /// The weapon keyword that `Placeholder0` stands for.
    pub weapon: Option<String>,
    pub damage_type: Option<DamageTypeArg>,
    pub magical: bool,
    pub nonlethal: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyStatus {
    pub status: String,
    /// Validated as a number by the engine; whether it is used is not confirmed.
    pub chance: Option<f64>,
    /// Duration in turns (a stats expression; `-1` = permanent).
    pub duration: Option<Expr>,
    pub param: Option<String>,
    pub condition: Option<Condition>,
}

/// Typed parameters for the functors the evaluator implements; others keep their raw args.
#[derive(Debug, Clone, PartialEq)]
pub enum Params {
    DealDamage(DealDamage),
    ApplyStatus(ApplyStatus),
    RemoveStatus { status: String },
    RegainHitPoints { amount: Expr },
    Raw,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Functor {
    /// Source text of this functor.
    pub text: String,
    pub contexts: Contexts,
    pub condition: Option<Condition>,
    pub target: TargetSelector,
    pub name: String,
    /// Arguments after the target keyword.
    pub args: Vec<String>,
    pub params: Params,
}

/// One `;`-separated item of a functor list.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Functor(Box<Functor>),
    /// The engine drops this item (unknown name, bad parameters); kept for diagnostics.
    Dropped {
        text: String,
        reason: String,
    },
}

/// A parsed functor field: one list per text key (`Default`, `Cast2`, `CastOffhand`, …).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctorList {
    pub groups: Vec<(String, Vec<Entry>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FunctorError {
    #[error(transparent)]
    TextKey(#[from] textkey::TextKeyError),
}

impl FunctorList {
    pub fn parse(s: &str) -> Result<Self, FunctorError> {
        let mut groups = Vec::new();
        for (key, content) in textkey::split(s)? {
            let entries = content
                .split(';')
                .map(trim)
                .filter(|p| !p.is_empty())
                .map(parse_functor)
                .collect();
            groups.push((key, entries));
        }
        Ok(Self { groups })
    }

    /// The default group's entries.
    pub fn default_group(&self) -> &[Entry] {
        self.group(textkey::DEFAULT_KEY)
    }

    pub fn group(&self, key: &str) -> &[Entry] {
        self.groups
            .iter()
            .find(|(k, _)| k == key)
            .map_or(&[], |(_, v)| v.as_slice())
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.groups.iter().flat_map(|(_, v)| v.iter())
    }

    pub fn functors(&self) -> impl Iterator<Item = &Functor> {
        self.entries().filter_map(|e| match e {
            Entry::Functor(f) => Some(&**f),
            Entry::Dropped { .. } => None,
        })
    }
}

fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r'])
}

/// Result of `eoc::ParseFunctorString`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call<'a> {
    pub name: &'a str,
    pub args: Vec<&'a str>,
    pub remainder: &'a str,
}

/// Split `name(arg, arg)rest` (see the module docs). `None` if a `(` is never closed.
pub fn parse_call(s: &str) -> Option<Call<'_>> {
    let Some(open) = s.find('(') else {
        // No call: the name runs to a `;` or the end.
        let end = s.find(';').unwrap_or(s.len());
        return Some(Call {
            name: trim(&s[..end]),
            args: Vec::new(),
            remainder: "",
        });
    };
    let name = trim(&s[..open]);
    let mut args = Vec::new();
    let mut depth = 1;
    let mut start = open + 1;
    for (i, c) in s.char_indices().skip_while(|(i, _)| *i <= open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let last = trim(&s[start..i]);
                    if !last.is_empty() {
                        args.push(last);
                    }
                    let remainder = s[i + 1..].trim_matches([' ', '\t', '\n', '\r', ',', ':', ';']);
                    return Some(Call {
                        name,
                        args,
                        remainder,
                    });
                }
            }
            ',' if depth == 1 => {
                args.push(trim(&s[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    None
}

/// Split a `CTX:CTX:Name` name into context flags and the name.
pub fn parse_context(name: &str) -> (Contexts, &str) {
    let mut pieces: Vec<&str> = name.split(':').collect();
    let last = pieces.pop().unwrap_or("");
    let mut bits = 0u64;
    for p in pieces {
        if let Some((_, b)) = CONTEXT_NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(p))
        {
            bits |= b;
        }
    }
    // No targeting context given (only AI flags or nothing): TARGET|AOE.
    if bits & !(CTX_AI_ONLY | CTX_AI_IGNORE) == 0 {
        bits |= CTX_TARGET | CTX_AOE;
    }
    (Contexts(bits), last)
}

fn dropped(text: &str, reason: impl Into<String>) -> Entry {
    Entry::Dropped {
        text: text.to_owned(),
        reason: reason.into(),
    }
}

/// Parse one functor (`eoc::StatsFunctorBase::ParseStatsFunctor` 0x101fa3f08).
pub fn parse_functor(text: &str) -> Entry {
    let Some(first) = parse_call(text) else {
        return dropped(text, "unclosed '('");
    };
    let (contexts, mut name) = parse_context(first.name);
    let mut args = first.args.clone();
    let mut condition = None;
    if !first.remainder.is_empty() && !first.args.is_empty() {
        match Condition::parse(first.args[0]) {
            Ok(c) => condition = Some(c),
            Err(e) => return dropped(text, format!("condition: {e}")),
        }
        let Some(second) = parse_call(first.remainder) else {
            return dropped(text, "unclosed '(' after IF");
        };
        name = second.name;
        args = second.args;
    }
    if !FUNCTOR_NAMES.contains(&name) {
        return dropped(text, format!("unknown functor '{name}'"));
    }
    let mut target = TargetSelector::Target;
    if let Some(&a0) = args.first() {
        let t = match a0 {
            "SELF" => Some(TargetSelector::Source),
            "OWNER" => Some(TargetSelector::Owner),
            "SWAP" => Some(TargetSelector::Swap),
            "OBSERVER_TARGET" => Some(TargetSelector::ObserverTarget),
            "OBSERVER_SOURCE" => Some(TargetSelector::ObserverSource),
            "OBSERVER_OBSERVER" => Some(TargetSelector::ObserverObserver),
            _ => None,
        };
        if let Some(t) = t {
            target = t;
            args.remove(0);
        }
    }
    let params = match parse_params(name, &args, contexts) {
        Ok(p) => p,
        Err(reason) => return dropped(text, reason),
    };
    Entry::Functor(Box::new(Functor {
        text: text.to_owned(),
        contexts,
        condition,
        target,
        name: name.to_owned(),
        args: args.iter().map(|s| s.to_string()).collect(),
        params,
    }))
}

fn parse_params(name: &str, args: &[&str], contexts: Contexts) -> Result<Params, String> {
    Ok(match name {
        "DealDamage" => Params::DealDamage(parse_deal_damage(args)?),
        "ApplyStatus" => Params::ApplyStatus(parse_apply_status(args, contexts)?),
        "RemoveStatus" => match args.first() {
            Some(s) => Params::RemoveStatus {
                status: s.to_string(),
            },
            None => return Err("RemoveStatus without a status".into()),
        },
        "RegainHitPoints" => match args.first() {
            Some(a) => Params::RegainHitPoints {
                amount: expr::parse(a).map_err(|e| format!("amount: {e}"))?,
            },
            None => return Err("RegainHitPoints without an amount".into()),
        },
        _ => Params::Raw,
    })
}

/// `DealDamage(amount, type, Magical, Nonlethal, int, _, bool, bool, bool, bool, roll)`.
// bg3: eoc::StatsFunctorDealDamage::Parse (0x101fb0cfc, 4.1.1.7631656)
fn parse_deal_damage(args: &[&str]) -> Result<DealDamage, String> {
    let Some(&raw) = args.first() else {
        return Err("DealDamage without an amount".into());
    };
    let mut amount_text = raw.to_owned();
    let mut weapon = None;
    for kw in WEAPON_KEYWORDS {
        if let Some(pos) = raw.find(kw) {
            amount_text = format!("{}Placeholder0{}", &raw[..pos], &raw[pos + kw.len()..]);
            weapon = Some(kw.to_string());
            break;
        }
    }
    let amount = expr::parse(&amount_text).map_err(|e| format!("amount: {e}"))?;
    let damage_type = match args.get(1) {
        None | Some(&"") => None,
        Some(t) => {
            if let Some(w) = t
                .strip_suffix("DamageType")
                .filter(|w| WEAPON_KEYWORDS.contains(w))
            {
                Some(DamageTypeArg::Weapon(w.to_owned()))
            } else if let Some(d) = DamageType::parse(t) {
                Some(DamageTypeArg::Type(d))
            } else {
                return Err(format!("unknown damage type '{t}'"));
            }
        }
    };
    if args.len() >= 5 && args[4].parse::<i64>().is_err() {
        return Err(format!("bad integer '{}'", args[4]));
    }
    Ok(DealDamage {
        amount,
        weapon,
        damage_type,
        magical: args
            .get(2)
            .is_some_and(|a| a.eq_ignore_ascii_case("Magical")),
        nonlethal: args
            .get(3)
            .is_some_and(|a| a.eq_ignore_ascii_case("Nonlethal")),
    })
}

/// `ApplyStatus(status, chance, duration, param, int, int, condition, bool)`.
// bg3: eoc::StatsFunctorStatus::ParseStatus (0x101fac62c, 4.1.1.7631656)
fn parse_apply_status(args: &[&str], contexts: Contexts) -> Result<ApplyStatus, String> {
    let Some(&status) = args.first() else {
        return Err("ApplyStatus without a status".into());
    };
    let mut out = ApplyStatus {
        status: status.to_owned(),
        chance: None,
        duration: None,
        param: None,
        condition: None,
    };
    // With 9+ arguments, or in an EQUIP-only context, only the status id is read.
    if args.len() >= 9 || contexts.0 == CTX_EQUIP {
        return Ok(out);
    }
    if let Some(c) = args.get(1) {
        out.chance = Some(c.parse().map_err(|_| format!("bad chance '{c}'"))?);
    }
    if let Some(d) = args.get(2).filter(|d| !d.is_empty()) {
        out.duration = Some(expr::parse(d).map_err(|e| format!("duration: {e}"))?);
    }
    out.param = args.get(3).filter(|p| !p.is_empty()).map(|p| p.to_string());
    if let Some(c) = args.get(6).filter(|c| !c.is_empty()) {
        out.condition = Some(Condition::parse(c).map_err(|e| format!("condition: {e}"))?);
    }
    Ok(out)
}
