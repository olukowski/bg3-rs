//! Stats expressions: the arithmetic formula language of damage, healing, durations and
//! description parameters (`1d8+StrengthModifier`, `max(1, 2d6)`, `(8d6)/2`,
//! `LevelMapValue(SneakAttack)`, …).
//!
//! The parser mirrors `eoc::StatsExpressionObject::Parse` (0x101f84b70, 4.1.1.7631656): no
//! tokenizer; the text is trimmed, redundant outer parentheses are stripped, and then each node
//! kind is tried in a fixed order. Binary operators split at the **first** operator at nesting
//! depth 0, so `+` binds loosest, then `-`, `/`, `*`, and chains are right-associative
//! (`a-b-c` = `a-(b-c)`, `a/b*c` = `a/(b*c)`). These quirks are deliberate.

use std::fmt;

use crate::context::Ability;
use crate::dice::{DiceRng, RollDefinition};

/// Integer arithmetic is 32-bit, like the engine's `OperatorAdd/Substract/Multiply/Divide`.
pub type Number = i32;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Roll(RollDefinition),
    /// The literal `ImprovisedWeapon` (a roll flagged as improvised-weapon damage).
    ImprovisedWeapon,
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Max(Vec<Expr>),
    /// `foreach(count, body)`: `body` rolled `count` times.
    ForEach(Box<Expr>, Box<Expr>),
    /// `PlaceholderN`; `DealDamage` substitutes weapon keywords with `Placeholder0`.
    Placeholder(u32),
    ResourceRoll {
        name: String,
        level: u8,
    },
    Var(Variable),
    Neg(Box<Expr>),
}

/// Whose value a variable reads (`EStatsContextType`): the default entity, or prefixed
/// `Target.`, `Owner.`, `Cause.`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VarContext {
    #[default]
    Default,
    Target,
    Owner,
    Cause,
}

/// Built-in variable names (`EStatsExpressionVariableData`, values 0–10 in this order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinVar {
    SpellCastingAbility,
    ProficiencyBonus,
    Level,
    SpellDC,
    WeaponActionDC,
    UnarmedMeleeAbility,
    CurrentHP,
    MaxHP,
    SpellPowerLevel,
    TadpolePowersCount,
    DamageDone,
}

impl BuiltinVar {
    pub const ALL: [BuiltinVar; 11] = [
        BuiltinVar::SpellCastingAbility,
        BuiltinVar::ProficiencyBonus,
        BuiltinVar::Level,
        BuiltinVar::SpellDC,
        BuiltinVar::WeaponActionDC,
        BuiltinVar::UnarmedMeleeAbility,
        BuiltinVar::CurrentHP,
        BuiltinVar::MaxHP,
        BuiltinVar::SpellPowerLevel,
        BuiltinVar::TadpolePowersCount,
        BuiltinVar::DamageDone,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BuiltinVar::SpellCastingAbility => "SpellCastingAbility",
            BuiltinVar::ProficiencyBonus => "ProficiencyBonus",
            BuiltinVar::Level => "Level",
            BuiltinVar::SpellDC => "SpellDC",
            BuiltinVar::WeaponActionDC => "WeaponActionDC",
            BuiltinVar::UnarmedMeleeAbility => "UnarmedMeleeAbility",
            BuiltinVar::CurrentHP => "CurrentHP",
            BuiltinVar::MaxHP => "MaxHP",
            BuiltinVar::SpellPowerLevel => "SpellPowerLevel",
            BuiltinVar::TadpolePowersCount => "TadpolePowersCount",
            BuiltinVar::DamageDone => "DamageDone",
        }
    }
}

/// D&D skills (`ESkill`), accepted as variable names.
pub const SKILLS: &[&str] = &[
    "Deception",
    "Intimidation",
    "Performance",
    "Persuasion",
    "Acrobatics",
    "SleightOfHand",
    "Stealth",
    "Arcana",
    "History",
    "Investigation",
    "Nature",
    "Religion",
    "Athletics",
    "AnimalHandling",
    "Insight",
    "Medicine",
    "Perception",
    "Survival",
];

#[derive(Debug, Clone, PartialEq)]
pub enum VarKind {
    Builtin(BuiltinVar),
    Ability(Ability),
    Skill(String),
    LevelMapValue(String),
    ClassLevel(String),
    /// Anything else (status groups, `X.Duration`-style references, …): kept by name, as the
    /// engine keeps unrecognised names as strings.
    Raw(String),
}

