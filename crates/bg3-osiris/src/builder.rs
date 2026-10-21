//! Author small synthetic stories (for tests and experiments).
//!
//! A tiny rule compiler: rules are written as condition chains and compiled into the same
//! network shapes the Osiris compiler emits in shipped stories (checked against
//! `Mods/GustavDev/Story/story.div.osi`): a left-deep chain of joins whose left edges carry an
//! identity adaptor over the bound variables (variable *i* ↔ column *i*), right edges that map
//! the condition's columns to variables/constants, compare nodes inline, generator links
//! pointing at the closest database (indirection +1 per level), and rule variables / action
//! parameters flagged as shared, grounded variables.
//!
//! ```
//! use bg3_osiris::builder::*;
//! let mut b = StoryBuilder::new();
//! b.event("Ping", &[INT]);
//! b.db("DB_Seen", &[INT]);
//! let g = b.goal("Main", &[]);
//! b.rule(g, Rule::when("Ping", [v("X")]).then("DB_Seen", [v("X")]));
//! let story = b.build();
//! ```

use std::collections::HashMap;

use bg3_story::{
    Action, Adapter, AdapterId, BuiltinType, Connection, Constant, ConstantData, Database,
    DatabaseId, DbLink, EntryPoint, Function, FunctionKind, Goal, GoalId, Header, JoinNode, Node,
    NodeId, NodeKind, RelOp, RuleNode, Signature, StartNode, Story, SubGoalCombiner, TypeId,
    TypeInfo, UnaryNode, Value, value_flags as vf,
};

pub const INT: TypeId = 1;
pub const INT64: TypeId = 2;
pub const REAL: TypeId = 3;
pub const STRING: TypeId = 4;
pub const GUIDSTRING: TypeId = 5;

/// A literal.
#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Int(i32),
    Int64(i64),
    Real(f32),
    Str(String),
    Guid(String),
}

impl Lit {
    fn default_type(&self) -> TypeId {
        match self {
            Lit::Int(_) => INT,
            Lit::Int64(_) => INT64,
            Lit::Real(_) => REAL,
            Lit::Str(_) => STRING,
            Lit::Guid(_) => GUIDSTRING,
        }
    }
}

/// A condition/action argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Var(String),
    /// `_`
    Any,
    Lit(Lit, Option<TypeId>),
}

pub fn v(name: &str) -> Term {
    Term::Var(name.to_owned())
}
pub fn any() -> Term {
    Term::Any
}
pub fn int(i: i32) -> Term {
    Term::Lit(Lit::Int(i), None)
}
pub fn int64(i: i64) -> Term {
    Term::Lit(Lit::Int64(i), None)
}
pub fn real(f: f32) -> Term {
    Term::Lit(Lit::Real(f), None)
}
pub fn s(text: &str) -> Term {
    Term::Lit(Lit::Str(text.to_owned()), None)
}
pub fn guid(text: &str) -> Term {
    Term::Lit(Lit::Guid(text.to_owned()), None)
}
/// A literal of an alias type, e.g. `(CHARACTER)S_Foo_<uuid>`.
pub fn typed(ty: TypeId, lit: Lit) -> Term {
    Term::Lit(lit, Some(ty))
}

#[derive(Clone, Debug)]
enum Cond {
    Fact {
        name: String,
        args: Vec<Term>,
        negate: bool,
    },
    Cmp(Term, RelOp, Term),
}

/// An action (rule THEN part, goal INIT/EXIT).
#[derive(Clone, Debug)]
pub struct Act {
    name: String,
    args: Vec<Term>,
    negate: bool,
    complete_goal: bool,
}

impl Act {
    pub fn call<const N: usize>(name: &str, args: [Term; N]) -> Self {
        Self {
            name: name.to_owned(),
            args: args.into(),
            negate: false,
            complete_goal: false,
        }
    }

    /// `NOT DB_Foo(...)`
    pub fn not<const N: usize>(name: &str, args: [Term; N]) -> Self {
        Self {
            negate: true,
            ..Self::call(name, args)
        }
    }

    /// `GoalCompleted;`
    pub fn complete() -> Self {
        Self {
            name: String::new(),
            args: Vec::new(),
            negate: false,
            complete_goal: true,
        }
    }
}

/// `IF <first> AND ... THEN ...`
#[derive(Clone, Debug)]
pub struct Rule {
    conds: Vec<Cond>,
    actions: Vec<Act>,
}

