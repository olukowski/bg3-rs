//! The immutable part of a loaded story: the rete network, adaptors, functions and goal
//! sections, resolved from [`bg3_story::Story`] into dense tables indexed by raw id.

use std::collections::HashMap;

use bg3_story::{
    Action, Adapter, Connection, ConstantData, DbLink, EntryPoint, FunctionKind, NodeKind, RelOp,
    Story, SubGoalCombiner, TypeId, value_flags,
};

use crate::value::{Data, Root, StringTable, TypeTable, Value};

/// A successor edge (`CReteConnection`). Raw ids, 0 = none.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Conn {
    pub node: u32,
    pub entry: u32,
    pub goal: u32,
}

impl Conn {
    fn from(c: &Connection) -> Self {
        Self {
            node: c.node.map_or(0, |n| n.get()),
            entry: c.entry.to_raw(),
            goal: c.goal.map_or(0, |g| g.get()),
        }
    }
}

/// A token generator link (`DbLink`): the closest database whose tuples can regenerate the
/// tokens of a node, the first edge below it, and the distance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Link {
    pub node: u32,
    pub conn: Conn,
    pub indirection: u8,
}

impl Link {
    fn from(l: &DbLink) -> Self {
        Self {
            node: l.node.map_or(0, |n| n.get()),
            conn: Conn::from(&l.connection),
            indirection: l.indirection,
        }
    }
}

#[derive(Debug)]
pub(crate) struct JoinData {
    pub negated: bool,
    pub next: Conn,
    pub left_parent: u32,
    pub right_parent: u32,
    pub left_adapter: u32,
    pub right_adapter: u32,
    pub left_link: Link,
    pub right_link: Link,
}

#[derive(Debug)]
pub(crate) enum Param {
    Const(Value),
    /// A rule variable shared with the action (`COsiVariable` bound at load by
    /// `CReteRuleActionPart`'s constructor). `save` = the variable has the shared flag (0x20)
    /// and is saved/restored around nested DB inserts and PROC calls.
    Var {
        index: u8,
        save: bool,
    },
}

#[derive(Debug)]
pub(crate) struct PParam {
    pub param: Param,
    /// Out-parameter flag (0x10): passed to the engine as a typed empty argument.
    pub out: bool,
}

#[derive(Debug)]
pub(crate) enum ActionKind {
    GoalCompleted(u32),
    DebugHook,
    Function { func: Option<usize> },
}

#[derive(Debug)]
pub(crate) struct PAction {
    pub name: String,
    pub kind: ActionKind,
    pub params: Vec<PParam>,
    pub negate: bool,
}

#[derive(Debug)]
pub(crate) struct RuleData {
    pub adapter: u32,
    pub actions: Vec<PAction>,
    pub variables: Vec<Value>,
    pub is_query: bool,
    pub line: u32,
}

#[derive(Debug)]
pub(crate) struct CompareData {
    pub next: Conn,
    pub parent: u32,
    pub adapter: u32,
    pub link: Link,
    pub left_var: u8,
    pub right_var: u8,
    pub left_value: Value,
    pub right_value: Value,
    pub op: RelOp,
}

#[derive(Debug)]
pub(crate) enum PKind {
    Start { successors: Vec<Conn> },
    DivQuery,
    InternalQuery,
    UserQuery,
    Join(Box<JoinData>),
    Compare(Box<CompareData>),
    Rule(Box<RuleData>),
}

#[derive(Debug)]
pub(crate) struct PNode {
    pub kind: PKind,
    /// Database owned by the node (raw id, 0 = none).
    pub db: u32,
    /// Function of a start/query node.
    pub func: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct PAdapter {
    pub id: u32,
    /// Per physical column: rule variable or -1.
    pub logical: Vec<i8>,
    /// `(column, value)` in file order.
    pub constants: Vec<(u8, Value)>,
    /// Logical → physical, sorted by variable (a `std::map` in Osiris).
    pub map: Vec<(u8, u8)>,
}

impl PAdapter {
    pub fn physical(&self, var: u8) -> Option<u8> {
        self.map
            .binary_search_by_key(&var, |&(l, _)| l)
            .ok()
            .map(|i| self.map[i].1)
    }