/// Suffix flags (`EStatsExpressionVariableDataModifier`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VarModifiers {
    pub modifier: bool,
    pub flat: bool,
    pub saving_throw: bool,
    pub dialogue_check: bool,
    pub advantage: bool,
    pub disadvantage: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub context: VarContext,
    pub kind: VarKind,
    pub modifiers: VarModifiers,
    /// The text as written.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("empty expression")]
    Empty,
    #[error("cannot parse '{0}'")]
    Unrecognised(String),
    #[error("bad arguments in '{0}'")]
    BadArgs(String),
}

/// Engine whitespace: space, tab, LF, CR only.
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r'])
}

/// Split at the first `op` at nesting depth 0 (`(`/`[` open, `)`/`]` close). `None` if there
/// is none or it is the first character.
// bg3: StatsExpressionObjectOperator::ParseLeftAndRight (0x101f8bd8c, 4.1.1.7631656)
fn split_first(s: &str, op: u8) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    for (i, c) in s.bytes().enumerate() {
        match c {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            c if c == op && depth == 0 => {
                return (i > 0).then(|| (&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Split a call's argument list at depth-0 commas.
pub fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.bytes().enumerate() {
        match c {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// `Name(inner)` → `inner`, if `s` is exactly that.
fn call_inner<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    s.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')
}

/// Result of one node parser: `Ok(None)` = "not mine", try the next kind.
type Try = Result<Option<Expr>, ParseError>;

/// Parse a stats expression.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let mut s = trim(src);
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    // Redundant outer parentheses are stripped without checking that they match each other
    // (as the engine does).
    while s.len() >= 2 && s.starts_with('(') && s.ends_with(')') {
        s = trim(&s[1..s.len() - 1]);
    }
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let parsers: [fn(&str) -> Try; 11] = [
        try_roll,
        |s| try_binop(s, b'+', Expr::Add),
        |s| try_binop(s, b'-', Expr::Sub),
        |s| try_binop(s, b'/', Expr::Div),
        |s| try_binop(s, b'*', Expr::Mul),
        try_max,
        try_foreach,
        try_placeholder,
        try_resource_roll,
        try_variable,
        try_negate,
    ];
    for p in parsers {
        if let Some(e) = p(s)? {
            return Ok(e);
        }
    }
    Err(ParseError::Unrecognised(s.to_owned()))
}

fn try_roll(s: &str) -> Try {
    if s.eq_ignore_ascii_case("ImprovisedWeapon") {
        return Ok(Some(Expr::ImprovisedWeapon));
    }
    Ok(RollDefinition::parse(s).map(Expr::Roll))
}

fn try_binop(s: &str, op: u8, make: fn(Box<Expr>, Box<Expr>) -> Expr) -> Try {
    let Some((l, r)) = split_first(s, op) else {
        return Ok(None);
    };
    Ok(Some(make(Box::new(parse(l)?), Box::new(parse(r)?))))
}

fn try_max(s: &str) -> Try {
    let Some(inner) = call_inner(s, "max") else {
        return Ok(None);
    };
    let args = split_args(inner)
        .into_iter()
        .map(parse)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(Expr::Max(args)))
}

fn try_foreach(s: &str) -> Try {
    let Some(inner) = call_inner(s, "foreach") else {
        return Ok(None);
    };
    let args = split_args(inner);
    if args.len() != 2 {
        return Err(ParseError::BadArgs(s.to_owned()));
    }
    Ok(Some(Expr::ForEach(
        Box::new(parse(args[0])?),
        Box::new(parse(args[1])?),
    )))
}

fn try_placeholder(s: &str) -> Try {
    let Some(rest) = s.strip_prefix("Placeholder") else {
        return Ok(None);
    };
    if rest.is_empty() {
        return Ok(Some(Expr::Placeholder(0)));
    }
    Ok(rest.parse().ok().map(Expr::Placeholder))
}

fn try_resource_roll(s: &str) -> Try {
    let Some(inner) = call_inner(s, "ResourceRoll") else {
        return Ok(None);
    };
    let args = split_args(inner);
    let level = args.get(1).and_then(|l| trim(l).parse::<u8>().ok());
    match (args.len(), level) {
        (2, Some(level)) => Ok(Some(Expr::ResourceRoll {
            name: trim(args[0]).to_owned(),
            level,
        })),
        _ => Err(ParseError::BadArgs(s.to_owned())),
    }
}

fn is_ident(s: &str) -> bool {
    let mut parts = s.split('.');
    let first = parts.next().unwrap_or("");
    let ok_first = first
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && first
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_');
    ok_first
        && parts.all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
}

// bg3: eoc::StatsExpressionObjectVariable::Parse (0x101f97f88, 4.1.1.7631656): suffixes, then
// context prefixes, then LevelMapValue/ClassLevel, built-in names, abilities, skills, and
// finally a raw name.
fn try_variable(s: &str) -> Try {
    let text = s.to_owned();
    let mut name = s;
    let mut modifiers = VarModifiers::default();
    for suffix in [
        "Modifier",
        "Flat",
        "SavingThrow",
        "DialogueCheck",
        "Disadvantage",
        "Advantage",
    ] {
        if let Some(n) = name.strip_suffix(suffix)
            && !n.is_empty()
            && !n.ends_with('(')
            && !n.ends_with(')')
        {
            name = n;
            match suffix {
                "Modifier" => modifiers.modifier = true,
                "Flat" => modifiers.flat = true,
                "SavingThrow" => modifiers.saving_throw = true,
                "DialogueCheck" => modifiers.dialogue_check = true,
                "Disadvantage" => modifiers.disadvantage = true,
                _ => modifiers.advantage = true,
            }
            break;
        }
    }
    let mut context = VarContext::Default;
    for (prefix, c) in [
        ("Target.", VarContext::Target),
        ("Owner.", VarContext::Owner),
        ("Cause.", VarContext::Cause),
    ] {
        if let Some(n) = name.strip_prefix(prefix) {
            name = n;
            context = c;
            break;
        }
    }
    let kind = if let Some(inner) = call_inner(name, "LevelMapValue") {
        VarKind::LevelMapValue(trim(inner).to_owned())
    } else if let Some(inner) = call_inner(name, "ClassLevel") {
        VarKind::ClassLevel(trim(inner).to_owned())
    } else if let Some(b) = BuiltinVar::ALL.iter().find(|b| b.name() == name) {
        VarKind::Builtin(*b)
    } else if let Some(a) = Ability::parse(name) {
        VarKind::Ability(a)
    } else if SKILLS.contains(&name) {
        VarKind::Skill(name.to_owned())
    } else if is_ident(name) {
        VarKind::Raw(name.to_owned())
    } else {
        return Ok(None);
    };
    Ok(Some(Expr::Var(Variable {
        context,
        kind,
        modifiers,
        text,
    })))
}

fn try_negate(s: &str) -> Try {
    match s.strip_prefix('-') {
        Some(rest) => Ok(Some(Expr::Neg(Box::new(parse(rest)?)))),
        None => Ok(None),
    }
}

impl Expr {
    /// Whether this node is a dice roll (`Roll` node), used by `max` tie-breaking.
    fn is_roll(&self) -> bool {
        matches!(self, Expr::Roll(_) | Expr::ImprovisedWeapon)
    }

    /// Visit every node.
    pub fn walk(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Add(a, b)
            | Expr::Sub(a, b)
            | Expr::Div(a, b)
            | Expr::Mul(a, b)
            | Expr::ForEach(a, b) => {
                a.walk(f);
                b.walk(f);
            }
            Expr::Max(v) => v.iter().for_each(|e| e.walk(f)),
            Expr::Neg(a) => a.walk(f),
            _ => {}
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Roll(r) => write!(f, "{r}"),
            Expr::ImprovisedWeapon => write!(f, "ImprovisedWeapon"),
            Expr::Add(a, b) => write!(f, "({a} + {b})"),
            Expr::Sub(a, b) => write!(f, "({a} - {b})"),
            Expr::Div(a, b) => write!(f, "({a} / {b})"),
            Expr::Mul(a, b) => write!(f, "({a} * {b})"),
            Expr::Max(v) => {
                write!(f, "max(")?;
                for (i, e) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{e}")?;
                }
                write!(f, ")")
            }
            Expr::ForEach(a, b) => write!(f, "foreach({a}, {b})"),
            Expr::Placeholder(n) => write!(f, "Placeholder{n}"),
            Expr::ResourceRoll { name, level } => write!(f, "ResourceRoll({name},{level})"),
            Expr::Var(v) => write!(f, "{}", v.text),
            Expr::Neg(a) => write!(f, "-{a}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Evaluation

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    #[error("unknown variable '{0}'")]
    UnknownVariable(String),
    #[error("{0}")]
    Unsupported(String),
}

/// Resolves the context-dependent parts of an expression.
pub trait ExprEnv {
    fn variable(&mut self, v: &Variable) -> Result<Number, EvalError>;
    /// `PlaceholderN` as a roll (for `DealDamage`, the weapon's damage roll).
    fn placeholder(&mut self, n: u32) -> Result<RollDefinition, EvalError> {
        Err(EvalError::Unsupported(format!("Placeholder{n}")))
    }
    fn resource_roll(&mut self, name: &str, level: u8) -> Result<RollDefinition, EvalError> {
        Err(EvalError::Unsupported(format!(
            "ResourceRoll({name},{level})"
        )))
    }
}

/// How dice are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Expected values, as for tooltips: `count * (faces + 1) / 2` per roll.
    Average,
    /// Roll the dice; `critical` doubles every roll's dice count.
    Roll { critical: bool },
}

/// One rolled die group, for logging.
#[derive(Debug, Clone, PartialEq)]
pub struct RolledDice {
    pub roll: RollDefinition,
    pub total: Number,
}

pub struct Evaluator<'a> {
    pub env: &'a mut dyn ExprEnv,
    pub rng: &'a mut dyn DiceRng,
    /// Every roll made, in order.
    pub rolls: Vec<RolledDice>,
}

impl<'a> Evaluator<'a> {
    pub fn new(env: &'a mut dyn ExprEnv, rng: &'a mut dyn DiceRng) -> Self {
        Self {
            env,
            rng,
            rolls: Vec::new(),
        }
    }

    fn roll(&mut self, r: RollDefinition, mode: Mode) -> Number {
        match mode {
            Mode::Average => r.average(),
            Mode::Roll { critical } => {
                let r = if critical { r.doubled() } else { r };
                let total = r.roll(self.rng);
                if r.dice.is_some() {
                    self.rolls.push(RolledDice { roll: r, total });
                }
                total
            }
        }
    }

    /// Evaluate to an integer.
    // bg3: eoc::stats_expression resolvers (table at 0x1086efeb0, 4.1.1.7631656)
    pub fn eval(&mut self, e: &Expr, mode: Mode) -> Result<Number, EvalError> {
        Ok(match e {
            Expr::Roll(r) => self.roll(*r, mode),
            Expr::ImprovisedWeapon => {
                return Err(EvalError::Unsupported("ImprovisedWeapon".into()));
            }
            Expr::Add(a, b) => self.eval(a, mode)?.wrapping_add(self.eval(b, mode)?),
            Expr::Sub(a, b) => self.eval(a, mode)?.wrapping_sub(self.eval(b, mode)?),
            Expr::Mul(a, b) => self.eval(a, mode)?.wrapping_mul(self.eval(b, mode)?),
            // Truncates toward zero; division by zero gives 0.
            Expr::Div(a, b) => {
                let x = self.eval(a, mode)?;
                let y = self.eval(b, mode)?;
                if y == 0 { 0 } else { x.wrapping_div(y) }
            }
            Expr::Neg(a) => self.eval(a, mode)?.wrapping_neg(),
            // Every argument is compared by its average; the largest wins (ties go to a
            // non-dice argument), and only the winner is rolled. So `max(1, 1d4-2)` can still
            // come out below 1 when rolled.
            // bg3: eoc::StatsExpressionObjectMax resolver (0x101f89a9c)
            Expr::Max(args) => {
                let mut best: Option<(usize, Number)> = None;
                for (i, a) in args.iter().enumerate() {
                    let v = self.eval(a, Mode::Average)?;
                    let better = match best {
                        None => true,
                        Some((bi, bv)) => v > bv || (v == bv && args[bi].is_roll() && !a.is_roll()),
                    };
                    if better {
                        best = Some((i, v));
                    }
                }
                match (best, mode) {
                    (None, _) => 0,
                    (Some((_, v)), Mode::Average) => v,
                    (Some((i, _)), Mode::Roll { .. }) => self.eval(&args[i], mode)?,
                }
            }
            Expr::ForEach(count, body) => {
                let n = self.eval(count, mode)?;
                if n <= 0 {
                    0
                } else if mode == Mode::Average {
                    self.eval(body, mode)?.wrapping_mul(n)
                } else {
                    let mut sum: Number = 0;
                    for _ in 0..n {
                        sum = sum.wrapping_add(self.eval(body, mode)?);
                    }
                    sum
                }
            }
            Expr::Placeholder(n) => {
                let r = self.env.placeholder(*n)?;
                self.roll(r, mode)
            }
            Expr::ResourceRoll { name, level } => {
                let r = self.env.resource_roll(name, *level)?;
                self.roll(r, mode)
            }
            Expr::Var(v) => self.env.variable(v)?,
        })
    }
}

/// An environment with no variables (pure dice arithmetic).
pub struct NoVariables;

impl ExprEnv for NoVariables {
    fn variable(&mut self, v: &Variable) -> Result<Number, EvalError> {
        Err(EvalError::UnknownVariable(v.text.clone()))
    }
}