impl Rule {
    /// First condition: an event, PROC, database or (for [`StoryBuilder::query_rule`]) the
    /// user query being defined.
    pub fn when<const N: usize>(name: &str, args: [Term; N]) -> Self {
        Self {
            conds: vec![Cond::Fact {
                name: name.to_owned(),
                args: args.into(),
                negate: false,
            }],
            actions: Vec::new(),
        }
    }

    /// `AND DB_Foo(...)` / `AND Query(...)`.
    pub fn and<const N: usize>(mut self, name: &str, args: [Term; N]) -> Self {
        self.conds.push(Cond::Fact {
            name: name.to_owned(),
            args: args.into(),
            negate: false,
        });
        self
    }

    /// `AND NOT DB_Foo(...)`.
    pub fn and_not<const N: usize>(mut self, name: &str, args: [Term; N]) -> Self {
        self.conds.push(Cond::Fact {
            name: name.to_owned(),
            args: args.into(),
            negate: true,
        });
        self
    }

    /// `AND left OP right`.
    pub fn cmp(mut self, left: Term, op: RelOp, right: Term) -> Self {
        self.conds.push(Cond::Cmp(left, op, right));
        self
    }

    pub fn then<const N: usize>(mut self, name: &str, args: [Term; N]) -> Self {
        self.actions.push(Act::call(name, args));
        self
    }

    pub fn then_not<const N: usize>(mut self, name: &str, args: [Term; N]) -> Self {
        self.actions.push(Act::not(name, args));
        self
    }

    pub fn then_complete(mut self) -> Self {
        self.actions.push(Act::complete());
        self
    }
}

/// Goal handle (raw goal id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct G(pub u32);

struct GoalDef {
    name: String,
    parents: Vec<u32>,
    combiner: Option<SubGoalCombiner>,
    init: Vec<Act>,
    exit: Vec<Act>,
}

/// Built-in syscall/sysquery signatures and their ids (`CFourKey` word 0).
/// `(name, kind, parameter types, out-parameters, id)`
type SysSignature = (
    &'static str,
    FunctionKind,
    &'static [TypeId],
    &'static [bool],
    u32,
);

const SYS: &[SysSignature] = &[
    ("SysCompleteGoal", FunctionKind::SysCall, &[STRING], &[], 1),
    ("SysActivateGoal", FunctionKind::SysCall, &[STRING], &[], 2),
    (
        "SysSetGoalSleeping",
        FunctionKind::SysCall,
        &[STRING],
        &[],
        3,
    ),
    ("SysLog", FunctionKind::SysCall, &[STRING, INT], &[], 4),
    ("SysClear", FunctionKind::SysCall, &[STRING, INT], &[], 5),
    (
        "SysStatus",
        FunctionKind::SysQuery,
        &[STRING, INT],
        &[false, true],
        100,
    ),
    (
        "SysIsCompleted",
        FunctionKind::SysQuery,
        &[STRING],
        &[],
        101,
    ),
    ("SysIsActive", FunctionKind::SysQuery, &[STRING], &[], 102),
    ("SysIsSleeping", FunctionKind::SysQuery, &[STRING], &[], 103),
    (
        "SysWasCompleted",
        FunctionKind::SysQuery,
        &[STRING],
        &[],
        104,
    ),
    ("SysWasActive", FunctionKind::SysQuery, &[STRING], &[], 105),
    (
        "SysWasSleeping",
        FunctionKind::SysQuery,
        &[STRING],
        &[],
        106,
    ),
    ("SysWasDefined", FunctionKind::SysQuery, &[STRING], &[], 107),
    (
        "SysCount",
        FunctionKind::SysQuery,
        &[STRING, INT, INT],
        &[false, false, true],
        108,
    ),
    (
        "SysStoryVersion",
        FunctionKind::SysQuery,
        &[INT, INT, INT, INT],
        &[true, true, true, true],
        109,
    ),
    (
        "SysFactAtIndex",
        FunctionKind::SysQuery,
        &[STRING, INT, INT, STRING],
        &[],
        110,
    ),
];

/// Where the chain currently ends.
#[derive(Clone, Copy)]
struct Prev {
    node: u32,
    /// Adaptor on the edge into the next node when `node` is a start node.
    start_adapter: Option<u32>,
}

