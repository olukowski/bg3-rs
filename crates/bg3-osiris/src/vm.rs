//! The Osiris virtual machine: rete evaluation, actions, goals.
//!
//! Evaluation is synchronous and depth-first, exactly like libOsiris: an action that inserts a
//! fact or calls a PROC runs every rule it triggers (recursively) before the next action of the
//! current rule. See `docs/research/osiris-vm.md` for the semantics and the original symbols.

use std::collections::HashMap;

use bg3_story::{FunctionKind, Story, SubGoalCombiner};

use crate::db::Db;
use crate::host::{Host, HostCtx, display, make_string};
use crate::program::{
    ActionKind, Conn, PAction, PAdapter, PKind, PNode, Param, Program, other_entry,
};
use crate::trace::{ActionKind as LogKind, LogData, LogValue, QueryArg, Record};
use crate::value::{Data, Root, StringTable, TypeId, Value, osi_compare, osi_eq, types};

/// A token: rule-variable bindings (`COsiVarIDValuePairList`), in insertion order.
pub type Token = Vec<(u8, Value)>;

const ENTRY_LEFT: u32 = 1;

/// Goal state as seen from outside (`COsiGoal` flags byte).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GoalState {
    /// Rules disabled, INIT not yet run (flag bit 1).
    Sleeping,
    /// Rules enabled.
    Active,
    /// `GoalCompleted` ran (flag bit 0): EXIT has run, rules are off.
    Completed,
}

impl GoalState {
    fn from_flags(f: u8) -> Self {
        if f & 1 != 0 {
            Self::Completed
        } else if f & 2 != 0 {
            Self::Sleeping
        } else {
            Self::Active
        }
    }
}

/// What happened to an injected event (`COsiris::Event` return code).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventOutcome {
    /// Passed to the event's start node.
    Processed,
    /// The story declares the event but no rule uses it.
    NoRules,
    /// No such event (name/arity) in the story.
    Unknown,
    /// An argument has no type; Osiris ignores the event.
    BadArguments,
}

/// Counters for diagnostics.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub events: u64,
    pub calls: u64,
    pub engine_queries: u64,
    pub user_queries: u64,
    pub internal_queries: u64,
    pub rules_fired: u64,
    pub actions_fired: u64,
    pub facts_inserted: u64,
    pub facts_deleted: u64,
    pub max_rule_nesting: u32,
}

/// Problems the VM ran into. Most mirror an assertion or log message in libOsiris that does
/// not stop execution; the VM carries on the way the original would.
#[derive(Clone, Debug, PartialEq)]
pub struct VmError(pub String);

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

const MAX_RECORDED_ERRORS: usize = 1000;

#[derive(Debug)]
struct State {
    dbs: Vec<Option<Db>>,
    goal_flags: Vec<u8>,
    rule_vars: Vec<Vec<Value>>,
    /// `CReteBinaryNode` +0x38: token waiting for a generator replay.
    pending: HashMap<u32, Token>,
    /// `g_OsiUserQueriesExecuted`.
    user_queries_executed: i64,
    /// `g_RuleNesting`.
    rule_nesting: u32,
    max_rule_nesting: u32,
    stats: Stats,
    errors: Vec<VmError>,
    error_count: usize,
    /// Runtime log records (`COsiris::m_LogEnabled` with `_ParserFlags` bit 19), if enabled.
    log: Option<Vec<Record>>,
}

/// A loaded story, ready to run.
pub struct Vm {
    pub(crate) prog: Program,
    st: State,
    pub(crate) strings: StringTable,
}

impl Vm {
    /// Load a story (`COsiris::Load`). Facts stored in the story (saves) are loaded into the
    /// databases; goal flags are taken as stored.
    pub fn new(story: &Story) -> Self {
        let mut strings = StringTable::default();
        let (prog, problems) = Program::build(story, &mut strings);
        let mut dbs: Vec<Option<Db>> = prog.db_arity.iter().map(|a| a.map(Db::new)).collect();
        let mut errors: Vec<VmError> = problems.into_iter().map(VmError).collect();
        for d in &story.databases {
            let db = dbs[d.id.get() as usize].as_mut().expect("db exists");
            for fact in &d.facts {
                let row: Vec<Value> = fact
                    .iter()
                    .map(|v| crate::program::stored_value(story, &prog.types, &mut strings, v))
                    .collect();
                if !db.insert(&row) {
                    errors.push(VmError(format!(
                        "duplicate stored fact in database {}",
                        d.id
                    )));
                }
            }
        }
        let goal_flags = prog
            .goals
            .iter()
            .map(|g| g.as_ref().map_or(0, |g| g.flags))
            .collect();
        let rule_vars = prog
            .nodes
            .iter()
            .map(|n| match n.as_ref().map(|n| &n.kind) {
                Some(PKind::Rule(r)) => r.variables.clone(),
                _ => Vec::new(),
            })
            .collect();
        let error_count = errors.len();
        Self {
            prog,
            st: State {
                dbs,
                goal_flags,
                rule_vars,
                pending: HashMap::new(),
                user_queries_executed: 0,
                rule_nesting: 0,
                max_rule_nesting: 2000,
                stats: Stats::default(),
                errors,
                error_count,
                log: None,
            },
            strings,
        }
    }

    /// Limit on nested rule firings (libOsiris has none and would overflow its stack). Rules
    /// beyond the limit are skipped with an error.
    pub fn set_max_rule_nesting(&mut self, n: u32) {
        self.st.max_rule_nesting = n;
    }

    /// Turn the runtime log on or off. While on, the VM records the same events libOsiris
    /// writes to its log (see [`crate::trace`]); fetch them with [`Vm::take_log`].
    pub fn set_logging(&mut self, on: bool) {
        self.st.log = on.then(Vec::new);
    }

    fn push_log(&mut self, r: Option<Record>) {
        if let (Some(log), Some(r)) = (self.st.log.as_mut(), r) {
            log.push(r);
        }
    }