    pub fn constant_at(&self, col: usize) -> Option<&Value> {
        self.constants
            .iter()
            .find(|(c, _)| *c as usize == col)
            .map(|(_, v)| v)
    }
}

#[derive(Debug)]
pub(crate) struct PFunc {
    pub name: String,
    pub kind: FunctionKind,
    pub node: u32,
    pub params: Vec<TypeId>,
    pub out: Vec<bool>,
    /// `CFourKey` word 0: the built-in id of `syscall`/`sysquery` functions.
    pub key0: u32,
}

#[derive(Debug)]
pub(crate) struct PGoal {
    pub name: String,
    pub combiner: SubGoalCombiner,
    pub parents: Vec<u32>,
    pub subgoals: Vec<u32>,
    pub flags: u8,
    pub init: Vec<PAction>,
    pub exit: Vec<PAction>,
}

#[derive(Debug)]
pub(crate) struct Program {
    pub nodes: Vec<Option<PNode>>,
    pub adapters: Vec<Option<PAdapter>>,
    /// Arity per database id (index = raw id).
    pub db_arity: Vec<Option<usize>>,
    pub functions: Vec<PFunc>,
    /// `(lowercase name, arity)` → function. Osiris looks functions up case-insensitively.
    pub function_index: HashMap<(String, u8), usize>,
    pub goals: Vec<Option<PGoal>>,
    pub global_actions: Vec<PAction>,
    pub story_version: String,
    pub types: TypeTable,
    /// Type names by id (for the runtime log).
    pub type_names: Vec<String>,
    /// Enumeration labels by type id (for the runtime log).
    pub enums: HashMap<TypeId, Vec<(String, u64)>>,
}

pub(crate) fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

impl Program {
    pub fn function(&self, name: &str, arity: usize) -> Option<usize> {
        self.function_index.get(&(fold(name), arity as u8)).copied()
    }

    pub fn node(&self, id: u32) -> Option<&PNode> {
        self.nodes.get(id as usize).and_then(|n| n.as_ref())
    }

    pub fn adapter(&self, id: u32) -> Option<&PAdapter> {
        self.adapters.get(id as usize).and_then(|a| a.as_ref())
    }

    pub fn goal(&self, id: u32) -> Option<&PGoal> {
        self.goals.get(id as usize).and_then(|g| g.as_ref())
    }

    pub fn goal_by_name(&self, name: &str) -> Option<u32> {
        self.goals
            .iter()
            .enumerate()
            .find(|(_, g)| {
                g.as_ref()
                    .is_some_and(|g| g.name.eq_ignore_ascii_case(name))
            })
            .map(|(i, _)| i as u32)
    }