pub struct StoryBuilder {
    types: Vec<TypeInfo>,
    functions: Vec<Function>,
    function_index: HashMap<(String, u8), usize>,
    nodes: Vec<Node>,
    adapters: Vec<Adapter>,
    databases: Vec<Database>,
    goals: Vec<GoalDef>,
    pending_rules: Vec<(u32, Rule, bool)>,
}

impl Default for StoryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn conn(node: u32, entry: EntryPoint, goal: u32) -> Connection {
    Connection {
        node: NodeId::from_raw(node),
        entry,
        goal: GoalId::from_raw(goal),
    }
}

fn no_link(indirection: u8) -> DbLink {
    DbLink {
        node: None,
        connection: conn(0, EntryPoint::None, 0),
        indirection,
    }
}

impl StoryBuilder {
    pub fn new() -> Self {
        Self {
            types: [
                ("INTEGER", 1),
                ("INTEGER64", 2),
                ("REAL", 3),
                ("STRING", 4),
                ("GUIDSTRING", 5),
            ]
            .iter()
            .map(|&(n, i)| TypeInfo {
                name: n.to_owned(),
                index: i,
                alias: 0,
            })
            .collect(),
            functions: Vec::new(),
            function_index: HashMap::new(),
            nodes: Vec::new(),
            adapters: Vec::new(),
            databases: Vec::new(),
            goals: Vec::new(),
            pending_rules: Vec::new(),
        }
    }

    /// Declare an alias type (e.g. `CHARACTER` of GUIDSTRING).
    pub fn alias(&mut self, name: &str, of: TypeId) -> TypeId {
        let index = self.types.iter().map(|t| t.index).max().unwrap_or(5).max(5) + 1;
        self.types.push(TypeInfo {
            name: name.to_owned(),
            index,
            alias: of as u8,
        });
        index as TypeId
    }

    fn declare(
        &mut self,
        name: &str,
        kind: FunctionKind,
        params: &[TypeId],
        out: &[bool],
        key0: u32,
    ) -> usize {
        let key = (name.to_owned(), params.len() as u8);
        if let Some(&i) = self.function_index.get(&key) {
            return i;
        }
        let mut mask = vec![0u8; params.len().div_ceil(8)];
        for (i, &o) in out.iter().enumerate() {
            if o {
                mask[i >> 3] |= 0x80 >> (i & 7);
            }
        }
        self.functions.push(Function {
            line: 0,
            condition_refs: 0,
            action_refs: 0,
            node: None,
            kind,
            keys: [key0, 0, 0, 0],
            signature: Signature {
                name: name.to_owned(),
                out_param_mask: mask,
                params: params.to_vec(),
            },
        });
        let i = self.functions.len() - 1;
        self.function_index.insert(key, i);
        i
    }

    pub fn event(&mut self, name: &str, params: &[TypeId]) {
        self.declare(name, FunctionKind::Event, params, &[], 0);
    }

    pub fn proc(&mut self, name: &str, params: &[TypeId]) {
        self.declare(name, FunctionKind::Proc, params, &[], 0);
    }

    pub fn db(&mut self, name: &str, params: &[TypeId]) {
        self.declare(name, FunctionKind::Database, params, &[], 0);
    }

    /// Engine call.
    pub fn call(&mut self, name: &str, params: &[TypeId]) {
        self.declare(name, FunctionKind::Call, params, &[], 0);
    }

    /// Engine query; `out[i]` marks out-parameters.
    pub fn query(&mut self, name: &str, params: &[TypeId], out: &[bool]) {
        self.declare(name, FunctionKind::Query, params, out, 0);
    }

    /// User query `name` (and its `name__DEF__` companion).
    pub fn user_query(&mut self, name: &str, params: &[TypeId]) {
        self.declare(name, FunctionKind::UserQuery, params, &[], 0);
        self.declare(
            &format!("{name}__DEF__"),
            FunctionKind::UserQuery,
            params,
            &[],
            0,
        );
    }

    /// A goal; root goals have no parents. Roots start active (flags 0, OR), others asleep
    /// (flags 2, AND) — as in compiled stories.
    pub fn goal(&mut self, name: &str, parents: &[G]) -> G {
        self.goals.push(GoalDef {
            name: name.to_owned(),
            parents: parents.iter().map(|g| g.0).collect(),
            combiner: None,
            init: Vec::new(),
            exit: Vec::new(),
        });
        G(self.goals.len() as u32)
    }

