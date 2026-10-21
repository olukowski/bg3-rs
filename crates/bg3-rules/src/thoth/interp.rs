//! Tree-walking interpreter for Thoth.
//!
//! Covers what condition evaluation needs: the full statement and expression language (except
//! `goto`), closures, varargs, multiple returns, tables with `__index`/`__call` metatables,
//! `try`/`catch`, a small standard library, and the engine object types (entities, enum values,
//! `ConditionResult`). Engine functions ("natives") are delegated to a [`Host`].
//!
//! Lua 5.3 semantics that matter for conditions and are reproduced here: integer/float
//! subtypes (`/` is float division, `math.floor` returns an integer), bitwise operators binding
//! tighter than comparisons, `and`/`or` returning operands, and every userdata (including a
//! false `ConditionResult`) being truthy.

use std::cell::RefCell;
use std::rc::Rc;

use super::ast::*;
use super::value::*;
use crate::context::EntityId;

/// Lexical scope: an immutable linked list of bindings, so closures capture exactly the locals
/// visible where they were created.
pub type Scope = Option<Rc<Binding>>;

pub struct Binding {
    name: Rc<str>,
    slot: RefCell<Value>,
    next: Scope,
}

fn bind(scope: &Scope, name: &str, v: Value) -> Scope {
    Some(Rc::new(Binding {
        name: name.into(),
        slot: RefCell::new(v),
        next: scope.clone(),
    }))
}

fn lookup<'a>(mut scope: &'a Scope, name: &str) -> Option<&'a Binding> {
    while let Some(b) = scope {
        if &*b.name == name {
            return Some(b);
        }
        scope = &b.next;
    }
    None
}

/// A runtime error: the error value (usually a message string), catchable by `try`/`pcall`.
#[derive(Debug, Clone)]
pub struct LuaError {
    pub value: Value,
}

impl std::fmt::Display for LuaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl std::error::Error for LuaError {}

pub type LResult<T> = Result<T, LuaError>;

pub fn rt_err<T>(msg: impl Into<String>) -> LResult<T> {
    Err(LuaError {
        value: Value::Str(msg.into().into()),
    })
}

/// The embedding application: engine natives and engine object properties.
pub trait Host {
    /// Call the native function `name` (a [`Value::Native`]).
    fn call_native(&mut self, m: &mut Machine, name: &str, args: &[Value]) -> LResult<Vec<Value>>;

    /// `entity.Key` (e.g. `IsValid`, `HP`, `Strength`).
    fn index_entity(&mut self, e: Option<EntityId>, key: &str) -> LResult<Value> {
        let _ = e;
        rt_err(format!("entity has no property '{key}'"))
    }

    /// `EnumType.Name.value`: the numeric value of an enum label, if known.
    fn enum_ordinal(&self, ty: &str, name: &str) -> Option<i64> {
        let _ = (ty, name);
        None
    }

    /// `require 'module'`. The default returns an empty table.
    fn require(&mut self, m: &mut Machine, module: &str) -> LResult<Value> {
        let _ = (m, module);
        Ok(Value::Table(Table::new_ref()))
    }

    fn print(&mut self, text: &str) {
        let _ = text;
    }
}

/// A host with no natives (plain Lua).
pub struct NoHost;

impl Host for NoHost {
    fn call_native(&mut self, _: &mut Machine, name: &str, _: &[Value]) -> LResult<Vec<Value>> {
        rt_err(format!("native '{name}' is not available"))
    }
}

enum Flow {
    Normal,
    Break,
    Return(Vec<Value>),
}

struct Frame {
    varargs: Vec<Value>,
}

/// Interpreter state: the global environment.
pub struct Machine {
    pub globals: TableRef,
    string_lib: TableRef,
    depth: usize,
}