    /// The log records written since the last call (empty if logging is off).
    pub fn take_log(&mut self) -> Vec<Record> {
        self.st.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// A value as the runtime log prints it.
    pub fn log_value(&self, v: &Value) -> LogValue {
        log_value(&self.prog, &self.strings, v)
    }

    fn exec<'a>(&'a mut self, host: &'a mut dyn Host) -> Exec<'a> {
        Exec {
            p: &self.prog,
            s: &mut self.st,
            strings: &mut self.strings,
            host,
        }
    }

    /// `COsiris::InitGame`: fire the global actions, then the INIT section of every goal whose
    /// rules are enabled (flag bit 1 clear), in goal-id order. For a compiled story these are
    /// the root goals; subgoals wake up when their parents complete.
    // bg3: COsiris::InitGame (4.1.1.7631656)
    pub fn init_game(&mut self, host: &mut dyn Host) {
        let mut x = self.exec(host);
        let p = x.p;
        x.emit(|_| Record::GlobalInit);
        for a in &p.global_actions {
            x.fire(a, None);
        }
        for gid in 0..p.goals.len() {
            let Some(goal) = p.goal(gid as u32) else {
                continue;
            };
            // Checked when the loop reaches the goal: a goal enabled earlier in this loop by a
            // completed parent (which already ran its INIT) runs INIT again here.
            if x.s.goal_flags[gid] & 2 == 0 {
                x.emit(|_| Record::GoalSection {
                    exit: false,
                    goal: goal.name.clone(),
                    id: gid as u32,
                    woken: false,
                });
                for a in &goal.init {
                    x.fire(a, None);
                }
            }
        }
    }

    /// Inject an engine event (`COsiris::Event`). Arguments keep their type id when their
    /// root matches the declared parameter (libOsiris does not retag engine values); values of
    /// another root are converted.
    // bg3: COsiris::Event (4.1.1.7631656)
    pub fn event(&mut self, host: &mut dyn Host, name: &str, args: &[Value]) -> EventOutcome {
        let Some(f) = self.prog.function(name, args.len()) else {
            return EventOutcome::Unknown;
        };
        let logging = self.st.log.is_some();
        self.push_log(logging.then_some(Record::EventCall));
        if args.iter().any(|a| a.ty == 0 || a.ty == 0x7f) {
            self.push_log(
                logging.then(|| Record::EventRejected("type of parameter unknown".to_owned())),
            );
            return EventOutcome::BadArguments;
        }
        let node = self.prog.functions[f].node;
        if self.prog.node(node).is_none() {
            self.push_log(logging.then(|| Record::EventIgnored {
                name: self.prog.functions[f].name.clone(),
                arity: args.len() as u32,
            }));
            return EventOutcome::NoRules;
        }
        self.push_log(logging.then(|| Record::Event {
            name: self.prog.functions[f].name.clone(),
            args: args.iter().map(|a| self.log_value(a)).collect(),
        }));
        let params = self.prog.functions[f].params.clone();
        let args: Vec<Value> = args
            .iter()
            .zip(&params)
            .map(|(a, &ty)| coerce(&mut self.strings, &self.prog, *a, ty))
            .collect();
        let mut x = self.exec(host);
        x.s.stats.events += 1;
        x.start_add(node, args);
        EventOutcome::Processed
    }

    /// Whether the story uses event/function `name`/`arity` in a rule (`None`: unknown).
    pub fn event_has_rules(&self, name: &str, arity: usize) -> Option<bool> {
        let f = self.prog.function(name, arity)?;
        Some(self.prog.node(self.prog.functions[f].node).is_some())
    }

    /// Declared parameter types of a function.
    pub fn function_params(&self, name: &str, arity: usize) -> Option<Vec<TypeId>> {
        let f = self.prog.function(name, arity)?;
        Some(self.prog.functions[f].params.clone())
    }

    /// Rows of database `name`/`arity` in insertion order, or `None` if there is no such
    /// database.
    pub fn db(&self, name: &str, arity: usize) -> Option<Vec<Vec<Value>>> {
        let db = self.db_ref(name, arity)?;
        Some(db.rows().map(|r| r.to_vec()).collect())
    }

    /// Number of rows of a database.
    pub fn db_len(&self, name: &str, arity: usize) -> Option<usize> {
        self.db_ref(name, arity).map(Db::len)
    }

    fn db_ref(&self, name: &str, arity: usize) -> Option<&Db> {
        let f = self.prog.function(name, arity)?;
        let node = self.prog.node(self.prog.functions[f].node)?;
        self.st.dbs.get(node.db as usize)?.as_ref()
    }

    /// All story databases: `(name, arity, rows)`, in function-table order.
    pub fn databases(&self) -> Vec<(&str, usize, usize)> {
        self.prog
            .functions
            .iter()
            .filter(|f| f.kind == FunctionKind::Database)
            .filter_map(|f| {
                let node = self.prog.node(f.node)?;
                let db = self.st.dbs.get(node.db as usize)?.as_ref()?;
                Some((f.name.as_str(), f.params.len(), db.len()))
            })
            .collect()
    }

    pub fn goal_state(&self, name: &str) -> Option<GoalState> {
        let id = self.prog.goal_by_name(name)?;
        Some(GoalState::from_flags(self.st.goal_flags[id as usize]))
    }

    /// `(name, state)` of every goal in id order.
    pub fn goals(&self) -> Vec<(&str, GoalState)> {
        self.prog
            .goals
            .iter()
            .enumerate()
            .filter_map(|(i, g)| {
                g.as_ref().map(|g| {
                    (
                        g.name.as_str(),
                        GoalState::from_flags(self.st.goal_flags[i]),
                    )
                })
            })
            .collect()
    }

    /// Recorded problems (first 1000) and the total count.
    pub fn errors(&self) -> (&[VmError], usize) {
        (&self.st.errors, self.st.error_count)
    }

    pub fn stats(&self) -> Stats {
        let mut s = self.st.stats.clone();
        s.max_rule_nesting = s.max_rule_nesting.max(self.st.rule_nesting);
        s
    }

    pub fn string(&mut self, s: &str) -> Value {
        make_string(&mut self.strings, types::STRING, Root::String, s)
    }

    pub fn guid(&mut self, s: &str) -> Value {
        make_string(&mut self.strings, types::GUIDSTRING, Root::GuidString, s)
    }

    pub fn text(&self, v: &Value) -> Option<&str> {
        v.as_str().map(|s| self.strings.text(s))
    }

    /// Render a value (strings quoted, GUIDs bare).
    pub fn display(&self, v: &Value) -> String {
        display(&self.strings, v)
    }

    /// `Compare` as a compare node would evaluate it (for tests and tools).
    pub fn compare(&self, a: &Value, b: &Value, op: bg3_story::RelOp) -> Option<bool> {
        osi_compare(&self.strings, a, b, op).ok()
    }
}

/// Fit a host-provided value to a declared parameter type. A value whose root already
/// matches keeps its own type id: libOsiris does not retag engine values (a GUIDSTRING passed
/// for a `CHARACTER` parameter stays a GUIDSTRING; the runtime log prints it without the alias
/// prefix). Only a value of another root is converted.
fn coerce(strings: &mut StringTable, p: &Program, v: Value, ty: TypeId) -> Value {
    if v.ty == ty || ty == 0 {
        return v;
    }
    let root = p.types.root(ty);
    if root == v.root {
        return v;
    }
    match (v.data, root) {
        (Data::Str(s), Root::String | Root::GuidString) => {
            let text = strings.text(s).to_owned();
            make_string(strings, ty, root, &text)
        }
        _ => v,
    }
}

/// A value as libOsiris prints it in its runtime log: alias types get a `(TYPE)` prefix,
/// enumerations print as `TYPE.LABEL`, unset values as `(unknown)` / `(TYPE)`.
// bg3: COsiTypedValueBase printing in the runtime log (4.1.1.7631656)
pub(crate) fn log_value(p: &Program, strings: &StringTable, v: &Value) -> LogValue {
    let name = |ty: TypeId| p.type_names.get(ty as usize).cloned().unwrap_or_default();
    if !v.is_valid() {
        return LogValue {
            ty: (v.ty != 0).then(|| name(v.ty)),
            v: LogData::Unset,
        };
    }
    if let Some(labels) = p.enums.get(&v.ty) {
        let raw = match v.data {
            Data::Int(i) => i as i64,
            Data::Int64(i) => i,
            _ => 0,
        };
        // Several labels can share a value (`__OSIRIS_ENUM_FALLBACK = 3`, `DOT = 3`); the log
        // names the real label.
        let label = labels
            .iter()
            .find(|(l, x)| *x as i64 == raw && l != "__OSIRIS_ENUM_FALLBACK")
            .or_else(|| labels.iter().find(|(_, x)| *x as i64 == raw))
            .map_or_else(|| format!("invalid_{raw}"), |(l, _)| l.clone());
        return LogValue {
            ty: None,
            v: LogData::Enum(name(v.ty), label),
        };
    }
    let ty = if v.ty > types::GUIDSTRING || v.ty == types::INTEGER64 {
        Some(name(v.ty))
    } else {
        None
    };
    let data = match v.data {
        Data::Undef => LogData::Unset,
        Data::Int(i) => LogData::Int(i as i64),
        Data::Int64(i) => LogData::Int(i),
        Data::Real(f) => LogData::Real(f as f64),
        Data::Str(s) => {
            let text = strings.text(s).to_owned();
            if v.root == Root::String {
                LogData::Str(text)
            } else {
                LogData::Guid(text)
            }
        }
    };
    LogValue { ty, v: data }
}

/// [`log_value`] for a host callback.
pub(crate) fn log_value_ctx(ctx: &HostCtx<'_>, v: &Value) -> LogValue {
    log_value(ctx.prog, ctx.strings, v)
}

/// `COsiVariable::Assign`: take the source's type; the payload only if the type is set and
/// the source is valid.
// bg3: COsiVariable::Assign(COsiTypedValue const&) (4.1.1.7631656)
fn assign(v: &Value) -> Value {
    if v.ty == 0 { Value::UNDEF } else { *v }
}

/// `COsiVarIDValuePairList::Merge`: all of `a`, then the entries of `b` whose variable is not
/// in `a`.
// bg3: COsiVarIDValuePairList::Merge (4.1.1.7631656)
fn merge(a: &Token, b: &Token) -> Token {
    let mut out = a.clone();
    for &(var, v) in b {
        if !a.iter().any(|&(k, _)| k == var) {
            out.push((var, v));
        }
    }
    out
}

struct Exec<'a> {
    p: &'a Program,
    s: &'a mut State,
    strings: &'a mut StringTable,
    host: &'a mut dyn Host,
}