    pub fn combiner(&mut self, g: G, c: SubGoalCombiner) {
        self.goals[g.0 as usize - 1].combiner = Some(c);
    }

    pub fn init(&mut self, g: G, actions: impl IntoIterator<Item = Act>) {
        self.goals[g.0 as usize - 1].init.extend(actions);
    }

    pub fn exit(&mut self, g: G, actions: impl IntoIterator<Item = Act>) {
        self.goals[g.0 as usize - 1].exit.extend(actions);
    }

    /// Add a rule to goal `g`. Rules are compiled in the order they are added.
    pub fn rule(&mut self, g: G, rule: Rule) {
        self.pending_rules.push((g.0, rule, false));
    }

    /// Add a `QRY` rule defining the user query named in the rule's first condition.
    pub fn query_rule(&mut self, g: G, rule: Rule) {
        self.pending_rules.push((g.0, rule, true));
    }

    fn function(&mut self, name: &str, arity: usize) -> usize {
        if let Some(&i) = self.function_index.get(&(name.to_owned(), arity as u8)) {
            return i;
        }
        if let Some(&(n, kind, params, out, key)) =
            SYS.iter().find(|s| s.0 == name && s.2.len() == arity)
        {
            return self.declare(n, kind, params, out, key);
        }
        panic!("builder: undeclared function {name}/{arity}");
    }

    fn new_node(&mut self, name: &str, arity: u8, database: Option<u32>, kind: NodeKind) -> u32 {
        let id = self.nodes.len() as u32 + 1;
        self.nodes.push(Node {
            id: NodeId::from_raw(id).expect("non-zero"),
            database: database.and_then(DatabaseId::from_raw),
            name: name.to_owned(),
            arity,
            kind,
        });
        id
    }

    fn node_mut(&mut self, id: u32) -> &mut Node {
        &mut self.nodes[id as usize - 1]
    }

    /// The start/query node of a function, created on first use.
    fn function_node(&mut self, f: usize) -> u32 {
        if let Some(n) = self.functions[f].node {
            return n.get();
        }
        let name = self.functions[f].signature.name.clone();
        let params = self.functions[f].signature.params.clone();
        let arity = params.len() as u8;
        let id = match self.functions[f].kind {
            FunctionKind::Database => {
                let db = self.databases.len() as u32 + 1;
                self.databases.push(Database {
                    id: DatabaseId::from_raw(db).expect("non-zero"),
                    param_types: params,
                    facts: Vec::new(),
                });
                self.new_node(
                    &name,
                    arity,
                    Some(db),
                    NodeKind::Database(StartNode::default()),
                )
            }
            FunctionKind::Event | FunctionKind::Proc => {
                self.new_node(&name, arity, None, NodeKind::Event(StartNode::default()))
            }
            FunctionKind::UserQuery if name.ends_with("__DEF__") => {
                self.new_node(&name, arity, None, NodeKind::Event(StartNode::default()))
            }
            FunctionKind::UserQuery => self.new_node(&name, arity, None, NodeKind::UserQuery),
            FunctionKind::Query => self.new_node(&name, arity, None, NodeKind::DivQuery),
            FunctionKind::SysQuery => self.new_node(&name, arity, None, NodeKind::InternalQuery),
            k => panic!("builder: {name} ({k:?}) has no node"),
        };
        self.functions[f].node = NodeId::from_raw(id);
        id
    }

    fn constant(&self, lit: &Lit, ty: Option<TypeId>) -> Constant {
        let ty = ty.unwrap_or_else(|| lit.default_type());
        let data = match lit {
            Lit::Int(i) => ConstantData::Integer(*i),
            Lit::Int64(i) => ConstantData::Integer64(*i),
            Lit::Real(f) => ConstantData::Real(*f),
            Lit::Str(s) => {
                let root = self.root(ty);
                if root == Some(BuiltinType::GuidString) {
                    ConstantData::GuidString(Some(s.clone()))
                } else {
                    ConstantData::String(Some(s.clone()))
                }
            }
            Lit::Guid(s) => ConstantData::GuidString(Some(s.clone())),
        };
        Constant { type_id: ty, data }
    }