const MAX_DEPTH: usize = 180;

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    /// A machine whose globals hold the standard library subset.
    pub fn new() -> Self {
        let g = Table::new_ref();
        let string_lib = Table::new_ref();
        {
            let mut t = g.borrow_mut();
            use Builtin::*;
            for (n, b) in [
                ("print", Print),
                ("type", Type),
                ("tostring", ToString),
                ("tonumber", ToNumber),
                ("pairs", Pairs),
                ("ipairs", IPairs),
                ("next", Next),
                ("select", Select),
                ("error", Error),
                ("assert", Assert),
                ("pcall", PCall),
                ("setmetatable", SetMetatable),
                ("getmetatable", GetMetatable),
                ("rawget", RawGet),
                ("rawset", RawSet),
                ("rawequal", RawEqual),
                ("require", Require),
                ("unpack", Unpack),
            ] {
                t.set_str(n, Value::Builtin(b));
            }
            let math = Table::new_ref();
            {
                let mut m = math.borrow_mut();
                m.set_str("floor", Value::Builtin(MathFloor));
                m.set_str("ceil", Value::Builtin(MathCeil));
                m.set_str("max", Value::Builtin(MathMax));
                m.set_str("min", Value::Builtin(MathMin));
                m.set_str("abs", Value::Builtin(MathAbs));
                m.set_str("huge", Value::Float(f64::INFINITY));
                m.set_str("maxinteger", Value::Int(i64::MAX));
                m.set_str("mininteger", Value::Int(i64::MIN));
                m.set_str("pi", Value::Float(std::f64::consts::PI));
            }
            t.set_str("math", Value::Table(math));
            let table = Table::new_ref();
            {
                let mut m = table.borrow_mut();
                m.set_str("insert", Value::Builtin(TableInsert));
                m.set_str("concat", Value::Builtin(TableConcat));
                m.set_str("remove", Value::Builtin(TableRemove));
                m.set_str("unpack", Value::Builtin(Unpack));
            }
            t.set_str("table", Value::Table(table));
            {
                let mut m = string_lib.borrow_mut();
                m.set_str("format", Value::Builtin(StringFormat));
                m.set_str("rep", Value::Builtin(StringRep));
                m.set_str("sub", Value::Builtin(StringSub));
                m.set_str("len", Value::Builtin(StringLen));
                m.set_str("lower", Value::Builtin(StringLower));
                m.set_str("upper", Value::Builtin(StringUpper));
                m.set_str("find", Value::Builtin(StringFind));
                m.set_str("gsub", Value::Builtin(StringGsub));
            }
            t.set_str("string", Value::Table(string_lib.clone()));
        }
        g.borrow_mut().set_str("_G", Value::Table(g.clone()));
        Self {
            globals: g,
            string_lib,
            depth: 0,
        }
    }

    pub fn set_global(&mut self, name: &str, v: Value) {
        self.globals.borrow_mut().set_str(name, v);
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.borrow().get_str(name)
    }

    /// Turn a parsed chunk into a callable function (a vararg closure with no upvalues).
    pub fn load(&self, chunk: Block) -> Value {
        Value::Function(Rc::new(Closure {
            body: Rc::new(FuncBody {
                params: Vec::new(),
                vararg: true,
                body: chunk,
                line: 0,
            }),
            scope: None,
        }))
    }

    /// A closure over a function body with no upvalues (a compiled chunk).
    pub fn closure(body: Rc<FuncBody>) -> Value {
        Value::Function(Rc::new(Closure { body, scope: None }))
    }

    /// Run a chunk with the given arguments (`...`).
    pub fn exec(
        &mut self,
        host: &mut dyn Host,
        chunk: Block,
        args: Vec<Value>,
    ) -> LResult<Vec<Value>> {
        let f = self.load(chunk);
        self.call(host, &f, args)
    }

    pub fn call(
        &mut self,
        host: &mut dyn Host,
        f: &Value,
        args: Vec<Value>,
    ) -> LResult<Vec<Value>> {
        if self.depth >= MAX_DEPTH {
            return rt_err("stack overflow");
        }
        self.depth += 1;
        let r = self.call_inner(host, f, args);
        self.depth -= 1;
        r
    }

    fn call_inner(
        &mut self,
        host: &mut dyn Host,
        f: &Value,
        args: Vec<Value>,
    ) -> LResult<Vec<Value>> {
        match f {
            Value::Function(c) => {
                let mut scope = c.scope.clone();
                let mut it = args.into_iter();
                for p in &c.body.params {
                    scope = bind(&scope, p, it.next().unwrap_or_default());
                }
                let frame = Frame {
                    varargs: if c.body.vararg {
                        it.collect()
                    } else {
                        Vec::new()
                    },
                };
                match self.exec_block(host, &c.body.body, &scope, &frame)? {
                    Flow::Return(v) => Ok(v),
                    _ => Ok(Vec::new()),
                }
            }
            Value::Builtin(b) => self.builtin(host, *b, args),
            Value::Native(n) => {
                let n = n.clone();
                host.call_native(self, &n, &args)
            }
            Value::Table(t) => {
                let call = t
                    .borrow()
                    .meta
                    .as_ref()
                    .map(|m| m.borrow().get_str("__call"));
                match call {
                    Some(c) if !matches!(c, Value::Nil) => {
                        let mut a = vec![f.clone()];
                        a.extend(args);
                        self.call(host, &c, a)
                    }
                    _ => rt_err("attempt to call a table value"),
                }
            }
            v => rt_err(format!("attempt to call a {} value", v.type_name())),
        }
    }

    fn exec_block(
        &mut self,
        host: &mut dyn Host,
        b: &Block,
        scope: &Scope,
        fr: &Frame,
    ) -> LResult<Flow> {
        let mut scope = scope.clone();
        for s in &b.stats {
            match self.exec_stat(host, s, &mut scope, fr)? {
                Flow::Normal => {}
                f => return Ok(f),
            }
        }
        if let Some(ret) = &b.ret {
            // A tail `return f(x)` keeps all results.
            return Ok(Flow::Return(self.eval_list(host, ret, &scope, fr)?));
        }
        Ok(Flow::Normal)
    }

    fn exec_stat(
        &mut self,
        host: &mut dyn Host,
        s: &Stat,
        scope: &mut Scope,
        fr: &Frame,
    ) -> LResult<Flow> {
        match s {
            Stat::Local { names, values } => {
                let vals = self.eval_list(host, values, scope, fr)?;
                let mut it = vals.into_iter();
                for n in names {
                    *scope = bind(scope, n, it.next().unwrap_or_default());
                }
            }
            Stat::LocalFunction { name, func } => {
                // The function can see itself (recursion).
                *scope = bind(scope, name, Value::Nil);
                let f = Value::Function(Rc::new(Closure {
                    body: func.clone(),
                    scope: scope.clone(),
                }));
                *scope.as_ref().unwrap().slot.borrow_mut() = f;
            }
            Stat::Assign { targets, values } => {
                let vals = self.eval_list(host, values, scope, fr)?;
                let mut it = vals.into_iter();
                for t in targets {
                    let v = it.next().unwrap_or_default();
                    self.assign(host, t, v, scope, fr)?;
                }
            }
            Stat::Call(e) => {
                self.eval_multi(host, e, scope, fr)?;
            }
            Stat::Do(b) => return self.exec_block(host, b, scope, fr),
            Stat::While { cond, body } => {
                while self.eval(host, cond, scope, fr)?.truthy() {
                    match self.exec_block(host, body, scope, fr)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Normal => {}
                    }
                }
            }
            Stat::Repeat { body, cond } => loop {
                // The condition can see the body's locals.
                let mut inner = scope.clone();
                let mut flow = Flow::Normal;
                for s in &body.stats {
                    flow = self.exec_stat(host, s, &mut inner, fr)?;
                    if !matches!(flow, Flow::Normal) {
                        break;
                    }
                }
                if matches!(flow, Flow::Normal)
                    && let Some(ret) = &body.ret
                {
                    flow = Flow::Return(self.eval_list(host, ret, &inner, fr)?);
                }
                match flow {
                    Flow::Break => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    Flow::Normal => {}
                }
                if self.eval(host, cond, &inner, fr)?.truthy() {
                    break;
                }
            },
            Stat::If { arms, otherwise } => {
                for (c, b) in arms {
                    if self.eval(host, c, scope, fr)?.truthy() {
                        return self.exec_block(host, b, scope, fr);
                    }
                }
                if let Some(b) = otherwise {
                    return self.exec_block(host, b, scope, fr);
                }
            }
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let a = self.eval(host, start, scope, fr)?;
                let l = self.eval(host, limit, scope, fr)?;
                let st = match step {
                    Some(s) => self.eval(host, s, scope, fr)?,
                    None => Value::Int(1),
                };
                if let (Value::Int(a), Some(l), Value::Int(st)) = (&a, l.as_number(), &st) {
                    if *st == 0 {
                        return rt_err("'for' step is zero");
                    }
                    let l = if *st > 0 { l.floor() } else { l.ceil() };
                    let mut i = *a;
                    while (*st > 0 && (i as f64) <= l) || (*st < 0 && (i as f64) >= l) {
                        let inner = bind(scope, var, Value::Int(i));
                        match self.exec_block(host, body, &inner, fr)? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            Flow::Normal => {}
                        }
                        i = match i.checked_add(*st) {
                            Some(n) => n,
                            None => break,
                        };
                    }
                } else {
                    let (Some(a), Some(l), Some(st)) =
                        (a.as_number(), l.as_number(), st.as_number())
                    else {
                        return rt_err("'for' initial value must be a number");
                    };
                    let mut i = a;
                    while (st > 0.0 && i <= l) || (st < 0.0 && i >= l) {
                        let inner = bind(scope, var, Value::Float(i));
                        match self.exec_block(host, body, &inner, fr)? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            Flow::Normal => {}
                        }
                        i += st;
                    }
                }
            }
            Stat::GenericFor { vars, exprs, body } => {
                let mut v = self.eval_list(host, exprs, scope, fr)?.into_iter();
                let f = v.next().unwrap_or_default();
                let state = v.next().unwrap_or_default();
                let mut control = v.next().unwrap_or_default();
                loop {
                    let r = self.call(host, &f, vec![state.clone(), control.clone()])?;
                    let first = r.first().cloned().unwrap_or_default();
                    if matches!(first, Value::Nil) {
                        break;
                    }
                    control = first;
                    let mut inner = scope.clone();
                    let mut it = r.into_iter();
                    for n in vars {
                        inner = bind(&inner, n, it.next().unwrap_or_default());
                    }
                    match self.exec_block(host, body, &inner, fr)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Normal => {}
                    }
                }
            }
            Stat::Function { path, method, func } => {
                let f = Value::Function(Rc::new(Closure {
                    body: func.clone(),
                    scope: scope.clone(),
                }));
                let mut target = Expr::Name(path[0].as_str().into());
                for p in &path[1..] {
                    target = Expr::Index(Box::new(target), Box::new(Expr::Str(p.as_str().into())));
                }
                if let Some(m) = method {
                    target = Expr::Index(Box::new(target), Box::new(Expr::Str(m.as_str().into())));
                }
                self.assign(host, &target, f, scope, fr)?;
            }
            Stat::Try { body, var, handler } => match self.exec_block(host, body, scope, fr) {
                Ok(flow) => return Ok(flow),
                Err(e) => {
                    let inner = bind(scope, var, e.value);
                    return self.exec_block(host, handler, &inner, fr);
                }
            },
            Stat::Break => return Ok(Flow::Break),
            Stat::Label(_) => {}
            Stat::Goto(l) => return rt_err(format!("goto {l}: not supported by this interpreter")),
        }
        Ok(Flow::Normal)
    }

    fn assign(
        &mut self,
        host: &mut dyn Host,
        target: &Expr,
        v: Value,
        scope: &Scope,
        fr: &Frame,
    ) -> LResult<()> {
        match target {
            Expr::Name(n) => {
                if let Some(b) = lookup(scope, n) {
                    *b.slot.borrow_mut() = v;
                } else {
                    self.globals.borrow_mut().set(Key::Str(n.clone()), v);
                }
                Ok(())
            }
            Expr::Index(obj, key) => {
                let o = self.eval(host, obj, scope, fr)?;
                let k = self.eval(host, key, scope, fr)?;
                self.set_index(&o, &k, v)
            }
            _ => rt_err("cannot assign"),
        }
    }

    pub fn set_index(&mut self, o: &Value, k: &Value, v: Value) -> LResult<()> {
        match o {
            Value::Table(t) => {
                let Some(key) = k.to_key() else {
                    return rt_err("table index is nil or NaN");
                };
                t.borrow_mut().set(key, v);
                Ok(())
            }
            Value::Cond(c) => {
                let mut c = c.borrow_mut();
                match k {
                    Value::Str(s) if &**s == "Priority" => c.priority = v.as_int().unwrap_or(0),
                    Value::Str(s) if &**s == "Result" => c.result = v.truthy(),
                    Value::Str(s) if &**s == "Chance" => {
                        c.chance = v.as_number().unwrap_or(0.0) as f32
                    }
                    _ => return rt_err(format!("ConditionResult has no settable field {k}")),
                }
                Ok(())
            }
            _ => rt_err(format!("attempt to index a {} value", o.type_name())),
        }
    }

    pub fn index(&mut self, host: &mut dyn Host, o: &Value, k: &Value) -> LResult<Value> {
        match o {
            Value::Table(t) => {
                let (raw, meta) = {
                    let t = t.borrow();
                    let raw = k.to_key().map(|key| t.get(&key)).unwrap_or_default();
                    (raw, t.meta.clone())
                };
                if !matches!(raw, Value::Nil) {
                    return Ok(raw);
                }
                let Some(meta) = meta else {
                    return Ok(Value::Nil);
                };
                let h = meta.borrow().get_str("__index");
                match h {
                    Value::Nil => Ok(Value::Nil),
                    Value::Function(_) | Value::Builtin(_) | Value::Native(_) => Ok(self
                        .call(host, &h, vec![o.clone(), k.clone()])?
                        .into_iter()
                        .next()
                        .unwrap_or_default()),
                    h => self.index(host, &h, k),
                }
            }
            Value::Str(_) => Ok(k
                .to_key()
                .map(|key| self.string_lib.borrow().get(&key))
                .unwrap_or_default()),
            Value::Entity(e) => match k {
                Value::Str(s) => host.index_entity(*e, s),
                _ => rt_err("entity index must be a string"),
            },
            Value::EnumType(t) => match k {
                Value::Str(s) => Ok(Value::Enum(Rc::new(EnumValue {
                    ty: t.clone(),
                    name: s.clone(),
                }))),
                _ => rt_err("enum index must be a string"),
            },
            Value::Enum(e) => match k {
                Value::Str(s) if &**s == "value" => match host.enum_ordinal(&e.ty, &e.name) {
                    Some(v) => Ok(Value::Int(v)),
                    None => rt_err(format!("no numeric value known for {}.{}", e.ty, e.name)),
                },
                Value::Str(s) if &**s == "Label" => Ok(Value::Str(e.name.clone())),
                _ => rt_err(format!("enum value has no field {k}")),
            },
            Value::Cond(c) => {
                let c = c.borrow();
                Ok(match k {
                    Value::Str(s) => match &**s {
                        "Result" => Value::Bool(c.result),
                        "Chance" => Value::Float(c.chance as f64),
                        "Priority" => Value::Int(c.priority),
                        "ActiveErrors" | "InactiveErrors" => {
                            let list = if &**s == "ActiveErrors" {
                                &c.active_errors
                            } else {
                                &c.inactive_errors
                            };
                            let t = Table::new_ref();
                            for e in list {
                                t.borrow_mut().push(Value::Str(e.name.clone()));
                            }
                            Value::Table(t)
                        }
                        _ => return rt_err(format!("ConditionResult has no field '{s}'")),
                    },
                    _ => return rt_err("ConditionResult index must be a string"),
                })
            }
            Value::Nil => rt_err(format!("attempt to index a nil value (field '{k}')")),
            _ => rt_err(format!("attempt to index a {} value", o.type_name())),
        }
    }

    fn eval_list(
        &mut self,
        host: &mut dyn Host,
        es: &[Expr],
        scope: &Scope,
        fr: &Frame,
    ) -> LResult<Vec<Value>> {
        let mut out = Vec::with_capacity(es.len());
        for (i, e) in es.iter().enumerate() {
            if i + 1 == es.len() {
                out.extend(self.eval_multi(host, e, scope, fr)?);
            } else {
                out.push(self.eval(host, e, scope, fr)?);
            }
        }
        Ok(out)
    }

    fn eval_multi(
        &mut self,
        host: &mut dyn Host,
        e: &Expr,
        scope: &Scope,
        fr: &Frame,
    ) -> LResult<Vec<Value>> {
        match e {
            Expr::Call(callee, args) => {
                let f = self.eval(host, callee, scope, fr)?;
                let a = self.eval_list(host, args, scope, fr)?;
                if matches!(f, Value::Nil) {
                    let name = match &**callee {
                        Expr::Name(n) => format!(" (global '{n}')"),
                        Expr::Index(_, k) => match &**k {
                            Expr::Str(s) => format!(" (field '{s}')"),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    return rt_err(format!("attempt to call a nil value{name}"));
                }
                self.call(host, &f, a)
            }
            Expr::Method(obj, name, args) => {
                let o = self.eval(host, obj, scope, fr)?;
                let f = self.index(host, &o, &Value::Str(name.clone()))?;
                if matches!(f, Value::Nil) {
                    return rt_err(format!("attempt to call a nil value (method '{name}')"));
                }
                let mut a = vec![o];
                a.extend(self.eval_list(host, args, scope, fr)?);
                self.call(host, &f, a)
            }
            Expr::Vararg => Ok(fr.varargs.clone()),
            e => Ok(vec![self.eval(host, e, scope, fr)?]),
        }
    }

    fn eval(&mut self, host: &mut dyn Host, e: &Expr, scope: &Scope, fr: &Frame) -> LResult<Value> {
        Ok(match e {
            Expr::Nil => Value::Nil,
            Expr::True => Value::Bool(true),
            Expr::False => Value::Bool(false),
            Expr::Int(i) => Value::Int(*i),
            Expr::Float(f) => Value::Float(*f),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Vararg => fr.varargs.first().cloned().unwrap_or_default(),
            Expr::Function(f) => Value::Function(Rc::new(Closure {
                body: f.clone(),
                scope: scope.clone(),
            })),
            Expr::Name(n) => match lookup(scope, n) {
                Some(b) => b.slot.borrow().clone(),
                None => self.globals.borrow().get(&Key::Str(n.clone())),
            },
            Expr::Index(o, k) => {
                let o = self.eval(host, o, scope, fr)?;
                let k = self.eval(host, k, scope, fr)?;
                self.index(host, &o, &k)?
            }
            Expr::Call(..) | Expr::Method(..) => self
                .eval_multi(host, e, scope, fr)?
                .into_iter()
                .next()
                .unwrap_or_default(),
            Expr::Paren(e) => self.eval(host, e, scope, fr)?,
            Expr::Table(fields) => {
                let t = Table::new_ref();
                let mut n = 0;
                for (i, f) in fields.iter().enumerate() {
                    match f {
                        TableField::Named(k, v) => {
                            let v = self.eval(host, v, scope, fr)?;
                            t.borrow_mut().set(Key::Str(k.clone()), v);
                        }
                        TableField::Keyed(k, v) => {
                            let k = self.eval(host, k, scope, fr)?;
                            let v = self.eval(host, v, scope, fr)?;
                            let Some(k) = k.to_key() else {
                                return rt_err("table index is nil");
                            };
                            t.borrow_mut().set(k, v);
                        }
                        TableField::Positional(v) => {
                            let vals = if i + 1 == fields.len() {
                                self.eval_multi(host, v, scope, fr)?
                            } else {
                                vec![self.eval(host, v, scope, fr)?]
                            };
                            for v in vals {
                                n += 1;
                                t.borrow_mut().set(Key::Int(n), v);
                            }
                        }
                    }
                }
                Value::Table(t)
            }
            Expr::Bin(BinOp::And, a, b) => {
                let a = self.eval(host, a, scope, fr)?;
                if !a.truthy() {
                    a
                } else {
                    self.eval(host, b, scope, fr)?
                }
            }
            Expr::Bin(BinOp::Or, a, b) => {
                let a = self.eval(host, a, scope, fr)?;
                if a.truthy() {
                    a
                } else {
                    self.eval(host, b, scope, fr)?
                }
            }
            Expr::Bin(op, a, b) => {
                let a = self.eval(host, a, scope, fr)?;
                let b = self.eval(host, b, scope, fr)?;
                binary(*op, &a, &b)?
            }
            Expr::Un(op, a) => {
                let a = self.eval(host, a, scope, fr)?;
                unary(*op, &a)?
            }
        })
    }

    fn builtin(
        &mut self,
        host: &mut dyn Host,
        b: Builtin,
        args: Vec<Value>,
    ) -> LResult<Vec<Value>> {
        use Builtin::*;
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let one = |v: Value| Ok(vec![v]);
        match b {
            Print => {
                let s: Vec<String> = args.iter().map(|v| v.to_string()).collect();
                host.print(&s.join("\t"));
                Ok(vec![])
            }
            Type => one(Value::str(arg(0).type_name())),
            ToString => one(Value::Str(tostring(&arg(0)).into())),
            ToNumber => {
                let v = arg(0);
                one(match v {
                    Value::Int(_) | Value::Float(_) => v,
                    Value::Str(ref s) => match super::lexer::parse_number(s) {
                        Some(super::lexer::Tok::Int(i)) => Value::Int(i),
                        Some(super::lexer::Tok::Float(f)) => Value::Float(f),
                        _ => Value::Nil,
                    },
                    _ => Value::Nil,
                })
            }
            Pairs => {
                let t = arg(0);
                if let Value::Table(tb) = &t {
                    let pairs = tb
                        .borrow()
                        .meta
                        .as_ref()
                        .map(|m| m.borrow().get_str("__pairs"));
                    if let Some(p) = pairs.filter(|p| !matches!(p, Value::Nil)) {
                        return self.call(host, &p, vec![t.clone()]);
                    }
                    Ok(vec![Value::Builtin(Next), t, Value::Nil])
                } else {
                    rt_err("bad argument #1 to 'pairs' (table expected)")
                }
            }
            IPairs => Ok(vec![Value::Builtin(IPairsIter), arg(0), Value::Int(0)]),
            IPairsIter => {
                let i = arg(1).as_int().unwrap_or(0) + 1;
                let v = self.index(host, &arg(0), &Value::Int(i))?;
                if matches!(v, Value::Nil) {
                    Ok(vec![Value::Nil])
                } else {
                    Ok(vec![Value::Int(i), v])
                }
            }
            Next => {
                let Value::Table(t) = arg(0) else {
                    return rt_err("bad argument #1 to 'next' (table expected)");
                };
                let k = arg(1).to_key();
                match t.borrow().next(k.as_ref()) {
                    Some((k, v)) => Ok(vec![Value::from_key(&k), v]),
                    None => Ok(vec![Value::Nil]),
                }
            }
            Select => {
                let n = arg(0);
                if let Value::Str(s) = &n
                    && &**s == "#"
                {
                    return one(Value::Int(args.len() as i64 - 1));
                }
                let Some(i) = n.as_int() else {
                    return rt_err("bad argument #1 to 'select'");
                };
                let len = args.len() as i64 - 1;
                let start = if i < 0 { len + i + 1 } else { i };
                if start < 1 {
                    return rt_err("bad argument #1 to 'select' (index out of range)");
                }
                Ok(args.into_iter().skip(start as usize).collect())
            }
            Error => Err(LuaError { value: arg(0) }),
            Assert => {
                if arg(0).truthy() {
                    Ok(args)
                } else {
                    match arg(1) {
                        Value::Nil => rt_err("assertion failed!"),
                        v => Err(LuaError { value: v }),
                    }
                }
            }
            PCall => {
                let f = arg(0);
                match self.call(host, &f, args.into_iter().skip(1).collect()) {
                    Ok(mut r) => {
                        r.insert(0, Value::Bool(true));
                        Ok(r)
                    }
                    Err(e) => Ok(vec![Value::Bool(false), e.value]),
                }
            }
            SetMetatable => {
                let Value::Table(t) = arg(0) else {
                    return rt_err("bad argument #1 to 'setmetatable' (table expected)");
                };
                t.borrow_mut().meta = match arg(1) {
                    Value::Table(m) => Some(m),
                    _ => None,
                };
                one(arg(0))
            }
            GetMetatable => one(match arg(0) {
                Value::Table(t) => t
                    .borrow()
                    .meta
                    .clone()
                    .map(Value::Table)
                    .unwrap_or_default(),
                Value::Str(_) => Value::Table(self.string_lib.clone()),
                _ => Value::Nil,
            }),
            RawGet => match (arg(0), arg(1).to_key()) {
                (Value::Table(t), Some(k)) => one(t.borrow().get(&k)),
                (Value::Table(_), None) => one(Value::Nil),
                _ => rt_err("bad argument #1 to 'rawget' (table expected)"),
            },
            RawSet => match (arg(0), arg(1).to_key()) {
                (Value::Table(t), Some(k)) => {
                    t.borrow_mut().set(k, arg(2));
                    one(arg(0))
                }
                _ => rt_err("bad argument to 'rawset'"),
            },
            RawEqual => one(Value::Bool(arg(0).raw_eq(&arg(1)))),
            Require => {
                let name = tostring(&arg(0));
                let key = format!("__loaded:{name}");
                let cached = self.global(&key);
                if !matches!(cached, Value::Nil) {
                    return one(cached);
                }
                let m = host.require(self, &name)?;
                self.set_global(&key, m.clone());
                one(m)
            }
            Unpack => {
                let Value::Table(t) = arg(0) else {
                    return rt_err("bad argument #1 to 'unpack' (table expected)");
                };
                let t = t.borrow();
                let from = arg(1).as_int().unwrap_or(1);
                let to = arg(2).as_int().unwrap_or_else(|| t.len());
                Ok((from..=to).map(|i| t.get(&Key::Int(i))).collect())
            }
            TableInsert => {
                let Value::Table(t) = arg(0) else {
                    return rt_err("bad argument #1 to 'insert' (table expected)");
                };
                let mut t = t.borrow_mut();
                if args.len() >= 3 {
                    let pos = arg(1).as_int().unwrap_or(1);
                    let n = t.len();
                    let mut i = n;
                    while i >= pos {
                        let v = t.get(&Key::Int(i));
                        t.set(Key::Int(i + 1), v);
                        i -= 1;
                    }
                    t.set(Key::Int(pos), arg(2));
                } else {
                    t.push(arg(1));
                }
                Ok(vec![])
            }
            TableRemove => {
                let Value::Table(t) = arg(0) else {
                    return rt_err("bad argument #1 to 'remove' (table expected)");
                };
                let mut t = t.borrow_mut();
                let n = t.len();
                let pos = arg(1).as_int().unwrap_or(n);
                let v = t.get(&Key::Int(pos));
                for i in pos..n {
                    let next = t.get(&Key::Int(i + 1));
                    t.set(Key::Int(i), next);
                }
                if n >= 1 {
                    t.set(Key::Int(n), Value::Nil);
                }
                one(v)
            }
            TableConcat => {
                let Value::Table(t) = arg(0) else {
                    return rt_err("bad argument #1 to 'concat' (table expected)");
                };
                let sep = match arg(1) {
                    Value::Nil => String::new(),
                    v => tostring(&v),
                };
                let t = t.borrow();
                let parts: Vec<String> = (1..=t.len())
                    .map(|i| tostring(&t.get(&Key::Int(i))))
                    .collect();
                one(Value::Str(parts.join(&sep).into()))
            }
            MathFloor | MathCeil => {
                let v = arg(0);
                if let Value::Int(i) = v {
                    return one(Value::Int(i));
                }
                let Some(f) = v.as_number() else {
                    return rt_err("bad argument #1 to 'floor' (number expected)");
                };
                let r = if b == MathFloor { f.floor() } else { f.ceil() };
                // Lua 5.3: the result is an integer when it fits.
                one(if r.abs() < 9.2e18 {
                    Value::Int(r as i64)
                } else {
                    Value::Float(r)
                })
            }
            MathMax | MathMin => {
                let mut best = arg(0);
                if best.as_number().is_none() {
                    return rt_err("bad argument #1 to 'max' (number expected)");
                }
                for v in &args[1..] {
                    let Some(x) = v.as_number() else {
                        return rt_err("bad argument to 'max' (number expected)");
                    };
                    let cur = best.as_number().unwrap();
                    if (b == MathMax && x > cur) || (b == MathMin && x < cur) {
                        best = v.clone();
                    }
                }
                one(best)
            }
            MathAbs => one(match arg(0) {
                Value::Int(i) => Value::Int(i.wrapping_abs()),
                v => Value::Float(v.as_number().unwrap_or(0.0).abs()),
            }),
            StringFormat => one(Value::Str(string_format(&args)?.into())),
            StringRep => {
                let s = tostring(&arg(0));
                let n = arg(1).as_int().unwrap_or(0).max(0) as usize;
                one(Value::Str(s.repeat(n).into()))
            }
            StringSub => {
                let s = tostring(&arg(0));
                let len = s.len() as i64;
                let norm = |i: i64| if i < 0 { (len + i + 1).max(1) } else { i };
                let i = norm(arg(1).as_int().unwrap_or(1)).max(1);
                let j = norm(arg(2).as_int().unwrap_or(-1)).min(len);
                let out = if i > j {
                    String::new()
                } else {
                    String::from_utf8_lossy(&s.as_bytes()[(i - 1) as usize..j as usize])
                        .into_owned()
                };
                one(Value::Str(out.into()))
            }
            StringLen => one(Value::Int(tostring(&arg(0)).len() as i64)),
            StringLower => one(Value::Str(tostring(&arg(0)).to_lowercase().into())),
            StringUpper => one(Value::Str(tostring(&arg(0)).to_uppercase().into())),
            StringFind => {
                let s = tostring(&arg(0));
                let p = tostring(&arg(1));
                let init = arg(2).as_int().unwrap_or(1).max(1) as usize - 1;
                let plain = arg(3).truthy();
                let found = if plain {
                    s.get(init..)
                        .and_then(|h| h.find(&p))
                        .map(|i| (i + init, i + init + p.len()))
                } else {
                    let pat = super::pattern::Pattern::compile(&p)?;
                    pat.find(s.as_bytes(), init)
                };
                match found {
                    Some((a, e)) => Ok(vec![Value::Int(a as i64 + 1), Value::Int(e as i64)]),
                    None => one(Value::Nil),
                }
            }
            StringGsub => {
                let s = tostring(&arg(0));
                let pat = super::pattern::Pattern::compile(&tostring(&arg(1)))?;
                let repl = match arg(2) {
                    Value::Str(r) => r,
                    _ => return rt_err("string.gsub: only string replacements are supported"),
                };
                let (out, n) = pat.gsub(s.as_bytes(), repl.as_bytes());
                Ok(vec![
                    Value::Str(String::from_utf8_lossy(&out).into_owned().into()),
                    Value::Int(n as i64),
                ])
            }
        }
    }
}

/// `tostring` (Lua's `%.14g` for floats, `%d` for integers).
pub fn tostring(v: &Value) -> String {
    match v {
        Value::Float(f) => fmt_g14(*f),
        v => v.to_string(),
    }
}

fn fmt_g14(f: f64) -> String {
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f.is_nan() {
        return "nan".into();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{f:.1}");
    }
    let s = format!("{:.13e}", f);
    // Back to a plain decimal when it's short, close to %.14g.
    let plain = format!("{}", f);
    if plain.len() <= 17 { plain } else { s }
}

fn string_format(args: &[Value]) -> LResult<String> {
    let fmt = args.first().map(tostring).unwrap_or_default();
    let mut out = String::new();
    let mut it = args.iter().skip(1);
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() || d == '.' || d == '-' {
                spec.push(d);
                chars.next();
            } else {
                break;
            }
        }
        let Some(conv) = chars.next() else { break };
        let prec: Option<usize> = spec.split_once('.').and_then(|(_, p)| p.parse().ok());
        match conv {
            '%' => out.push('%'),
            's' => out.push_str(&tostring(it.next().unwrap_or(&Value::Nil))),
            'd' | 'i' => {
                let v = it.next().and_then(|v| v.as_number()).unwrap_or(0.0);
                out.push_str(&(v as i64).to_string());
            }
            'f' | 'g' => {
                let v = it.next().and_then(|v| v.as_number()).unwrap_or(0.0);
                out.push_str(&format!("{:.*}", prec.unwrap_or(6), v));
            }
            'x' => {
                let v = it.next().and_then(|v| v.as_int()).unwrap_or(0);
                out.push_str(&format!("{v:x}"));
            }
            'q' => out.push_str(&format!("{:?}", tostring(it.next().unwrap_or(&Value::Nil)))),
            other => return rt_err(format!("invalid conversion '%{other}' to 'format'")),
        }
    }
    Ok(out)
}

