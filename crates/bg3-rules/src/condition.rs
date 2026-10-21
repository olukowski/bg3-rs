//! Stats conditions (`TargetConditions`, `Conditions`, `SpellRoll`, `IF(...)` in functors, …).
//!
//! A condition is a Thoth (Lua 5.3) expression written with `and`/`or`/`not`. The engine turns
//! it into a chunk with `Scripts/thoth/generateConditionString.khn`, which prepends
//! `context = ...` / `return ` and textually replaces ` and ` → `&`, `)and ` → `)&`,
//! ` or ` → `|`, `)or ` → `)|` and `not ` → `~`, so that the logical operators become the
//! non-short-circuiting `ConditionResult` operators. Two consequences we reproduce:
//! - both sides are always evaluated;
//! - Lua's bitwise operators bind tighter than comparisons, so `a == b and c` means
//!   `a == (b & c)` after the rewrite.
//!
//! [`ConditionEngine`] holds a Thoth machine with the engine natives (see [`crate::natives`])
//! and the helper scripts loaded, and evaluates conditions against a [`RulesContext`].

use std::rc::Rc;

use crate::context::{EntityId, RulesContext};
use crate::dice::DiceRng;
use crate::natives::{RollEvent, RulesHost};
use crate::textkey;
use crate::thoth::{self, ConditionResult, Machine, SyntaxError, Value, ast};

/// `generateConditionString.khn`, reimplemented: the Lua source the engine compiles for a
/// condition string. `string.gsub` with these literal patterns is a plain left-to-right
/// replacement.
pub fn condition_to_lua(cond: &str) -> String {
    let s = format!("context = ... \nreturn {cond}");
    s.replace(" and ", "&")
        .replace(")and ", ")&")
        .replace(" or ", "|")
        .replace(")or ", ")|")
        .replace("not ", "~")
}

/// `convertToChanceString.khn`, reimplemented: the variant used for hit-chance evaluation
/// (no `return` prefix and no `)and`/`)or` rules).
pub fn condition_to_chance_lua(cond: &str) -> String {
    if cond.is_empty() {
        return String::new();
    }
    cond.replace(" and ", "&")
        .replace(" or ", "|")
        .replace("not ", "~")
}

/// A compiled condition.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    /// The text as written in the stats.
    pub source: String,
    /// The generated Lua chunk source.
    pub lua: String,
    /// The compiled chunk, as a vararg function body.
    pub chunk: Rc<ast::FuncBody>,
}

impl Condition {
    pub fn parse(source: &str) -> Result<Self, SyntaxError> {
        let lua = condition_to_lua(source);
        let chunk = thoth::parse_chunk(&lua)?;
        Ok(Self {
            source: source.to_owned(),
            lua,
            chunk: Rc::new(ast::FuncBody {
                params: Vec::new(),
                vararg: true,
                body: chunk,
                line: 1,
            }),
        })
    }

    /// The returned expression (`return <expr>`), if the chunk is that shape.
    pub fn expr(&self) -> Option<&ast::Expr> {
        match self.chunk.body.ret.as_deref() {
            Some([e]) => Some(e),
            _ => None,
        }
    }

    /// Global functions the condition calls directly.
    pub fn called_functions(&self) -> Vec<Rc<str>> {
        let mut out = Vec::new();
        if let Some(r) = &self.chunk.body.ret {
            for e in r {
                out.extend(e.called_names());
            }
        }
        out
    }
}

