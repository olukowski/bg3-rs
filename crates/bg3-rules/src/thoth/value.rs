//! Runtime values of the Thoth interpreter.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use super::ast::FuncBody;
use super::interp::Scope;
use crate::context::EntityId;

pub type TableRef = Rc<RefCell<Table>>;

/// A Lua value, plus the engine object types Thoth exposes (entities, enum values,
/// `ConditionResult`).
#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Table(TableRef),
    Function(Rc<Closure>),
    /// A function of the interpreter's standard library.
    Builtin(Builtin),
    /// A function implemented by the host (the engine's exported natives), called by name.
    Native(Rc<str>),
    /// An entity handle; `None` is the null handle (`IsValid == false`).
    Entity(Option<EntityId>),
    /// An enum type exported to scripts (`Ability`, `AttackType`, …). Indexing it yields values.
    EnumType(Rc<str>),
    /// An enum value (`Ability.Dexterity`).
    Enum(Rc<EnumValue>),
    /// `ls::thoth::shared::ConditionResult`.
    Cond(Rc<RefCell<ConditionResult>>),
}

pub struct Closure {
    pub body: Rc<FuncBody>,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumValue {
    pub ty: Rc<str>,
    pub name: Rc<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Print,
    Type,
    ToString,
    ToNumber,
    Pairs,
    IPairs,
    IPairsIter,
    Next,
    Select,
    Error,
    Assert,
    PCall,
    SetMetatable,
    GetMetatable,
    RawGet,
    RawSet,
    RawEqual,
    Require,
    Unpack,
    TableInsert,
    TableConcat,
    TableRemove,
    MathFloor,
    MathCeil,
    MathMax,
    MathMin,
    MathAbs,
    StringFormat,
    StringRep,
    StringSub,
    StringLen,
    StringLower,
    StringUpper,
    StringFind,
    StringGsub,
}

/// One error entry of a [`ConditionResult`] (`ls::thoth::shared::ConditionError`): an error name
/// (shown in tooltips, e.g. why a target is invalid) and optional data.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionError {
    pub name: Rc<str>,
    pub data: Vec<String>,
}

/// `ls::thoth::shared::ConditionResult`.
///
/// `active_errors` are reported when the result is false; `inactive_errors` are the ones that
/// apply if the result gets negated (they swap on `~`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionResult {
    pub result: bool,
    /// Probability of `result` being true, as computed by chance-mode helpers; 1.0/0.0 for
    /// plain booleans. Stored as `f32` like the engine.
    pub chance: f32,
    pub active_errors: Vec<ConditionError>,
    pub inactive_errors: Vec<ConditionError>,
    pub priority: i64,
}

impl ConditionResult {
    /// `ConditionResult(result, active, inactive, chance)`: chance defaults to 1.0/0.0.
    // bg3: ls::thoth::shared::ConditionResult::Make (0x102020960, 4.1.1.7631656)
    pub fn new(result: bool) -> Self {
        Self {
            result,
            chance: if result { 1.0 } else { 0.0 },
            active_errors: Vec::new(),
            inactive_errors: Vec::new(),
            priority: 0,
        }
    }

    /// Coercion of a Lua number operand (`{Result = f > 0, Chance = f}`).
    pub fn from_number(f: f64) -> Self {
        Self {
            result: f > 0.0,
            chance: f as f32,
            ..Self::new(false)
        }
    }

    /// `a & b`: both must hold; chances multiply. When the operands disagree, the false one's
    /// errors are kept, otherwise the left one's.
    // bg3: ls::thoth::shared::operator& (0x102020b0c, 4.1.1.7631656)
    pub fn and(&self, b: &Self) -> Self {
        let errs = if self.result != b.result {
            if self.result { b } else { self }
        } else {
            self
        };
        Self {
            result: self.result && b.result,
            chance: self.chance * b.chance,
            active_errors: errs.active_errors.clone(),
            inactive_errors: errs.inactive_errors.clone(),
            priority: 0,
        }
    }