fn arith_err<T>(op: &str, a: &Value, b: &Value) -> LResult<T> {
    let bad = if a.as_number().is_none() { a } else { b };
    rt_err(format!(
        "attempt to perform {op} on a {} value",
        bad.type_name()
    ))
}

/// Coerce an operand of `&`/`|` that involves a `ConditionResult`.
// bg3: ls::khonsu CallOperator<ConditionResult,…> dispatchers (4.1.1.7631656): the overloads
// accept (CR, CR), (bool, CR), (CR, bool), (number, CR), (CR, number).
fn as_cond(v: &Value) -> LResult<ConditionResult> {
    match v {
        Value::Cond(c) => Ok(c.borrow().clone()),
        Value::Bool(b) => Ok(ConditionResult::new(*b)),
        Value::Int(_) | Value::Float(_) => Ok(ConditionResult::from_number(v.as_number().unwrap())),
        v => rt_err(format!(
            "NotImplemented: ConditionResult operator with a {} operand",
            v.type_name()
        )),
    }
}

pub fn binary(op: BinOp, a: &Value, b: &Value) -> LResult<Value> {
    use BinOp::*;
    Ok(match op {
        Add | Sub | Mul | Mod | IDiv => {
            if let (Some(x), Some(y)) = (int_operand(a), int_operand(b)) {
                match op {
                    Add => Value::Int(x.wrapping_add(y)),
                    Sub => Value::Int(x.wrapping_sub(y)),
                    Mul => Value::Int(x.wrapping_mul(y)),
                    Mod => {
                        if y == 0 {
                            return rt_err("attempt to perform 'n%%0'");
                        }
                        let r = x.wrapping_rem(y);
                        Value::Int(if r != 0 && (r ^ y) < 0 { r + y } else { r })
                    }
                    _ => {
                        if y == 0 {
                            return rt_err("attempt to perform 'n//0'");
                        }
                        let q = x.wrapping_div(y);
                        Value::Int(if (x % y != 0) && ((x < 0) != (y < 0)) {
                            q - 1
                        } else {
                            q
                        })
                    }
                }
            } else {
                let (Some(x), Some(y)) = (a.as_number(), b.as_number()) else {
                    return arith_err("arithmetic", a, b);
                };
                Value::Float(match op {
                    Add => x + y,
                    Sub => x - y,
                    Mul => x * y,
                    Mod => {
                        let r = x % y;
                        if r != 0.0 && (r < 0.0) != (y < 0.0) {
                            r + y
                        } else {
                            r
                        }
                    }
                    _ => (x / y).floor(),
                })
            }
        }
        Div | Pow => {
            let (Some(x), Some(y)) = (a.as_number(), b.as_number()) else {
                return arith_err("arithmetic", a, b);
            };
            Value::Float(if op == Div { x / y } else { x.powf(y) })
        }
        Concat => match (a, b) {
            (
                Value::Str(_) | Value::Int(_) | Value::Float(_),
                Value::Str(_) | Value::Int(_) | Value::Float(_),
            ) => Value::Str(format!("{}{}", tostring(a), tostring(b)).into()),
            _ => {
                let bad = if matches!(a, Value::Str(_) | Value::Int(_) | Value::Float(_)) {
                    b
                } else {
                    a
                };
                return rt_err(format!(
                    "attempt to concatenate a {} value",
                    bad.type_name()
                ));
            }
        },
        Eq => Value::Bool(a.raw_eq(b)),
        Ne => Value::Bool(!a.raw_eq(b)),
        Lt | Le | Gt | Ge => {
            let (x, y, op) = match op {
                Gt => (b, a, Lt),
                Ge => (b, a, Le),
                o => (a, b, o),
            };
            let r = match (x, y) {
                (Value::Str(s), Value::Str(t)) => {
                    if op == Lt {
                        s < t
                    } else {
                        s <= t
                    }
                }
                (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                    let (p, q) = (x.as_number().unwrap(), y.as_number().unwrap());
                    if op == Lt { p < q } else { p <= q }
                }
                _ => {
                    return rt_err(format!(
                        "attempt to compare {} with {}",
                        x.type_name(),
                        y.type_name()
                    ));
                }
            };
            Value::Bool(r)
        }
        BAnd | BOr if matches!(a, Value::Cond(_)) || matches!(b, Value::Cond(_)) => {
            let (x, y) = (as_cond(a)?, as_cond(b)?);
            Value::cond(if op == BAnd { x.and(&y) } else { x.or(&y) })
        }
        BAnd | BOr | BXor | Shl | Shr => {
            let (Some(x), Some(y)) = (bit_operand(a), bit_operand(b)) else {
                let bad = if bit_operand(a).is_none() { a } else { b };
                return rt_err(format!(
                    "attempt to perform bitwise operation on a {} value",
                    bad.type_name()
                ));
            };
            Value::Int(match op {
                BAnd => x & y,
                BOr => x | y,
                BXor => x ^ y,
                Shl => shift_left(x, y),
                _ => shift_left(x, y.wrapping_neg()),
            })
        }
        And | Or => unreachable!("short-circuit operators are evaluated by the interpreter"),
    })
}