    fn root(&self, mut ty: TypeId) -> Option<BuiltinType> {
        for _ in 0..16 {
            if let Some(b) = BuiltinType::from_id(ty) {
                return Some(b);
            }
            ty = self.types.iter().find(|t| t.index as TypeId == ty)?.alias as TypeId;
        }
        None
    }

    fn new_adapter(&mut self, logical: Vec<i8>, constants: Vec<Value>) -> u32 {
        let mut map: Vec<(u8, u8)> = Vec::new();
        for (col, &l) in logical.iter().enumerate() {
            if l >= 0 && !map.iter().any(|&(k, _)| k == l as u8) {
                map.push((l as u8, col as u8));
            }
        }
        let id = self.adapters.len() as u32 + 1;
        self.adapters.push(Adapter {
            id: AdapterId::from_raw(id).expect("non-zero"),
            constants,
            logical_indices: logical,
            logical_to_physical: map,
        });
        id
    }

    fn identity_adapter(&mut self, bound: usize) -> u32 {
        self.new_adapter((0..bound as i8).collect(), Vec::new())
    }

    /// Adaptor for a condition's columns; unbound variables get bound when `bind`.
    fn fact_adapter(&mut self, args: &[Term], vars: &mut Vec<String>, bind: bool) -> u32 {
        let mut logical = Vec::new();
        let mut constants = Vec::new();
        for (col, t) in args.iter().enumerate() {
            match t {
                Term::Var(name) => {
                    let idx = match vars.iter().position(|n| n == name) {
                        Some(i) => i,
                        None if bind => {
                            vars.push(name.clone());
                            vars.len() - 1
                        }
                        None => panic!("builder: variable {name} unbound in a negated condition"),
                    };
                    logical.push(idx as i8);
                }
                Term::Any => logical.push(-1),
                Term::Lit(lit, ty) => {
                    logical.push(-1);
                    constants.push(Value {
                        index: col as i8,
                        flags: vf::SPEC_VALUE | vf::VALID,
                        constant: Some(self.constant(lit, *ty)),
                    });
                }
            }
        }
        self.new_adapter(logical, constants)
    }

    /// Generator link a child inherits from `prev`.
    fn link_from(&self, prev: Prev, child: u32, entry: EntryPoint, goal: u32) -> DbLink {
        let node = &self.nodes[prev.node as usize - 1];
        match &node.kind {
            NodeKind::Database(_) => DbLink {
                node: NodeId::from_raw(prev.node),
                connection: conn(child, entry, goal),
                indirection: 1,
            },
            NodeKind::And(j) | NodeKind::NotAnd(j) => {
                let g = if j.left_db.indirection <= j.right_db.indirection {
                    j.left_db
                } else {
                    j.right_db
                };
                if g.node.is_none() {
                    no_link(0)
                } else {
                    DbLink {
                        indirection: g.indirection.saturating_add(1),
                        ..g
                    }
                }
            }
            NodeKind::Compare(c) => {
                if c.unary.db.node.is_none() {
                    no_link(0)
                } else {
                    DbLink {
                        indirection: c.unary.db.indirection.saturating_add(1),
                        ..c.unary.db
                    }
                }
            }
            _ => no_link(0),
        }
    }

    /// Hook `child` (entering at `entry`) under `prev`; returns the adaptor of that edge.
    fn attach(
        &mut self,
        prev: Prev,
        child: u32,
        entry: EntryPoint,
        goal: u32,
        bound: usize,
    ) -> u32 {
        let c = conn(child, entry, goal);
        match &mut self.node_mut(prev.node).kind {
            NodeKind::Database(s) | NodeKind::Event(s) => s.successors.push(c),
            NodeKind::And(j) | NodeKind::NotAnd(j) => j.next = c,
            NodeKind::Compare(cmp) => cmp.unary.next = c,
            _ => panic!("builder: cannot attach below node {}", prev.node),
        }
        match prev.start_adapter {
            Some(a) => a,
            None => self.identity_adapter(bound),
        }
    }