    /// `a | b`: chance `1 - (1-a)(1-b)`. When the operands disagree, the true one's errors are
    /// kept, otherwise the left one's.
    // bg3: ls::thoth::shared::operator| (0x1020215a0, 4.1.1.7631656)
    pub fn or(&self, b: &Self) -> Self {
        // The engine's "1 - x" helper leaves values outside [-1, 1] (and NaN) unchanged and
        // uses 1 - |x| otherwise.
        fn complement(x: f32) -> f32 {
            if x.is_nan() || x.abs() > 1.0 {
                x
            } else {
                1.0 - x.abs()
            }
        }
        let errs = if self.result != b.result {
            if self.result { self } else { b }
        } else {
            self
        };
        Self {
            result: self.result || b.result,
            chance: complement(complement(self.chance) * complement(b.chance)),
            active_errors: errs.active_errors.clone(),
            inactive_errors: errs.inactive_errors.clone(),
            priority: 0,
        }
    }

    /// `~a`: flips the result, swaps the error lists, chance becomes `1 - chance`.
    // bg3: ls::thoth::shared BNOT operator binding (0x1020b6afc, 4.1.1.7631656)
    pub fn not(&self) -> Self {
        Self {
            result: !self.result,
            chance: 1.0 - self.chance,
            active_errors: self.inactive_errors.clone(),
            inactive_errors: self.active_errors.clone(),
            priority: 0,
        }
    }
}

/// Table key. Floats with an integral value are normalised to `Int`, as in Lua.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Int(i64),
    Float(u64),
    Str(Rc<str>),
    Bool(bool),
    Entity(Option<EntityId>),
    Enum(Rc<EnumValue>),
    /// Reference types, by address.
    Ptr(usize),
}

/// A Lua table: insertion-ordered (so `pairs` is deterministic) with an optional metatable.
#[derive(Default)]
pub struct Table {
    entries: Vec<(Key, Value)>,
    index: HashMap<Key, usize>,
    pub meta: Option<TableRef>,
}

impl Table {
    pub fn new_ref() -> TableRef {
        Rc::new(RefCell::new(Table::default()))
    }

    pub fn get(&self, k: &Key) -> Value {
        self.index
            .get(k)
            .map(|&i| self.entries[i].1.clone())
            .unwrap_or_default()
    }

    pub fn get_str(&self, k: &str) -> Value {
        self.get(&Key::Str(k.into()))
    }