/// `RollConditions` (`SpellRoll`, …): one condition per text key, e.g.
/// `Attack(AttackType.MeleeUnarmedAttack);Cast2[Attack(AttackType.MeleeUnarmedAttack)]`.
// bg3: RPGStats::ParseDataBuffer → TextKeyParsingHelpers::SplitStringPerTextKeyNoMerge
// (0x102016a7c) → RPGStats::AddCondition per key (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct RollConditions {
    pub groups: Vec<(String, Condition)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RollConditionsError {
    #[error(transparent)]
    TextKey(#[from] textkey::TextKeyError),
    #[error("{key}: {error}")]
    Condition { key: String, error: SyntaxError },
}

impl RollConditions {
    pub fn parse(s: &str) -> Result<Self, RollConditionsError> {
        let mut groups = Vec::new();
        for (key, text) in textkey::split(s)? {
            let c = Condition::parse(&text).map_err(|error| RollConditionsError::Condition {
                key: key.clone(),
                error,
            })?;
            groups.push((key, c));
        }
        Ok(Self { groups })
    }

    pub fn get(&self, key: &str) -> Option<&Condition> {
        self.groups.iter().find(|(k, _)| k == key).map(|(_, c)| c)
    }
}

/// The `context` a condition runs with (`ls::thoth::shared::ContextData`): the subset we model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConditionContext {
    pub source: Option<EntityId>,
    pub target: Option<EntityId>,
    pub observer: Option<EntityId>,
    /// Spell being cast, if any (`context.SpellId`-style queries).
    pub spell: Option<String>,
    /// Status involved, if any.
    pub status: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConditionEvalError {
    #[error("script error: {0}")]
    Script(String),
    #[error("condition returned {0}, not a ConditionResult/boolean")]
    BadReturn(String),
}

/// The result of evaluating a condition, plus the rolls it made.
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub result: ConditionResult,
    pub rolls: Vec<RollEvent>,
}

/// A Thoth machine set up for condition evaluation: natives registered, helper scripts loaded.
pub struct ConditionEngine {
    pub machine: Machine,
    /// `BaseSpellDifficultyCheck` (from `Data.txt` when available).
    pub base_spell_dc: i32,
}

impl Default for ConditionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ConditionEngine {
    pub fn new() -> Self {
        let mut machine = Machine::new();
        crate::natives::register(&mut machine);
        Self {
            machine,
            base_spell_dc: crate::rules::DEFAULT_BASE_SPELL_DC,
        }
    }

    /// Run a helper script (e.g. `CommonConditions.khn`) in the condition environment.
    pub fn load_helpers(
        &mut self,
        name: &str,
        src: &str,
        ctx: &mut dyn RulesContext,
        rng: &mut dyn DiceRng,
    ) -> Result<(), ConditionEvalError> {
        let chunk = thoth::parse_chunk(src)
            .map_err(|e| ConditionEvalError::Script(format!("{name}: {e}")))?;
        let mut host = RulesHost::new(ctx, rng, ConditionContext::default(), self.base_spell_dc);
        self.machine
            .exec(&mut host, chunk, Vec::new())
            .map_err(|e| ConditionEvalError::Script(format!("{name}: {e}")))?;
        Ok(())
    }

    /// Evaluate a condition. An empty condition is true (an absent condition in the engine).
    // bg3: ls::thoth::Machine::CheckCondition (0x1020ca9c0, 4.1.1.7631656)
    pub fn evaluate(
        &mut self,
        cond: &Condition,
        ctx: &mut dyn RulesContext,
        rng: &mut dyn DiceRng,
        data: &ConditionContext,
    ) -> Result<Evaluation, ConditionEvalError> {
        if cond.source.trim().is_empty() {
            return Ok(Evaluation {
                result: ConditionResult::new(true),
                rolls: Vec::new(),
            });
        }
        let mut host = RulesHost::new(ctx, rng, data.clone(), self.base_spell_dc);
        let context_value = host.context_value();
        let f = Machine::closure(cond.chunk.clone());
        let out = self
            .machine
            .call(&mut host, &f, vec![context_value])
            .map_err(|e| ConditionEvalError::Script(e.to_string()))?;
        let result = match out.into_iter().next().unwrap_or_default() {
            Value::Cond(c) => c.borrow().clone(),
            Value::Bool(b) => ConditionResult::new(b),
            Value::Int(_) | Value::Float(_) => ConditionResult::new(true),
            v => return Err(ConditionEvalError::BadReturn(v.to_string())),
        };
        Ok(Evaluation {
            result,
            rolls: host.rolls,
        })
    }

    /// Parse and evaluate condition text.
    pub fn evaluate_str(
        &mut self,
        cond: &str,
        ctx: &mut dyn RulesContext,
        rng: &mut dyn DiceRng,
        data: &ConditionContext,
    ) -> Result<Evaluation, ConditionEvalError> {
        let c = Condition::parse(cond).map_err(|e| ConditionEvalError::Script(e.to_string()))?;
        self.evaluate(&c, ctx, rng, data)
    }
}