    fn compile_rule(&mut self, goal: u32, rule: &Rule, is_query: bool) {
        let mut vars: Vec<String> = Vec::new();
        let Cond::Fact { name, args, .. } = &rule.conds[0] else {
            panic!("builder: a rule must start with a fact");
        };
        let first = if is_query {
            format!("{name}__DEF__")
        } else {
            name.clone()
        };
        let f = self.function(&first, args.len());
        let start = self.function_node(f);
        let a0 = self.fact_adapter(args, &mut vars, true);
        let mut prev = Prev {
            node: start,
            start_adapter: Some(a0),
        };

        for cond in &rule.conds[1..] {
            match cond {
                Cond::Fact { name, args, negate } => {
                    let f = self.function(name, args.len());
                    let kind = self.functions[f].kind;
                    let right = self.function_node(f);
                    let placeholder = JoinNode {
                        next: conn(0, EntryPoint::None, 0),
                        left_parent: NodeId::from_raw(prev.node),
                        right_parent: NodeId::from_raw(right),
                        left_adapter: None,
                        right_adapter: None,
                        left_db: no_link(0),
                        right_db: no_link(255),
                    };
                    let join = self.new_node(
                        "",
                        0,
                        None,
                        if *negate {
                            NodeKind::NotAnd(placeholder)
                        } else {
                            NodeKind::And(placeholder)
                        },
                    );
                    let bound = vars.len();
                    let left_db = self.link_from(prev, join, EntryPoint::Left, goal);
                    let left_adapter = self.attach(prev, join, EntryPoint::Left, goal, bound);
                    let right_adapter = self.fact_adapter(args, &mut vars, !negate);
                    let right_db = if kind == FunctionKind::Database {
                        if let NodeKind::Database(s) = &mut self.node_mut(right).kind {
                            s.successors.push(conn(join, EntryPoint::Right, goal));
                        }
                        DbLink {
                            node: NodeId::from_raw(right),
                            connection: conn(join, EntryPoint::Right, goal),
                            indirection: 1,
                        }
                    } else {
                        no_link(255)
                    };
                    if let NodeKind::And(j) | NodeKind::NotAnd(j) = &mut self.node_mut(join).kind {
                        j.left_adapter = AdapterId::from_raw(left_adapter);
                        j.right_adapter = AdapterId::from_raw(right_adapter);
                        j.left_db = left_db;
                        j.right_db = right_db;
                    }
                    prev = Prev {
                        node: join,
                        start_adapter: None,
                    };
                }
                Cond::Cmp(l, op, r) => {
                    let side = |b: &Self, t: &Term| -> (i8, Value) {
                        match t {
                            Term::Var(name) => {
                                let idx =
                                    vars.iter().position(|n| n == name).unwrap_or_else(|| {
                                        panic!("builder: unbound {name} in compare")
                                    });
                                (
                                    idx as i8,
                                    Value {
                                        index: idx as i8,
                                        flags: vf::SPEC_VARIABLE,
                                        constant: None,
                                    },
                                )
                            }
                            Term::Lit(lit, ty) => (
                                -1,
                                Value {
                                    index: -1,
                                    flags: vf::SPEC_VALUE | vf::VALID,
                                    constant: Some(b.constant(lit, *ty)),
                                },
                            ),
                            Term::Any => panic!("builder: _ in compare"),
                        }
                    };
                    let (left_var, left_value) = side(self, l);
                    let (right_var, right_value) = side(self, r);
                    let unary = UnaryNode {
                        next: conn(0, EntryPoint::None, 0),
                        parent: NodeId::from_raw(prev.node),
                        adapter: None,
                        db: no_link(0),
                    };
                    let cmp = self.new_node(
                        "",
                        0,
                        None,
                        NodeKind::Compare(bg3_story::CompareNode {
                            unary,
                            left_var,
                            right_var,
                            left_value,
                            right_value,
                            op: *op,
                        }),
                    );
                    let link = self.link_from(prev, cmp, EntryPoint::None, goal);
                    let ad = self.attach(prev, cmp, EntryPoint::None, goal, vars.len());
                    if let NodeKind::Compare(c) = &mut self.node_mut(cmp).kind {
                        c.unary.adapter = AdapterId::from_raw(ad);
                        c.unary.db = link;
                    }
                    prev = Prev {
                        node: cmp,
                        start_adapter: None,
                    };
                }
            }
        }

        let variables: Vec<Value> = (0..vars.len())
            .map(|i| Value {
                index: i as i8,
                flags: vf::SPEC_VARIABLE | vf::IS_TYPE | vf::ADAPTED,
                constant: None,
            })
            .collect();
        let actions = rule
            .actions
            .iter()
            .map(|a| self.action(a, goal, Some(&vars)))
            .collect();
        let rule_node = self.new_node(
            "",
            0,
            None,
            NodeKind::Rule(RuleNode {
                unary: UnaryNode {
                    next: conn(0, EntryPoint::None, 0),
                    parent: NodeId::from_raw(prev.node),
                    adapter: None,
                    db: no_link(0),
                },
                actions,
                variables,
                line: 0,
                is_query,
            }),
        );
        let link = self.link_from(prev, rule_node, EntryPoint::None, goal);
        let ad = self.attach(prev, rule_node, EntryPoint::None, goal, vars.len());
        if let NodeKind::Rule(r) = &mut self.node_mut(rule_node).kind {
            r.unary.adapter = AdapterId::from_raw(ad);
            r.unary.db = link;
        }
    }