impl<'a> Exec<'a> {
    /// Append a runtime-log record (built only if logging is on).
    fn emit(&mut self, f: impl FnOnce(&Self) -> Record) {
        if self.s.log.is_some() {
            let r = f(self);
            if let Some(log) = self.s.log.as_mut() {
                log.push(r);
            }
        }
    }

    fn lv(&self, v: &Value) -> LogValue {
        log_value(self.p, self.strings, v)
    }

    fn lvs(&self, v: &[Value]) -> Vec<LogValue> {
        v.iter().map(|x| self.lv(x)).collect()
    }

    fn err(&mut self, msg: impl Into<String>) {
        self.s.error_count += 1;
        if self.s.errors.len() < MAX_RECORDED_ERRORS {
            self.s.errors.push(VmError(msg.into()));
        }
    }

    fn eq(&mut self, a: &Value, b: &Value) -> bool {
        match osi_eq(a, b) {
            Ok(r) => r,
            Err(()) => {
                let msg = format!(
                    "invalid comparison {} == {}",
                    display(self.strings, a),
                    display(self.strings, b)
                );
                self.err(msg);
                false
            }
        }
    }

    fn node(&self, id: u32) -> Option<&'a PNode> {
        self.p.node(id)
    }

    fn db_arity(&self, db: u32) -> usize {
        self.p
            .db_arity
            .get(db as usize)
            .copied()
            .flatten()
            .unwrap_or(0)
    }

    fn has_db(&self, node: &PNode) -> bool {
        node.db != 0 && matches!(self.s.dbs.get(node.db as usize), Some(Some(_)))
    }

    fn db_mut(&mut self, db: u32) -> &mut Db {
        self.s.dbs[db as usize].as_mut().expect("checked by has_db")
    }

    // ---- adaptors ----

    /// `Adaptor(entry)` of a node: the adaptor on the edge into it.
    // bg3: CReteBinaryNode::Adaptor, CReteUnaryNode::Adaptor (4.1.1.7631656)
    fn adaptor_of(&mut self, node: u32, entry: u32) -> Option<&'a PAdapter> {
        let n = self.node(node)?;
        let id = match &n.kind {
            PKind::Join(j) => {
                if entry == 0 {
                    self.err(format!("node {node}: can't have adaptor"));
                }
                if entry == ENTRY_LEFT {
                    j.left_adapter
                } else {
                    j.right_adapter
                }
            }
            PKind::Compare(c) => c.adapter,
            PKind::Rule(r) => r.adapter,
            _ => {
                self.err(format!("node {node}: start/query nodes have no adaptor"));
                return None;
            }
        };
        let a = self.p.adapter(id);
        if a.is_none() {
            self.err(format!("node {node}: missing adaptor {id}"));
        }
        a
    }

    /// Physical tuple → token: variable columns become bindings, constant columns must match
    /// (walking the constants in order), and a variable bound by two columns must get equal
    /// values. `None` = the tuple does not fit.
    // bg3: CReteAdaptor::token(CTuple const&) (4.1.1.7631656)
    fn token(&mut self, ad: &PAdapter, tuple: &[Value]) -> Option<Token> {
        let mut out = Token::with_capacity(tuple.len());
        let mut next_const = 0;
        for (col, v) in tuple.iter().enumerate() {
            let logical = ad.logical.get(col).copied().unwrap_or(-1);
            if logical == -1 {
                if let Some((c, cv)) = ad.constants.get(next_const)
                    && *c as usize == col
                {
                    next_const += 1;
                    if !self.eq(v, cv) {
                        return None;
                    }
                }
            } else {
                out.push((logical as u8, *v));
            }
        }
        for i in 0..out.len() {
            for j in 0..i {
                if out[i].0 == out[j].0 {
                    let (a, b) = (out[j].1, out[i].1);
                    if !self.eq(&a, &b) {
                        return None;
                    }
                    break;
                }
            }
        }
        Some(out)
    }

    /// Select descriptor for looking up a database through this adaptor: the token's bound
    /// variables that map to columns, then the constants.
    // bg3: CReteAdaptor::selectDesc (4.1.1.7631656)
    fn select_desc(ad: &PAdapter, token: &Token) -> Vec<(u8, Value)> {
        let mut out = Vec::new();
        for &(var, v) in token {
            if let Some(col) = ad.physical(var)
                && col != 0xff
            {
                out.push((col, v));
            }
        }
        out.extend(ad.constants.iter().copied());
        out
    }

    /// Token → physical tuple of `arity` columns (unset where nothing maps).
    // bg3: CReteAdaptor::InitTuple (4.1.1.7631656)
    fn init_tuple(ad: Option<&PAdapter>, token: &Token, arity: usize) -> Vec<Value> {
        let mut tuple = vec![Value::UNDEF; arity];
        let Some(ad) = ad else {
            return tuple;
        };
        for &(var, v) in token {
            if let Some(col) = ad.physical(var)
                && let Some(slot) = tuple.get_mut(col as usize)
            {
                *slot = v;
            }
        }
        for &(col, v) in &ad.constants {
            if let Some(slot) = tuple.get_mut(col as usize) {
                *slot = v;
            }
        }
        tuple
    }

    /// Every variable the adaptor maps, unset, in variable order.
    // bg3: CReteAdaptor::undefToken (4.1.1.7631656)
    fn undef_token(ad: Option<&PAdapter>) -> Token {
        ad.map_or_else(Vec::new, |ad| {
            ad.map.iter().map(|&(var, _)| (var, Value::UNDEF)).collect()
        })
    }

    // ---- start nodes ----

    /// `CReteStartNode::Add(COsipParameterList*)`: a DB start node stores the tuple first and
    /// only propagates new facts; event/proc start nodes always propagate.
    // bg3: CReteStartNode::Add(COsipParameterList*) (4.1.1.7631656)
    fn start_add(&mut self, node: u32, tuple: Vec<Value>) {
        let Some(n) = self.node(node) else {
            self.err(format!("start node {node} missing"));
            return;
        };
        if self.has_db(n) {
            let db = self.db_mut(n.db);
            if db.arity() == 0 {
                if !tuple.is_empty() {
                    return;
                }
            } else {
                if db.find(&tuple).is_some() {
                    self.emit(|x| Record::InsertFailed {
                        start: true,
                        tuple: x.lvs(&tuple),
                    });
                    return;
                }
                db.push(&tuple);
            }
            self.s.stats.facts_inserted += 1;
        }
        self.forward_add(node, &tuple);
    }

    // bg3: CReteStartNode::Del(COsipParameterList*) (4.1.1.7631656)
    fn start_del(&mut self, node: u32, tuple: Vec<Value>) {
        let Some(n) = self.node(node) else {
            self.err(format!("start node {node} missing"));
            return;
        };
        if self.has_db(n) && self.db_mut(n.db).erase(&tuple) {
            self.s.stats.facts_deleted += 1;
            self.forward_del(node, &tuple);
        }
    }

    /// Successors that belong to a goal whose rules are disabled are skipped; the goal state is
    /// read as each successor is reached.
    fn successor_enabled(&mut self, c: &Conn) -> bool {
        if c.goal == 0 {
            return true;
        }
        match self.s.goal_flags.get(c.goal as usize) {
            Some(f) if self.p.goal(c.goal).is_some() => f & 2 == 0,
            _ => {
                self.err(format!("goal {} not in use", c.goal));
                true
            }
        }
    }

    // bg3: CReteStartNode::ForwardAddToken (4.1.1.7631656)
    fn forward_add(&mut self, node: u32, tuple: &[Value]) {
        let Some(PKind::Start { successors }) = self.node(node).map(|n| &n.kind) else {
            return;
        };
        for c in successors {
            if !self.successor_enabled(c) {
                continue;
            }
            let Some(ad) = self.adaptor_of(c.node, c.entry) else {
                continue;
            };
            if let Some(t) = self.token(ad, tuple) {
                self.add(c.node, t, node, c.entry);
            }
        }
    }

    // bg3: CReteStartNode::ForwardDelToken (4.1.1.7631656)
    fn forward_del(&mut self, node: u32, tuple: &[Value]) {
        let Some(PKind::Start { successors }) = self.node(node).map(|n| &n.kind) else {
            return;
        };
        for c in successors {
            if !self.successor_enabled(c) {
                continue;
            }
            let Some(ad) = self.adaptor_of(c.node, c.entry) else {
                continue;
            };
            if let Some(t) = self.token(ad, tuple) {
                self.del(c.node, t, node, c.entry);
            }
        }
    }

    // ---- dispatch ----

    fn add(&mut self, node: u32, token: Token, from: u32, entry: u32) {
        let Some(n) = self.node(node) else {
            return;
        };
        match &n.kind {
            PKind::Join(j) if j.negated => self.nand_add(node, token, from, entry),
            PKind::Join(_) => self.join(node, token, from, entry),
            PKind::Compare(_) => self.rel_add(node, token),
            PKind::Rule(_) => self.rule_add(node, token),
            // CReteStartNode::Add(token) and CReteQuery::Add are empty.
            _ => {}
        }
    }

    fn del(&mut self, node: u32, token: Token, from: u32, entry: u32) {
        let Some(n) = self.node(node) else {
            return;
        };
        match &n.kind {
            PKind::Join(j) if j.negated => self.nand_del(node, token, from, entry),
            // CReteAnd::Del is CReteChildNode::Del; compare nodes inherit it.
            PKind::Join(_) | PKind::Compare(_) => self.child_del(node, token),
            // CReteRuleActionPart::Del, CReteStartNode::Del(token), CReteQuery::Del are empty.
            _ => {}
        }
    }

    /// `Valid(token, adaptor)`: does the subnetwork rooted at `node` hold for the token?
    /// May bind more variables (query out-parameters).
    fn valid(&mut self, node: u32, token: &mut Token, adapter: u32) -> bool {
        let Some(n) = self.node(node) else {
            self.err(format!("valid: node {node} missing"));
            return false;
        };
        let ad = self.p.adapter(adapter);
        match &n.kind {
            PKind::Start { .. } => {
                if self.has_db(n) {
                    // bg3: CReteFact::Valid (4.1.1.7631656)
                    self.db_valid(n.db, ad, token)
                } else {
                    // bg3: CReteEvent::Valid (4.1.1.7631656) — always false
                    false
                }
            }
            PKind::Join(j) => {
                if self.has_db(n) {
                    return self.db_valid(n.db, ad, token);
                }
                if !j.negated {
                    // bg3: CReteAnd::Valid (4.1.1.7631656)
                    self.valid(j.left_parent, token, j.left_adapter)
                        && self.valid(j.right_parent, token, j.right_adapter)
                } else {
                    // bg3: CReteNAnd::Valid (4.1.1.7631656)
                    let mut left = merge(token, &Self::undef_token(self.p.adapter(j.left_adapter)));
                    let mut right =
                        merge(token, &Self::undef_token(self.p.adapter(j.right_adapter)));
                    if self.valid(j.left_parent, &mut left, j.left_adapter)
                        && !self.valid(j.right_parent, &mut right, j.right_adapter)
                    {
                        *token = merge(&left, &right);
                        true
                    } else {
                        false
                    }
                }
            }
            PKind::Compare(c) => {
                // bg3: CReteRelCondition::Valid (4.1.1.7631656)
                if self.has_db(n) {
                    return self.db_valid(n.db, ad, token);
                }
                if !self.valid(c.parent, token, c.adapter) {
                    return false;
                }
                self.condition(node, token)
            }
            PKind::Rule(_) => {
                self.err(format!("node {node}: Valid on a rule node"));
                false
            }
            PKind::DivQuery | PKind::InternalQuery | PKind::UserQuery => {
                self.query_valid(n, token, ad)
            }
        }
    }

    fn db_valid(&mut self, db: u32, ad: Option<&PAdapter>, token: &Token) -> bool {
        let tuple = Self::init_tuple(ad, token, self.db_arity(db));
        self.s.dbs[db as usize]
            .as_ref()
            .is_some_and(|d| d.valid(&tuple))
    }

    // ---- inner nodes ----

    /// The edge out of an inner node.
    fn next_of(&self, node: u32) -> Option<Conn> {
        match &self.node(node)?.kind {
            PKind::Join(j) => Some(j.next),
            PKind::Compare(c) => Some(c.next),
            _ => None,
        }
    }

    /// `CReteChildNode::Add`: materialise the token into this node's database (if it has one;
    /// a known tuple stops propagation), then pass it to the successor.
    // bg3: CReteChildNode::Add (4.1.1.7631656)
    fn child_add(&mut self, node: u32, token: Token) {
        let Some(n) = self.node(node) else {
            return;
        };
        let next = self.next_of(node);
        if self.has_db(n) {
            let ad = match next {
                Some(c) => self.adaptor_of(c.node, c.entry),
                None => None,
            };
            let arity = self.db_arity(n.db);
            if arity != 0 {
                let tuple = Self::init_tuple(ad, &token, arity);
                let db = self.db_mut(n.db);
                if db.find(&tuple).is_some() {
                    self.emit(|x| Record::InsertFailed {
                        start: false,
                        tuple: x.lvs(&tuple),
                    });
                    return;
                }
                db.push(&tuple);
            }
        }
        if let Some(c) = next
            && c.node != 0
        {
            self.add(c.node, token, node, c.entry);
        }
    }

    /// `CReteChildNode::Del`: only nodes with a database propagate deletions, and only when
    /// something was erased.
    // bg3: CReteChildNode::Del (4.1.1.7631656)
    fn child_del(&mut self, node: u32, token: Token) {
        let Some(n) = self.node(node) else {
            return;
        };
        if !self.has_db(n) {
            return;
        }
        let next = self.next_of(node);
        let ad = match next {
            Some(c) => self.adaptor_of(c.node, c.entry),
            None => None,
        };
        let tuple = Self::init_tuple(ad, &token, self.db_arity(n.db));
        if self.db_mut(n.db).erase(&tuple)
            && let Some(c) = next
            && c.node != 0
        {
            self.del(c.node, token, node, c.entry);
        }
    }

    /// The token generator of a node: the closest database link (left wins ties).
    // bg3: CReteBinaryNode::pTokenGenerator, CReteBinaryNode::GeneratorConnection,
    //      CReteUnaryNode::pTokenGenerator (4.1.1.7631656)
    fn generator(&self, node: &PNode) -> Option<(u32, Conn)> {
        let link = match &node.kind {
            PKind::Join(j) => {
                if j.left_link.indirection <= j.right_link.indirection {
                    j.left_link
                } else {
                    j.right_link
                }
            }
            PKind::Compare(c) => c.link,
            _ => return None,
        };
        self.node(link.node).map(|_| (link.node, link.conn))
    }

    /// AND join (`CReteBinaryNode::Join` / `JoinInternal`). The token came in on `entry` from
    /// `from`; the *other* parent is consulted:
    /// 1. while this node waits for a generator replay, the token is the replayed one: merge
    ///    with the waiting token and pass on;
    /// 2. other parent has a database: select the matching rows (snapshot, insertion order)
    ///    and pass each merged token through `child_add`;
    /// 3. other parent can regenerate its tokens from a database: replay them into it;
    /// 4. otherwise (queries, events) ask `Valid` and pass the token on if it holds.
    // bg3: CReteBinaryNode::Join, CReteBinaryNode::JoinInternal (4.1.1.7631656)
    fn join(&mut self, node: u32, token: Token, from: u32, entry: u32) {
        let Some(PKind::Join(j)) = self.node(node).map(|n| &n.kind) else {
            return;
        };
        if let Some(waiting) = self.s.pending.get(&node) {
            let out = merge(waiting, &token);
            if j.next.node != 0 {
                self.add(j.next.node, out, node, j.next.entry);
            }
            return;
        }
        let other_id = if j.left_parent == from {
            j.right_parent
        } else {
            j.left_parent
        };
        let other_entry = other_entry(entry);
        let Some(other) = self.node(other_id) else {
            self.err(format!("join {node}: parent {other_id} missing"));
            return;
        };
        if self.has_db(other) {
            let Some(ad) = self.adaptor_of(node, other_entry) else {
                return;
            };
            let desc = Self::select_desc(ad, &token);
            let db = self.s.dbs[other.db as usize].as_ref().expect("has_db");
            // The matches are copied before propagating (CTupleList), so changes made by the
            // rules we trigger do not affect this iteration.
            let rows: Vec<Box<[Value]>> = db
                .select(&desc)
                .into_iter()
                .map(|s| db.row(s).into())
                .collect();
            for row in rows {
                if let Some(t) = self.token(ad, &row) {
                    let m = merge(&token, &t);
                    self.child_add(node, m);
                }
            }
        } else if let Some((gen_node, conn)) = self.generator(other) {
            self.s.pending.insert(node, token.clone());
            self.generate(gen_node, &token, conn);
            // Cleared, not restored: a nested wait on the same node loses the outer one.
            self.s.pending.remove(&node);
        } else {
            let ad = self.adaptor_of(node, other_entry);
            let mut out = merge(&token, &Self::undef_token(ad));
            let ad_id = ad.map_or(0, |a| a.id);
            if self.valid(other_id, &mut out, ad_id) && j.next.node != 0 {
                self.add(j.next.node, out, node, j.next.entry);
            }
        }
    }

    /// Replay the rows of `gen` (a database start node) that match `token` into the first
    /// node below it; they flow down to the join that waits for them.
    // bg3: CReteNode::GenerateAllSatisfying (4.1.1.7631656)
    fn generate(&mut self, gen_node: u32, token: &Token, conn: Conn) {
        let Some(g) = self.node(gen_node) else {
            return;
        };
        if !self.has_db(g) {
            self.err(format!("generator {gen_node} should have a database"));
            return;
        }
        let Some(ad) = self.adaptor_of(conn.node, conn.entry) else {
            return;
        };
        let desc = Self::select_desc(ad, token);
        let db = self.s.dbs[g.db as usize].as_ref().expect("has_db");
        let rows: Vec<Box<[Value]>> = db
            .select(&desc)
            .into_iter()
            .map(|s| db.row(s).into())
            .collect();
        for row in rows {
            if let Some(t) = self.token(ad, &row) {
                let m = merge(token, &t);
                self.add(conn.node, m, gen_node, conn.entry);
            }
        }
    }

    /// NOT-AND, token arriving (`CReteNAnd::Add`). From the left: passes if the right side
    /// does not hold. From the right (a fact appeared): retracts downstream.
    // bg3: CReteNAnd::Add (4.1.1.7631656)
    fn nand_add(&mut self, node: u32, mut token: Token, _from: u32, entry: u32) {
        if entry != ENTRY_LEFT {
            self.child_del(node, token);
            return;
        }
        let Some(PKind::Join(j)) = self.node(node).map(|n| &n.kind) else {
            return;
        };
        if self.valid(j.right_parent, &mut token, j.right_adapter) {
            return;
        }
        self.child_add(node, token);
    }

    /// NOT-AND, token retracted (`CReteNAnd::Del`). From the right (a fact disappeared): if
    /// the right side no longer holds, the matching left tokens are joined and passed on
    /// again, so the rule fires.
    // bg3: CReteNAnd::Del (4.1.1.7631656)
    fn nand_del(&mut self, node: u32, mut token: Token, from: u32, entry: u32) {
        if entry == ENTRY_LEFT {
            self.child_del(node, token);
            return;
        }
        let Some(PKind::Join(j)) = self.node(node).map(|n| &n.kind) else {
            return;
        };
        if self.valid(j.right_parent, &mut token, j.right_adapter) {
            return;
        }
        self.join(node, token, from, entry);
    }

    /// The comparison of a compare node on a token.
    // bg3: CReteRelCondition::ConditionValid (4.1.1.7631656)
    fn condition(&mut self, node: u32, token: &Token) -> bool {
        let Some(PKind::Compare(c)) = self.node(node).map(|n| &n.kind) else {
            return false;
        };
        let mut left = c.left_value;
        let mut right = c.right_value;
        for &(var, v) in token {
            if var == c.left_var {
                left = v;
            } else if var == c.right_var {
                right = v;
            }
        }
        match osi_compare(self.strings, &left, &right, c.op) {
            Ok(r) => r,
            Err(()) => {
                let msg = format!(
                    "node {node}: invalid compare {} {} {}",
                    display(self.strings, &left),
                    c.op.symbol(),
                    display(self.strings, &right)
                );
                self.err(msg);
                false
            }
        }
    }

    // bg3: CReteRelCondition::Add (4.1.1.7631656)
    fn rel_add(&mut self, node: u32, token: Token) {
        if self.condition(node, &token) {
            self.child_add(node, token);
        }
    }

    /// Rule terminal: bind the rule variables, count a QRY rule, fire the actions in order.
    // bg3: CReteRuleActionPart::Add (4.1.1.7631656)
    fn rule_add(&mut self, node: u32, token: Token) {
        let Some(PKind::Rule(r)) = self.node(node).map(|n| &n.kind) else {
            return;
        };
        let vars = &mut self.s.rule_vars[node as usize];
        let mut out_of_range = false;
        for (var, v) in &token {
            match vars.get_mut(*var as usize) {
                Some(slot) => *slot = assign(v),
                None => out_of_range = true,
            }
        }
        if out_of_range {
            self.err(format!("rule {node}: token variable out of range"));
        }
        if self.s.rule_nesting >= self.s.max_rule_nesting {
            self.err(format!(
                "rule {node} (line {}): nesting limit {} reached, actions skipped",
                r.line, self.s.max_rule_nesting
            ));
            return;
        }
        if r.is_query {
            self.s.user_queries_executed += 1;
        }
        self.s.stats.rules_fired += 1;
        let depth = self.s.rule_nesting;
        self.emit(|x| Record::RuleFires {
            depth,
            line: r.line,
            vars: x.lvs(&x.s.rule_vars[node as usize]),
        });
        self.s.rule_nesting += 1;
        self.s.stats.max_rule_nesting = self.s.stats.max_rule_nesting.max(self.s.rule_nesting);
        for a in &r.actions {
            self.fire(a, Some(node));
        }
        self.s.rule_nesting -= 1;
        self.emit(|_| Record::RuleEnd { depth });
    }

    // ---- queries ----

    /// Arguments for a query node from a token (`CReteAdaptor::pParameterList`): bound
    /// variables and constants are inputs; declared out-parameters are passed unset.
    /// Returns `(value, is_out)` per parameter.
    // bg3: CReteAdaptor::pParameterList (4.1.1.7631656)
    fn param_list(
        &self,
        ad: &PAdapter,
        params: &[TypeId],
        out: &[bool],
        token: &Token,
    ) -> Vec<(Value, bool)> {
        params
            .iter()
            .enumerate()
            .map(|(i, &ty)| {
                let is_out = out.get(i).copied().unwrap_or(false);
                let logical = ad.logical.get(i).copied().unwrap_or(-1);
                let v = if logical != -1 {
                    match token.iter().find(|(var, _)| *var == logical as u8) {
                        Some((_, v)) if v.is_valid() => *v,
                        // Typed with the declared type's *root* (the log prints
                        // `[out] (GUIDSTRING)` for a `TAG` out-parameter).
                        _ => {
                            let root = self.p.types.root(ty);
                            Value::undef(root.type_id(), root)
                        }
                    }
                } else {
                    match ad.constant_at(i) {
                        Some(c) => *c,
                        None => Value::UNDEF,
                    }
                };
                (v, is_out)
            })
            .collect()
    }

    /// `CReteQuery::Valid`: run the query and bind its out-parameters into the token.
    // bg3: CReteQuery::Valid, CReteAdaptor::UpdateToken (4.1.1.7631656)
    fn query_valid(&mut self, n: &PNode, token: &mut Token, ad: Option<&PAdapter>) -> bool {
        let (Some(f), Some(ad)) = (n.func, ad) else {
            self.err("query node without function or adaptor");
            return false;
        };
        let pf = &self.p.functions[f];
        let params = self.param_list(ad, &pf.params, &pf.out, token);
        let mut args: Vec<Value> = params
            .iter()
            .zip(&pf.params)
            .map(|(&(v, out), &decl)| {
                if out {
                    let root = self.p.types.root(decl);
                    Value::undef(root.type_id(), root)
                } else {
                    v
                }
            })
            .collect();
        self.emit(|x| Record::QueryExec {
            kind: match n.kind {
                PKind::DivQuery => "DIV query",
                PKind::UserQuery => "Osi user query",
                _ => "internal query",
            }
            .to_owned(),
            name: pf.name.clone(),
            args: params
                .iter()
                .map(|(v, out)| QueryArg {
                    out: *out,
                    value: x.lv(v),
                })
                .collect(),
        });
        let ok = match n.kind {
            PKind::DivQuery => self.div_query(f, &mut args),
            PKind::InternalQuery => self.internal_query(pf.key0, &mut args),
            PKind::UserQuery => self.user_query(f, &args),
            _ => false,
        };
        if !ok {
            self.emit(|_| Record::QueryFailed);
        }
        if !args.is_empty() {
            // Printed from the engine argument list, which carries root types only.
            self.emit(|x| Record::QueryReturns {
                name: pf.name.clone(),
                args: args
                    .iter()
                    .map(|a| {
                        if !a.is_valid() {
                            // Printed as `(undef value)`, whatever the type.
                            return LogValue::unset();
                        }
                        let enum_ty = x.p.enums.contains_key(&a.ty);
                        let ty = if enum_ty { a.ty } else { a.root.type_id() };
                        x.lv(&Value { ty, ..*a })
                    })
                    .collect(),
            });
        }
        if !ok {
            return false;
        }
        // An out-parameter that was already bound must come back unchanged.
        let mut ok = true;
        for (i, &(v, out)) in params.iter().enumerate() {
            if out && v.is_valid() {
                let r = args[i];
                ok &= self.eq(&v, &r);
            }
        }
        if !ok {
            return false;
        }
        for (col, r) in args.iter().enumerate() {
            let logical = ad.logical.get(col).copied().unwrap_or(-1);
            if logical == -1 {
                if let Some(c) = ad.constant_at(col).copied()
                    && !self.eq(r, &c)
                {
                    return false;
                }
                continue;
            }
            let Some(pos) = token.iter().position(|(var, _)| *var == logical as u8) else {
                self.err(format!("query {}: unknown variable {logical}", pf.name));
                continue;
            };
            if token[pos].1.ty == 0 {
                token[pos].1 = assign(r);
            } else {
                let cur = token[pos].1;
                if !self.eq(&cur, r) {
                    return false;
                }
            }
        }
        true
    }

    /// Engine query (`CReteDIVQuery::_ExecuteQuery`). On failure the out-parameters are
    /// cleared.
    // bg3: CReteDIVQuery::_ExecuteQuery (4.1.1.7631656)
    fn div_query(&mut self, f: usize, args: &mut [Value]) -> bool {
        let pf = &self.p.functions[f];
        self.s.stats.engine_queries += 1;
        let mut ctx = HostCtx {
            strings: self.strings,
            types: &self.p.types,
            prog: self.p,
        };
        let ok = self.host.query(&mut ctx, &pf.name, args, &pf.out);
        for (i, a) in args.iter_mut().enumerate() {
            if pf.out.get(i).copied().unwrap_or(false) {
                *a = if ok {
                    coerce(self.strings, self.p, *a, pf.params[i])
                } else {
                    Value::UNDEF
                };
            }
        }
        ok
    }

    /// User query (`CReteOsiQuery::_ExecuteQuery`): add the arguments to the start node of
    /// `<name>__DEF__`; succeeds iff at least one of its QRY rules fired. The counter is
    /// restored afterwards, so nested queries do not count for the caller.
    // bg3: CReteOsiQuery::_ExecuteQuery (4.1.1.7631656)
    fn user_query(&mut self, f: usize, args: &[Value]) -> bool {
        let pf = &self.p.functions[f];
        self.s.stats.user_queries += 1;
        let def_name = format!("{}__DEF__", pf.name);
        let Some(def) = self.p.function(&def_name, args.len()) else {
            return false;
        };
        let def = &self.p.functions[def];
        if def.kind != FunctionKind::UserQuery {
            self.err(format!("{def_name}: type should be user query"));
            return false;
        }
        // The arguments travel as an engine argument list (`COsiArgumentDesc`), which only
        // carries root types: the QRY rules see `GUIDSTRING`, not `TAG` (runtime log).
        let args: Vec<Value> = args
            .iter()
            .map(|a| {
                if self.p.enums.contains_key(&a.ty) {
                    *a
                } else {
                    Value {
                        ty: a.root.type_id(),
                        ..*a
                    }
                }
            })
            .collect();
        let before = self.s.user_queries_executed;
        self.start_add(def.node, args);
        let after = self.s.user_queries_executed;
        self.s.user_queries_executed = before;
        after > before
    }

    fn goal_arg(&self, args: &[Value]) -> Option<u32> {
        let a = args.first()?;
        if a.ty != types::STRING {
            return None;
        }
        let name = self.strings.text(a.as_str().unwrap_or_default());
        self.p.goal_by_name(name)
    }

    fn db_of_function(&self, name_arg: Option<&Value>, arity_arg: Option<&Value>) -> Option<u32> {
        let (n, a) = (name_arg?, arity_arg?);
        if n.ty != types::STRING || a.ty != types::INTEGER {
            return None;
        }
        let name = self.strings.text(n.as_str()?);
        let f = self.p.function(name, a.as_int()? as usize)?;
        let node = self.p.node(self.p.functions[f].node)?;
        self.has_db(node).then_some(node.db)
    }

    /// Osiris built-in queries, by `CFourKey` id.
    // bg3: InternalQuery(unsigned int, COsiArgumentDesc*) (4.1.1.7631656)
    fn internal_query(&mut self, id: u32, args: &mut [Value]) -> bool {
        self.s.stats.internal_queries += 1;
        match id {
            100..=107 => {
                let Some(g) = self.goal_arg(args) else {
                    return false;
                };
                let f = self.s.goal_flags[g as usize];
                match id {
                    // SysStatus(goal, out state): 1 sleeping, 2 active, 3 completed.
                    100 => {
                        let Some(out) = args.get_mut(1) else {
                            return false;
                        };
                        if out.ty != types::INTEGER {
                            return false;
                        }
                        let state = if f & 1 != 0 {
                            3
                        } else if f & 2 != 0 {
                            1
                        } else {
                            2
                        };
                        *out = Value::int(state);
                        true
                    }
                    101 => f & 1 != 0,                     // SysIsCompleted
                    102 => f & 2 == 0 && f & 1 == 0,       // SysIsActive
                    103 => f & 2 != 0 && f & 1 == 0,       // SysIsSleeping
                    104 => f & 0x40 != 0,                  // SysWasCompleted
                    105 => f & 0x80 == 0 && f & 0x40 == 0, // SysWasActive
                    106 => f & 0x80 != 0 && f & 0x40 == 0, // SysWasSleeping
                    _ => f & 0x20 != 0,                    // 107 SysWasDefined
                }
            }
            // SysCount(db name, arity, out count)
            108 => {
                let Some(db) = self.db_of_function(args.first(), args.get(1)) else {
                    return false;
                };
                if args.get(2).is_none_or(|a| a.ty != types::INTEGER) {
                    return false;
                }
                let n = self.s.dbs[db as usize].as_ref().map_or(0, Db::len);
                args[2] = Value::int(n as i32);
                true
            }
            // SysStoryVersion(out major, minor, build, revision): bytes of the packed version.
            109 => {
                let mut parts = self
                    .p
                    .story_version
                    .split('.')
                    .map(|s| s.parse::<i32>().unwrap_or(0));
                let mut v = [0i32; 4];
                for slot in &mut v {
                    *slot = parts.next().unwrap_or(0);
                }
                let packed = (v[0] << 24)
                    .wrapping_add(v[1] << 16)
                    .wrapping_add(v[2] << 8)
                    .wrapping_add(v[3]) as u32;
                for (i, a) in args.iter_mut().take(4).enumerate() {
                    *a = Value::int(((packed >> (24 - 8 * i)) & 0xff) as i32);
                }
                true
            }
            // SysFactAtIndex(db name, arity, 1-based index, out db name): replace the contents
            // of the output database with the n-th row of the input one. No propagation.
            110 => {
                let Some(src) = self.db_of_function(args.first(), args.get(1)) else {
                    return false;
                };
                let Some(index) = args.get(2).and_then(Value::as_int) else {
                    return false;
                };
                let Some(dst) = self.db_of_function(args.get(3), args.get(1)) else {
                    self.err("SysFactAtIndex: output database does not exist");
                    return false;
                };
                let src_db = self.s.dbs[src as usize].as_ref().expect("has_db");
                if index < 1 || index as usize > src_db.len() {
                    return false;
                }
                let row = src_db.nth(index as usize - 1).expect("in range").to_vec();
                let out = self.db_mut(dst);
                out.clear();
                out.insert(&row);
                true
            }
            _ => false,
        }
    }

    /// Osiris built-in calls, by `CFourKey` id.
    // bg3: InternalCall(unsigned int, COsipParameterList*) (4.1.1.7631656)
    fn internal_call(&mut self, id: u32, args: &[Value]) {
        match id {
            1..=3 => {
                let Some(g) = self.goal_arg(args) else {
                    return;
                };
                match id {
                    1 => self.set_completed(g), // SysCompleteGoal
                    2 => self.enable_rules(g),  // SysActivateGoal
                    _ => {
                        // SysSetGoalSleeping (COsiGoal::SetSleeping)
                        let f = &mut self.s.goal_flags[g as usize];
                        *f = (*f & 0xf8) | 2;
                    }
                }
            }
            4 => {} // SysLog: logging only
            5 => {
                // SysClear(db name, arity): empty the database, no propagation.
                if let Some(db) = self.db_of_function(args.first(), args.get(1)) {
                    self.db_mut(db).clear();
                }
            }
            _ => self.err(format!("unknown internal call {id}")),
        }
    }

    // ---- actions ----

    fn param_value(&mut self, p: &Param, rule: Option<u32>) -> Value {
        match p {
            Param::Const(v) => *v,
            Param::Var { index, .. } => match rule
                .and_then(|r| self.s.rule_vars.get(r as usize))
                .and_then(|vars| vars.get(*index as usize))
            {
                Some(v) => *v,
                None => {
                    self.err(format!("variable {index} outside a rule"));
                    Value::UNDEF
                }
            },
        }
    }

    fn saved_shared(&self, a: &PAction, rule: Option<u32>) -> Vec<Value> {
        let Some(vars) = rule.and_then(|r| self.s.rule_vars.get(r as usize)) else {
            return Vec::new();
        };
        a.params
            .iter()
            .filter_map(|p| match p.param {
                Param::Var { index, save: true } => vars.get(index as usize).copied(),
                _ => None,
            })
            .collect()
    }

    fn restore_shared(&mut self, a: &PAction, rule: Option<u32>, saved: Vec<Value>) {
        let Some(vars) = rule.and_then(|r| self.s.rule_vars.get_mut(r as usize)) else {
            return;
        };
        let mut saved = saved.into_iter();
        for p in &a.params {
            if let Param::Var { index, save: true } = p.param {
                let Some(v) = saved.next() else {
                    return;
                };
                if let Some(slot) = vars.get_mut(index as usize) {
                    // COsiTypedValueBase::SetValue: payload only, the variable keeps its type.
                    slot.data = v.data;
                }
            }
        }
    }

    /// Fire one action (`COsiFunctionParseData::Fire`).
    // bg3: COsiFunctionParseData::Fire (4.1.1.7631656)
    fn fire(&mut self, a: &'a PAction, rule: Option<u32>) {
        self.s.stats.actions_fired += 1;
        let depth = self.s.rule_nesting;
        let f = match a.kind {
            ActionKind::GoalCompleted(g) => {
                self.emit(|x| Record::GoalCompleted {
                    depth,
                    goal: x.p.goal(g).map_or_else(String::new, |g| g.name.clone()),
                    id: g,
                });
                if self.p.goal(g).is_some() {
                    self.set_completed(g);
                } else {
                    self.err(format!("GoalCompleted: goal {g} not in use"));
                }
                return;
            }
            ActionKind::DebugHook => return,
            ActionKind::Function { func: None } => {
                self.err(format!(
                    "{}/{}: function is not in use",
                    a.name,
                    a.params.len()
                ));
                return;
            }
            ActionKind::Function { func: Some(f) } => f,
        };
        let pf = &self.p.functions[f];
        let values: Vec<Value> = a
            .params
            .iter()
            .map(|p| self.param_value(&p.param, rule))
            .collect();
        self.emit(|x| Record::Action {
            depth,
            kind: match pf.kind {
                FunctionKind::Database if a.negate => LogKind::DeleteFact,
                FunctionKind::Database => LogKind::AddFact,
                FunctionKind::Proc => LogKind::Proc,
                FunctionKind::SysCall => LogKind::InternalCall,
                _ => LogKind::DivCall,
            },
            // The name as written in the action (functions are matched case-insensitively).
            name: a.name.clone(),
            args: x.lvs(&values),
        });
        match pf.kind {
            FunctionKind::Call => {
                // Out-parameters go to the engine as typed empty arguments.
                let args: Vec<Value> = values
                    .iter()
                    .zip(&a.params)
                    .map(|(v, p)| {
                        if p.out {
                            Value::undef(v.ty, v.root)
                        } else {
                            *v
                        }
                    })
                    .collect();
                self.s.stats.calls += 1;
                let mut ctx = HostCtx {
                    strings: self.strings,
                    types: &self.p.types,
                    prog: self.p,
                };
                if !self.host.call(&mut ctx, &pf.name, &args) {
                    self.emit(|_| Record::CallFailed);
                }
            }
            FunctionKind::Database if a.negate => {
                self.start_del(pf.node, values);
            }
            FunctionKind::Database | FunctionKind::Proc => {
                // The nested evaluation may re-enter this rule and overwrite its variables;
                // the ones this action passes are saved and restored around it.
                let saved = self.saved_shared(a, rule);
                self.start_add(pf.node, values);
                self.restore_shared(a, rule, saved);
            }
            FunctionKind::SysCall => self.internal_call(pf.key0, &values),
            other => self.err(format!(
                "{}: a {} cannot be an action",
                pf.name,
                other.keyword()
            )),
        }
    }

    // ---- goals ----

    /// `GoalCompleted`: disable the goal's rules, run EXIT, mark it completed, then let each
    /// subgoal check whether its parents allow it to start.
    // bg3: COsiGoal::SetCompleted (4.1.1.7631656)
    fn set_completed(&mut self, g: u32) {
        let Some(goal) = self.p.goal(g) else {
            return;
        };
        if self.s.goal_flags[g as usize] & 1 != 0 {
            return;
        }
        self.s.goal_flags[g as usize] |= 6;
        self.emit(|_| Record::GoalSection {
            exit: true,
            goal: goal.name.clone(),
            id: g,
            woken: true,
        });
        for a in &goal.exit {
            self.fire(a, None);
        }
        self.s.goal_flags[g as usize] |= 1;
        for &sub in &goal.subgoals {
            if self.p.goal(sub).is_some() {
                self.check_enable_rules(sub);
            } else {
                self.err(format!("subgoal {sub} not in use"));
            }
        }
        // The log's END line comes after the subgoals have run their INIT sections
        // (observed in a runtime log of the game).
        self.emit(|_| Record::GoalSectionEnd {
            exit: true,
            goal: goal.name.clone(),
            id: g,
        });
    }

    // bg3: COsiGoal::CheckEnableRules (4.1.1.7631656)
    fn check_enable_rules(&mut self, g: u32) {
        let Some(goal) = self.p.goal(g) else {
            return;
        };
        if self.s.goal_flags[g as usize] & 4 != 0 {
            return;
        }
        if !goal.parents.is_empty() {
            let done = |p: &u32| {
                self.s
                    .goal_flags
                    .get(*p as usize)
                    .is_some_and(|f| f & 1 != 0)
            };
            match goal.combiner {
                SubGoalCombiner::And => {
                    if !goal.parents.iter().all(done) {
                        return;
                    }
                }
                SubGoalCombiner::Or => {
                    if !goal.parents.iter().any(done) {
                        return;
                    }
                }
                SubGoalCombiner::Unknown(c) => self.err(format!("goal {g}: unknown combiner {c}")),
            }
        }
        self.enable_rules(g);
    }

    /// Wake a sleeping goal: clear the completed/sleeping bits and run INIT.
    // bg3: COsiGoal::EnableRules (4.1.1.7631656)
    fn enable_rules(&mut self, g: u32) {
        let Some(goal) = self.p.goal(g) else {
            return;
        };
        if self.s.goal_flags[g as usize] & 2 == 0 {
            return;
        }
        self.s.goal_flags[g as usize] &= !3;
        self.emit(|_| Record::GoalSection {
            exit: false,
            goal: goal.name.clone(),
            id: g,
            woken: true,
        });
        for a in &goal.init {
            self.fire(a, None);
        }
        self.emit(|_| Record::GoalSectionEnd {
            exit: false,
            goal: goal.name.clone(),
            id: g,
        });
    }
}