    pub fn build(story: &Story, strings: &mut StringTable) -> (Self, Vec<String>) {
        let types = TypeTable::from_story(story);
        let mut cx = Cx {
            story,
            types: &types,
            strings,
            problems: Vec::new(),
        };

        let mut functions = Vec::with_capacity(story.functions.len());
        let mut function_index = HashMap::new();
        for (i, f) in story.functions.iter().enumerate() {
            let sig = &f.signature;
            functions.push(PFunc {
                name: sig.name.clone(),
                kind: f.kind,
                node: f.node.map_or(0, |n| n.get()),
                params: sig.params.clone(),
                out: (0..sig.params.len()).map(|i| sig.is_out(i)).collect(),
                key0: f.keys[0],
            });
            function_index
                .entry((fold(&sig.name), sig.arity()))
                .or_insert(i);
        }
        let lookup =
            |name: &str, arity: usize| function_index.get(&(fold(name), arity as u8)).copied();

        let mut adapters: Vec<Option<PAdapter>> = Vec::new();
        adapters.resize_with(story.adapters.id_bound(), || None);
        for a in &story.adapters {
            adapters[a.id.get() as usize] = Some(cx.adapter(a));
        }

        let mut db_arity = vec![None; story.databases.id_bound()];
        for d in &story.databases {
            db_arity[d.id.get() as usize] = Some(d.param_types.len());
        }

        let mut nodes: Vec<Option<PNode>> = Vec::new();
        nodes.resize_with(story.nodes.id_bound(), || None);
        for n in &story.nodes {
            let func = if n.name.is_empty() {
                None
            } else {
                lookup(&n.name, n.arity as usize)
            };
            let kind = match &n.kind {
                NodeKind::Database(s) | NodeKind::Event(s) => PKind::Start {
                    successors: s.successors.iter().map(Conn::from).collect(),
                },
                NodeKind::DivQuery => PKind::DivQuery,
                NodeKind::InternalQuery => PKind::InternalQuery,
                NodeKind::UserQuery => PKind::UserQuery,
                NodeKind::And(j) | NodeKind::NotAnd(j) => PKind::Join(Box::new(JoinData {
                    negated: matches!(n.kind, NodeKind::NotAnd(_)),
                    next: Conn::from(&j.next),
                    left_parent: j.left_parent.map_or(0, |p| p.get()),
                    right_parent: j.right_parent.map_or(0, |p| p.get()),
                    left_adapter: j.left_adapter.map_or(0, |a| a.get()),
                    right_adapter: j.right_adapter.map_or(0, |a| a.get()),
                    left_link: Link::from(&j.left_db),
                    right_link: Link::from(&j.right_db),
                })),
                NodeKind::Compare(c) => PKind::Compare(Box::new(CompareData {
                    next: Conn::from(&c.unary.next),
                    parent: c.unary.parent.map_or(0, |p| p.get()),
                    adapter: c.unary.adapter.map_or(0, |a| a.get()),
                    link: Link::from(&c.unary.db),
                    left_var: c.left_var as u8,
                    right_var: c.right_var as u8,
                    left_value: cx.value(&c.left_value),
                    right_value: cx.value(&c.right_value),
                    op: c.op,
                })),
                NodeKind::Rule(r) => {
                    let variables: Vec<Value> = r.variables.iter().map(|v| cx.value(v)).collect();
                    let shared: Vec<bool> = r
                        .variables
                        .iter()
                        .map(|v| v.flags & value_flags::IS_TYPE != 0)
                        .collect();
                    let actions = r
                        .actions
                        .iter()
                        .map(|a| cx.action(a, Some(&shared), &lookup))
                        .collect();
                    PKind::Rule(Box::new(RuleData {
                        adapter: r.unary.adapter.map_or(0, |a| a.get()),
                        actions,
                        variables,
                        is_query: r.is_query,
                        line: r.line,
                    }))
                }
            };
            nodes[n.id.get() as usize] = Some(PNode {
                kind,
                db: n.database.map_or(0, |d| d.get()),
                func,
            });
        }

        let mut goals: Vec<Option<PGoal>> = Vec::new();
        goals.resize_with(story.goals.id_bound(), || None);
        for g in &story.goals {
            goals[g.id.get() as usize] = Some(PGoal {
                name: g.name.clone(),
                combiner: g.combiner,
                parents: g.parents.iter().map(|p| p.get()).collect(),
                subgoals: g.subgoals.iter().map(|p| p.get()).collect(),
                flags: g.flags,
                init: g
                    .init_actions
                    .iter()
                    .map(|a| cx.action(a, None, &lookup))
                    .collect(),
                exit: g
                    .exit_actions
                    .iter()
                    .map(|a| cx.action(a, None, &lookup))
                    .collect(),
            });
        }
        let global_actions = story
            .global_actions
            .iter()
            .map(|a| cx.action(a, None, &lookup))
            .collect();
        let problems = cx.problems;

        (
            Self {
                nodes,
                adapters,
                db_arity,
                functions,
                function_index,
                goals,
                global_actions,
                story_version: story.header.story_version_str().to_owned(),
                types,
                type_names: (0..256u16).map(|id| story.type_name(id)).collect(),
                enums: story
                    .enums
                    .iter()
                    .map(|e| (e.type_id, e.elements.clone()))
                    .collect(),
            },
            problems,
        )
    }
}

struct Cx<'a> {
    story: &'a Story,
    types: &'a TypeTable,
    strings: &'a mut StringTable,
    problems: Vec<String>,
}

/// A stored value: its constant if the VALID flag is set, otherwise an unset value.
pub(crate) fn stored_value(
    story: &Story,
    types: &TypeTable,
    strings: &mut StringTable,
    v: &bg3_story::Value,
) -> Value {
    let mut cx = Cx {
        story,
        types,
        strings,
        problems: Vec::new(),
    };
    cx.value(v)
}