    fn action(&mut self, a: &Act, goal: u32, vars: Option<&[String]>) -> Action {
        if a.complete_goal {
            return Action {
                name: String::new(),
                params: None,
                negate: false,
                goal_or_hook: goal as i32,
            };
        }
        let f = self.function(&a.name, a.args.len());
        let decl = self.functions[f].signature.params.clone();
        let params = a
            .args
            .iter()
            .enumerate()
            .map(|(i, t)| match t {
                Term::Var(name) => {
                    let idx = vars
                        .and_then(|vs| vs.iter().position(|n| n == name))
                        .unwrap_or_else(|| panic!("builder: unbound {name} in action {}", a.name));
                    Value {
                        index: idx as i8,
                        flags: vf::SPEC_VARIABLE | vf::IS_TYPE | vf::ADAPTED,
                        constant: None,
                    }
                }
                Term::Lit(lit, ty) => {
                    // Untyped literals take the declared parameter type when the roots agree.
                    let ty = ty.or_else(|| {
                        decl.get(i)
                            .copied()
                            .filter(|&t| self.root(t) == self.root(lit.default_type()))
                    });
                    Value {
                        index: -1,
                        flags: vf::SPEC_TYPED | vf::VALID,
                        constant: Some(self.constant(lit, ty)),
                    }
                }
                Term::Any => panic!("builder: _ in action {}", a.name),
            })
            .collect();
        Action {
            name: a.name.clone(),
            params: Some(params),
            negate: a.negate,
            goal_or_hook: 0,
        }
    }

    pub fn build(mut self) -> Story {
        for (goal, rule, is_query) in std::mem::take(&mut self.pending_rules) {
            self.compile_rule(goal, &rule, is_query);
        }
        // Every database gets its start node (and storage), used in a condition or not.
        for f in 0..self.functions.len() {
            if self.functions[f].kind == FunctionKind::Database {
                self.function_node(f);
            }
        }
        let mut goals = Vec::new();
        let defs = std::mem::take(&mut self.goals);
        for (i, g) in defs.iter().enumerate() {
            let id = i as u32 + 1;
            let subgoals = defs
                .iter()
                .enumerate()
                .filter(|(_, c)| c.parents.contains(&id))
                .map(|(j, _)| GoalId::from_raw(j as u32 + 1).expect("non-zero"))
                .collect();
            let root = g.parents.is_empty();
            let init = g.init.iter().map(|a| self.action(a, id, None)).collect();
            let exit = g.exit.iter().map(|a| self.action(a, id, None)).collect();
            goals.push(Goal {
                id: GoalId::from_raw(id).expect("non-zero"),
                name: g.name.clone(),
                combiner: g.combiner.unwrap_or(if root {
                    SubGoalCombiner::Or
                } else {
                    SubGoalCombiner::And
                }),
                parents: g
                    .parents
                    .iter()
                    .map(|&p| GoalId::from_raw(p).expect("non-zero"))
                    .collect(),
                subgoals,
                flags: if root { 0 } else { 2 },
                init_actions: init,
                exit_actions: exit,
            });
        }
        let mut story_version = [0u8; 0x80];
        story_version[..7].copy_from_slice(b"1.0.0.0");
        Story::from_parts(
            Header {
                buffer_flag: 0,
                banner: "Synthetic story (bg3-osiris builder)".into(),
                major: 1,
                minor: 15,
                big_endian: false,
                unused: 0,
                story_version,
                debug_flags: 0,
            },
            self.types,
            Vec::new(),
            self.functions,
            self.nodes,
            self.adapters,
            self.databases,
            goals,
            Vec::new(),
        )
        .expect("builder produces unique ids")
    }
}