fn shift_left(x: i64, n: i64) -> i64 {
    if n <= -64 || n >= 64 {
        0
    } else if n >= 0 {
        ((x as u64) << n) as i64
    } else {
        ((x as u64) >> (-n)) as i64
    }
}

fn int_operand(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

fn bit_operand(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Float(_) | Value::Str(_) => v.as_int(),
        _ => None,
    }
}

pub fn unary(op: UnOp, a: &Value) -> LResult<Value> {
    Ok(match op {
        UnOp::Not => Value::Bool(!a.truthy()),
        UnOp::Neg => match a {
            Value::Int(i) => Value::Int(i.wrapping_neg()),
            v => match v.as_number() {
                Some(f) => Value::Float(-f),
                None => {
                    return rt_err(format!(
                        "attempt to perform arithmetic on a {} value",
                        v.type_name()
                    ));
                }
            },
        },
        UnOp::Len => match a {
            Value::Str(s) => Value::Int(s.len() as i64),
            Value::Table(t) => Value::Int(t.borrow().len()),
            v => {
                return rt_err(format!(
                    "attempt to get length of a {} value",
                    v.type_name()
                ));
            }
        },
        UnOp::BNot => match a {
            Value::Cond(c) => Value::cond(c.borrow().not()),
            v => match bit_operand(v) {
                Some(i) => Value::Int(!i),
                None => {
                    return rt_err(format!(
                        "attempt to perform bitwise operation on a {} value",
                        v.type_name()
                    ));
                }
            },
        },
    })
}