impl Cx<'_> {
    /// A stored value: its constant if the VALID flag is set, otherwise an unset value.
    // bg3: COsiTypedValueBase::COsiTypedValueBase(TOsiValueSpecializationType, COsiSmartBuf*) (4.1.1.7631656)
    fn value(&mut self, v: &bg3_story::Value) -> Value {
        let Some(c) = &v.constant else {
            return Value::UNDEF;
        };
        let ty = c.type_id;
        let root = self.types.root(ty);
        if v.flags & value_flags::VALID == 0 && v.spec() != value_flags::SPEC_VALUE {
            // Typed but unset (e.g. a variable's declared type).
            return Value::undef(ty, root);
        }
        let data = match (&c.data, root) {
            (ConstantData::None, _) => Data::Undef,
            (ConstantData::Integer(i), _) => Data::Int(*i),
            (ConstantData::Integer64(i), _) => Data::Int64(*i),
            (ConstantData::Real(f), _) => Data::Real(*f),
            (ConstantData::String(s), _) => {
                Data::Str(self.strings.intern(s.as_deref().unwrap_or(""), false))
            }
            (ConstantData::GuidString(s), _) => {
                // COsiSmartBuf::readGuidString drops spaces.
                let s: String = s
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .filter(|&c| c != ' ')
                    .collect();
                Data::Str(self.strings.intern(&s, true))
            }
            (ConstantData::Raw(i), Root::Integer64) => Data::Int64(*i as i64),
            (ConstantData::Raw(i), _) => Data::Int(*i),
            (ConstantData::Enum(label), _) => {
                match self
                    .story
                    .enum_def(ty)
                    .and_then(|e| e.elements.iter().find(|(l, _)| l == label))
                {
                    Some(&(_, v)) => match root {
                        Root::Integer => Data::Int(v as i32),
                        _ => Data::Int64(v as i64),
                    },
                    None => {
                        self.problems
                            .push(format!("unknown enum label {label} for type {ty}"));
                        Data::Undef
                    }
                }
            }
            (ConstantData::Object(name), _) => {
                self.problems
                    .push(format!("DIV object constant {name} (unsupported)"));
                Data::Undef
            }
        };
        Value { ty, root, data }
    }

    fn adapter(&mut self, a: &Adapter) -> PAdapter {
        let constants = a
            .constants
            .iter()
            .map(|c| (c.index as u8, self.value(c)))
            .collect();
        let mut map: Vec<(u8, u8)> = Vec::new();
        for &(l, p) in &a.logical_to_physical {
            // std::map: the first insertion of a key wins.
            if !map.iter().any(|&(k, _)| k == l) {
                map.push((l, p));
            }
        }
        map.sort_by_key(|&(l, _)| l);
        PAdapter {
            id: a.id.get(),
            logical: a.logical_indices.clone(),
            constants,
            map,
        }
    }

    fn action(
        &mut self,
        a: &Action,
        shared: Option<&[bool]>,
        lookup: &dyn Fn(&str, usize) -> Option<usize>,
    ) -> PAction {
        let kind = if a.goal_or_hook > 0 {
            ActionKind::GoalCompleted(a.goal_or_hook as u32)
        } else if a.goal_or_hook < 0 {
            ActionKind::DebugHook
        } else {
            let arity = a.params.as_ref().map_or(0, |p| p.len());
            let func = lookup(&a.name, arity);
            ActionKind::Function { func }
        };
        let params = a
            .params
            .iter()
            .flatten()
            .map(|p| {
                let out = p.flags & value_flags::OUT_PARAM != 0;
                // bg3: CReteRuleActionPart::CReteRuleActionPart(COsiSmartBuf*) — grounded
                // variable parameters are replaced by the rule's variable objects.
                let param = if p.is_variable() && p.is_adapted() && shared.is_some() {
                    let shared = shared.unwrap_or(&[]);
                    let index = p.index as u8;
                    if index as usize >= shared.len() {
                        self.problems
                            .push(format!("action {}: variable {index} out of range", a.name));
                    }
                    Param::Var {
                        index,
                        save: shared.get(index as usize).copied().unwrap_or(false),
                    }
                } else {
                    if p.is_variable() {
                        self.problems
                            .push(format!("action {}: parameter is not grounded", a.name));
                    }
                    Param::Const(self.value(p))
                };
                PParam { param, out }
            })
            .collect();
        PAction {
            name: a.name.clone(),
            kind,
            params,
            negate: a.negate,
        }
    }
}

/// Map an entry point to the opposite side of a binary node (table at 0x8cf74:
/// none→none, left→right, right→left).
// bg3: CReteBinaryNode::JoinInternal — entry point flip (4.1.1.7631656)
pub(crate) fn other_entry(entry: u32) -> u32 {
    match entry {
        e if e == EntryPoint::Left.to_raw() => EntryPoint::Right.to_raw(),
        e if e == EntryPoint::Right.to_raw() => EntryPoint::Left.to_raw(),
        _ => 0,
    }
}