    pub fn set(&mut self, k: Key, v: Value) {
        match self.index.get(&k) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                if matches!(v, Value::Nil) {
                    return;
                }
                self.index.insert(k.clone(), self.entries.len());
                self.entries.push((k, v));
            }
        }
    }

    pub fn set_str(&mut self, k: &str, v: Value) {
        self.set(Key::Str(k.into()), v);
    }

    /// The border `#t`: number of consecutive non-nil integer keys from 1.
    pub fn len(&self) -> i64 {
        let mut n = 0;
        while !matches!(self.get(&Key::Int(n + 1)), Value::Nil) {
            n += 1;
        }
        n
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The entry after `k` (`None` = first), skipping nil values.
    pub fn next(&self, k: Option<&Key>) -> Option<(Key, Value)> {
        let start = match k {
            None => 0,
            Some(k) => self.index.get(k)? + 1,
        };
        self.entries[start..]
            .iter()
            .find(|(_, v)| !matches!(v, Value::Nil))
            .cloned()
    }

    pub fn push(&mut self, v: Value) {
        let n = self.len();
        self.set(Key::Int(n + 1), v);
    }
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    pub fn cond(c: ConditionResult) -> Value {
        Value::Cond(Rc::new(RefCell::new(c)))
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Float(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Function(_) | Value::Builtin(_) | Value::Native(_) => "function",
            _ => "userdata",
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Str(s) => match super::lexer::parse_number(s) {
                Some(super::lexer::Tok::Int(i)) => Some(i as f64),
                Some(super::lexer::Tok::Float(f)) => Some(f),
                _ => None,
            },
            _ => None,
        }
    }

    /// Integer value if this is an integer or a float/string with an exact integer value.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.2e18 => Some(*f as i64),
            Value::Str(_) => {
                let n = self.as_number()?;
                (n.fract() == 0.0).then_some(n as i64)
            }
            _ => None,
        }
    }

    pub fn to_key(&self) -> Option<Key> {
        Some(match self {
            Value::Nil => return None,
            Value::Bool(b) => Key::Bool(*b),
            Value::Int(i) => Key::Int(*i),
            Value::Float(f) => {
                if f.is_nan() {
                    return None;
                }
                if f.fract() == 0.0 && f.abs() < 9.2e18 {
                    Key::Int(*f as i64)
                } else {
                    Key::Float(f.to_bits())
                }
            }
            Value::Str(s) => Key::Str(s.clone()),
            Value::Entity(e) => Key::Entity(*e),
            Value::Enum(e) => Key::Enum(e.clone()),
            Value::Table(t) => Key::Ptr(Rc::as_ptr(t) as *const u8 as usize),
            Value::Function(f) => Key::Ptr(Rc::as_ptr(f) as *const u8 as usize),
            Value::Cond(c) => Key::Ptr(Rc::as_ptr(c) as *const u8 as usize),
            Value::Native(n) => Key::Str(format!("native:{n}").into()),
            Value::Builtin(b) => Key::Str(format!("builtin:{b:?}").into()),
            Value::EnumType(t) => Key::Str(format!("enumtype:{t}").into()),
        })
    }

    pub fn from_key(k: &Key) -> Value {
        match k {
            Key::Int(i) => Value::Int(*i),
            Key::Float(b) => Value::Float(f64::from_bits(*b)),
            Key::Str(s) => Value::Str(s.clone()),
            Key::Bool(b) => Value::Bool(*b),
            Key::Entity(e) => Value::Entity(*e),
            Key::Enum(e) => Value::Enum(e.clone()),
            // Reference keys can't be turned back into values; scripts in the install don't
            // iterate tables keyed by tables.
            Key::Ptr(_) => Value::Nil,
        }
    }

    /// Raw equality (`rawequal`, and `==` without metamethods).
    pub fn raw_eq(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                self.as_number() == o.as_number()
            }
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(a, b),
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            (Value::Builtin(a), Value::Builtin(b)) => a == b,
            (Value::Native(a), Value::Native(b)) => a == b,
            (Value::Entity(a), Value::Entity(b)) => a == b,
            (Value::Enum(a), Value::Enum(b)) => a == b,
            (Value::EnumType(a), Value::EnumType(b)) => a == b,
            (Value::Cond(a), Value::Cond(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if x.fract() == 0.0 && x.abs() < 1e15 {
                    write!(f, "{x:.1}")
                } else {
                    write!(f, "{x}")
                }
            }
            Value::Str(s) => write!(f, "{s}"),
            Value::Table(t) => write!(f, "table: {:p}", Rc::as_ptr(t)),
            Value::Function(c) => write!(f, "function: {:p}", Rc::as_ptr(c)),
            Value::Builtin(b) => write!(f, "builtin: {b:?}"),
            Value::Native(n) => write!(f, "native: {n}"),
            Value::Entity(Some(e)) => write!(f, "Entity({})", e.0),
            Value::Entity(None) => write!(f, "Entity(null)"),
            Value::EnumType(t) => write!(f, "{t}"),
            Value::Enum(e) => write!(f, "{}.{}", e.ty, e.name),
            Value::Cond(c) => {
                let c = c.borrow();
                write!(f, "ConditionResult({}, {})", c.result, c.chance)
            }
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{s:?}"),
            v => write!(f, "{v}"),
        }
    }
}
