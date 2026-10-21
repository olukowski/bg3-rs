//! In-memory model of a compiled Osiris story.
//!
//! The model mirrors the on-disk structure closely (so it can be re-serialised byte-for-byte in
//! tests) but replaces raw `u32` references with typed ids. All ids are 1-based as in the file;
//! `0` on disk means "no reference" and becomes `None`.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::num::NonZeroU32;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        pub struct $name(pub NonZeroU32);

        impl $name {
            /// `None` for the on-disk null reference `0`.
            pub fn from_raw(raw: u32) -> Option<Self> {
                NonZeroU32::new(raw).map(Self)
            }

            /// The raw 1-based id.
            pub fn get(self) -> u32 {
                self.0.get()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "#{}", self.0)
            }
        }
    };
}

id_type!(
    /// Index of a rete node (`CReteNode`).
    NodeId
);
id_type!(
    /// Index of a tuple adaptor (`CReteAdaptor`).
    AdapterId
);
id_type!(
    /// Index of a database (`CReteDBase`).
    DatabaseId
);
id_type!(
    /// Index of a goal (`COsiGoal`).
    GoalId
);

#[cfg(test)]
pub(crate) fn raw<I: Into<u32>>(id: Option<I>) -> u32 {
    id.map_or(0, Into::into)
}

macro_rules! id_into_u32 {
    ($($t:ident),*) => {$(
        impl From<$t> for u32 {
            fn from(id: $t) -> u32 {
                id.get()
            }
        }
    )*};
}
id_into_u32!(NodeId, AdapterId, DatabaseId, GoalId);

/// Something stored in an id-indexed [`Table`].
pub trait Keyed {
    type Id: Copy + Into<u32>;
    fn id(&self) -> Self::Id;
}

/// Items in file order with O(1) lookup by their 1-based id (ids may in principle have gaps;
/// Osiris' factories pad missing slots with null).
#[derive(Debug, Clone)]
pub struct Table<T: Keyed> {
    items: Vec<T>,
    slots: Vec<u32>,
    _id: PhantomData<T::Id>,
}

impl<T: Keyed> Default for Table<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            slots: Vec::new(),
            _id: PhantomData,
        }
    }
}

impl<T: Keyed> Table<T> {
    /// Returns `Err(id)` on a duplicate id.
    pub(crate) fn from_items(items: Vec<T>) -> Result<Self, u32> {
        let mut slots = Vec::new();
        for (pos, item) in items.iter().enumerate() {
            let idx = item.id().into() as usize - 1;
            if slots.len() <= idx {
                slots.resize(idx + 1, u32::MAX);
            }
            if slots[idx] != u32::MAX {
                return Err(idx as u32 + 1);
            }
            slots[idx] = pos as u32;
        }
        Ok(Self {
            items,
            slots,
            _id: PhantomData,
        })
    }

    pub fn get(&self, id: T::Id) -> Option<&T> {
        let idx = id.into() as usize - 1;
        match self.slots.get(idx) {
            Some(&pos) if pos != u32::MAX => Some(&self.items[pos as usize]),
            _ => None,
        }
    }

    /// Items in file order.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Highest id + 1 (size of a dense id-indexed side table).
    pub fn id_bound(&self) -> usize {
        self.slots.len() + 1
    }
}

impl<'a, T: Keyed> IntoIterator for &'a Table<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

/// A whole compiled story (`story.div.osi`, or the story embedded in a save).
#[derive(Debug, Clone)]
pub struct Story {
    pub header: Header,
    /// Custom/alias types from the header (the built-in types 0..=5 are implicit).
    pub types: Vec<TypeInfo>,
    /// Enumerations (format 1.13+).
    pub enums: Vec<EnumDef>,
    /// "DIV objects": named engine objects the story may refer to (empty in BG3).
    pub objects: Vec<DivObject>,
    /// Every function signature the story knows: events, queries, calls, procs, DBs, ...
    pub functions: Vec<Function>,
    pub nodes: Table<Node>,
    pub adapters: Table<Adapter>,
    pub databases: Table<Database>,
    pub goals: Table<Goal>,
    /// Actions run once when the story is initialised.
    pub global_actions: Vec<Action>,
    pub(crate) type_index: TypeIndex,
    pub(crate) function_index: HashMap<(String, u8), usize>,
}

/// File header.
// bg3: COsiris::_ReadHeader (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// First byte of the file, consumed by the `COsiSmartBuf` constructor (0 in all BG3 files).
    pub buffer_flag: u8,
    /// Free-form banner, e.g. `Osiris save file dd. 09/21/26 13:32:45. Version 1.8.`
    pub banner: String,
    pub major: u8,
    pub minor: u8,
    /// Byte order flag; only little-endian (false) files are supported.
    pub big_endian: bool,
    pub unused: u8,
    /// Fixed 128-byte NUL-padded build string, e.g. `1.0.0.7143220` (format 1.2+).
    pub story_version: [u8; 0x80],
    /// Parser/debug flags the story was compiled with (format 1.3+).
    pub debug_flags: u32,
}

impl Header {
    /// `(major << 8) | minor`, e.g. `0x010f` for 1.15.
    pub fn version(&self) -> u16 {
        (self.major as u16) << 8 | self.minor as u16
    }

    pub fn story_version_str(&self) -> &str {
        let end = self
            .story_version
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.story_version.len());
        std::str::from_utf8(&self.story_version[..end]).unwrap_or("")
    }
}

/// A type id as used in signatures and values. 0..=5 are the built-in types.
pub type TypeId = u16;

/// Built-in value types (`TOsiValueType` roots).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinType {
    None = 0,
    Integer = 1,
    Integer64 = 2,
    Real = 3,
    String = 4,
    GuidString = 5,
}

impl BuiltinType {
    pub fn from_id(id: TypeId) -> Option<Self> {
        Some(match id {
            0 => Self::None,
            1 => Self::Integer,
            2 => Self::Integer64,
            3 => Self::Real,
            4 => Self::String,
            5 => Self::GuidString,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Integer => "INTEGER",
            Self::Integer64 => "INTEGER64",
            Self::Real => "REAL",
            Self::String => "STRING",
            Self::GuidString => "GUIDSTRING",
        }
    }
}

/// A named type from the header's type table.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeInfo {
    pub name: String,
    /// Type id (1..=127).
    pub index: u8,
    /// Type this one aliases (0 = none, i.e. a "DIV object" type or a built-in).
    pub alias: u8,
}

/// An enumeration: a type whose values are named 64-bit constants.
// bg3: COsiris::_ReadEnumElements (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub type_id: TypeId,
    pub elements: Vec<(String, u64)>,
}

/// A named engine object.
// bg3: COsiDIVObjectData::COsiDIVObjectData(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct DivObject {
    pub name: String,
    pub type_id: u8,
    pub keys: [u32; 4],
}

/// What kind of function a signature describes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FunctionKind {
    /// Engine → story notification (`event`).
    Event,
    /// Built-in engine query (`query`).
    Query,
    /// Built-in engine call (`call`).
    Call,
    /// Story database (`DB_*`).
    Database,
    /// Story procedure (`PROC_*`).
    Proc,
    /// Osiris-internal query (`sysquery`).
    SysQuery,
    /// Osiris-internal call (`syscall`).
    SysCall,
    /// Story-defined query (`QRY_*`).
    UserQuery,
    Unknown(u8),
}

impl FunctionKind {
    pub fn from_raw(v: u8) -> Self {
        match v {
            1 => Self::Event,
            2 => Self::Query,
            3 => Self::Call,
            4 => Self::Database,
            5 => Self::Proc,
            6 => Self::SysQuery,
            7 => Self::SysCall,
            8 => Self::UserQuery,
            v => Self::Unknown(v),
        }
    }

    pub fn to_raw(self) -> u8 {
        match self {
            Self::Event => 1,
            Self::Query => 2,
            Self::Call => 3,
            Self::Database => 4,
            Self::Proc => 5,
            Self::SysQuery => 6,
            Self::SysCall => 7,
            Self::UserQuery => 8,
            Self::Unknown(v) => v,
        }
    }

    pub fn keyword(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Query => "query",
            Self::Call => "call",
            Self::Database => "db",
            Self::Proc => "proc",
            Self::SysQuery => "sysquery",
            Self::SysCall => "syscall",
            Self::UserQuery => "userquery",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// A function definition.
// bg3: COsiFunctionData::COsiFunctionData(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// Source line of the definition.
    pub line: u32,
    /// How many rule conditions reference the function.
    pub condition_refs: u32,
    /// How many rule actions reference the function.
    pub action_refs: u32,
    /// Start node of the rete network for this function (DB/event/proc/query nodes).
    pub node: Option<NodeId>,
    pub kind: FunctionKind,
    /// Engine binding key (`CFourKey`): for engine functions, identifies the native handler.
    pub keys: [u32; 4],
    pub signature: Signature,
}

/// Name, parameter types and in/out directions.
// bg3: COsiFunctionDef::COsiFunctionDef(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub name: String,
    /// Bit `0x80 >> (i & 7)` of byte `i >> 3` set = parameter `i` is an out-parameter.
    pub out_param_mask: Vec<u8>,
    pub params: Vec<TypeId>,
}

impl Signature {
    pub fn is_out(&self, i: usize) -> bool {
        self.out_param_mask
            .get(i >> 3)
            .is_some_and(|b| b & (0x80 >> (i & 7)) != 0)
    }

    pub fn arity(&self) -> u8 {
        self.params.len() as u8
    }
}

/// Bits of [`Value::flags`] (`COsiTypedValueBase` byte 11).
pub mod value_flags {
    /// Low three bits: specialisation (1 = plain value, 2 = typed parameter, 3 = variable).
    pub const SPEC_MASK: u8 = 0x07;
    /// A constant payload follows / is bound.
    pub const VALID: u8 = 0x08;
    /// Parameter is an out-parameter.
    pub const OUT_PARAM: u8 = 0x10;
    /// LSLib calls this "IsAType"; in BG3 stories it is set on every rule variable, so its
    /// exact meaning is unconfirmed.
    pub const IS_TYPE: u8 = 0x20;
    /// Variable is the anonymous `_`.
    pub const UNUSED: u8 = 0x40;
    /// Variable has been bound through an adaptor.
    pub const ADAPTED: u8 = 0x80;

    pub const SPEC_VALUE: u8 = 1;
    pub const SPEC_TYPED: u8 = 2;
    pub const SPEC_VARIABLE: u8 = 3;
}

/// A typed value, parameter or rule variable (`COsiTypedValue` / `COsiVariable`).
// bg3: COsiTypedValueBase::COsiTypedValueBase(TOsiValueSpecializationType, COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Value {
    /// Column index (adaptor constants), variable index (rule variables / variable
    /// parameters) or `-1`.
    pub index: i8,
    /// See [`value_flags`].
    pub flags: u8,
    /// The constant, present iff `flags & VALID`.
    pub constant: Option<Constant>,
}

impl Value {
    pub fn spec(&self) -> u8 {
        self.flags & value_flags::SPEC_MASK
    }

    pub fn is_variable(&self) -> bool {
        self.spec() == value_flags::SPEC_VARIABLE
    }

    pub fn is_unused(&self) -> bool {
        self.flags & value_flags::UNUSED != 0
    }

    pub fn is_out_param(&self) -> bool {
        self.flags & value_flags::OUT_PARAM != 0
    }

    /// See [`value_flags::IS_TYPE`].
    pub fn is_type_marker(&self) -> bool {
        self.flags & value_flags::IS_TYPE != 0
    }

    pub fn is_adapted(&self) -> bool {
        self.flags & value_flags::ADAPTED != 0
    }

    /// Declared type of the constant, if any.
    pub fn type_id(&self) -> Option<TypeId> {
        self.constant.as_ref().map(|c| c.type_id)
    }
}

/// A typed constant.
#[derive(Debug, Clone, PartialEq)]
pub struct Constant {
    /// Declared type (may be an alias; the payload follows its root type).
    pub type_id: TypeId,
    pub data: ConstantData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstantData {
    /// Root type 0: no payload.
    None,
    Integer(i32),
    Integer64(i64),
    Real(f32),
    /// `None` = null string.
    String(Option<String>),
    /// As stored; Osiris strips spaces from GUID strings when loading.
    GuidString(Option<String>),
    /// Name of a DIV object (types without a built-in root).
    Object(String),
    /// Enum label (on-disk marker `'e'`), resolved to a value via the enum's symbol table.
    Enum(String),
    /// Raw 32-bit payload (on-disk marker `'1'`).
    Raw(i32),
}

/// Where a tuple enters a binary node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryPoint {
    None,
    Left,
    Right,
    Unknown(u32),
}

impl EntryPoint {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Left,
            2 => Self::Right,
            v => Self::Unknown(v),
        }
    }

    pub fn to_raw(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Left => 1,
            Self::Right => 2,
            Self::Unknown(v) => v,
        }
    }
}

/// An edge to a successor node (`CReteConnection`).
// bg3: CReteConnection::Read (4.1.1.7631656)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Connection {
    pub node: Option<NodeId>,
    pub entry: EntryPoint,
    /// Goal owning the rule the successor belongs to.
    pub goal: Option<GoalId>,
}

/// A database a join/compare/rule node reads directly (used for the DB side of joins and for
/// re-evaluating conditions when facts change).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DbLink {
    /// The database's start node.
    pub node: Option<NodeId>,
    pub connection: Connection,
    /// How many hops away the database is.
    pub indirection: u8,
}

/// A rete node.
// bg3: CReteNode::CReteNode(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    /// Database owned/used by this node.
    pub database: Option<DatabaseId>,
    /// Function name for start nodes; empty for inner nodes.
    pub name: String,
    /// Function arity (only stored when `name` is non-empty).
    pub arity: u8,
    pub kind: NodeKind,
}

impl Keyed for Node {
    type Id = NodeId;
    fn id(&self) -> NodeId {
        self.id
    }
}

/// On-disk node type byte (`_ReadReteNodes` dispatch).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// 1 — database start node (`CReteFact`).
    Database(StartNode),
    /// 2 — event/proc start node (`CReteEvent`).
    Event(StartNode),
    /// 3 — built-in engine query (`CReteDIVQuery`). Queries are pulled by joins, so query
    /// nodes have no successor list.
    DivQuery,
    /// 4 — join (`CReteAnd`).
    And(JoinNode),
    /// 5 — negated join (`CReteNAnd`).
    NotAnd(JoinNode),
    /// 6 — comparison (`CReteRelCondition`).
    Compare(CompareNode),
    /// 7 — rule terminal with the THEN part (`CReteRuleActionPart`).
    Rule(RuleNode),
    /// 8 — Osiris-internal query (`CReteInternalQuery`).
    InternalQuery,
    /// 9 — story-defined query (`CReteOsiQuery`).
    UserQuery,
}

impl NodeKind {
    pub fn type_byte(&self) -> u8 {
        match self {
            Self::Database(_) => 1,
            Self::Event(_) => 2,
            Self::DivQuery => 3,
            Self::And(_) => 4,
            Self::NotAnd(_) => 5,
            Self::Compare(_) => 6,
            Self::Rule(_) => 7,
            Self::InternalQuery => 8,
            Self::UserQuery => 9,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Database(_) => "Database",
            Self::Event(_) => "Event",
            Self::DivQuery => "DivQuery",
            Self::And(_) => "And",
            Self::NotAnd(_) => "NotAnd",
            Self::Compare(_) => "Compare",
            Self::Rule(_) => "Rule",
            Self::InternalQuery => "InternalQuery",
            Self::UserQuery => "UserQuery",
        }
    }

    pub fn as_start(&self) -> Option<&StartNode> {
        match self {
            Self::Database(s) | Self::Event(s) => Some(s),
            _ => None,
        }
    }

    /// The single successor edge of an inner node.
    pub fn next(&self) -> Option<&Connection> {
        match self {
            Self::And(j) | Self::NotAnd(j) => Some(&j.next),
            Self::Compare(c) => Some(&c.unary.next),
            Self::Rule(r) => Some(&r.unary.next),
            _ => None,
        }
    }

    /// Whether this is a query node (pulled by joins instead of pushing tuples).
    pub fn is_query(&self) -> bool {
        matches!(self, Self::DivQuery | Self::InternalQuery | Self::UserQuery)
    }

    /// Outgoing edges (start nodes may have many).
    pub fn successors(&self) -> &[Connection] {
        match self {
            Self::Database(s) | Self::Event(s) => &s.successors,
            Self::DivQuery | Self::InternalQuery | Self::UserQuery => &[],
            Self::And(j) | Self::NotAnd(j) => std::slice::from_ref(&j.next),
            Self::Compare(c) => std::slice::from_ref(&c.unary.next),
            Self::Rule(r) => std::slice::from_ref(&r.unary.next),
        }
    }

    /// The single-parent part of compare/rule nodes.
    pub fn unary(&self) -> Option<&UnaryNode> {
        match self {
            Self::Compare(c) => Some(&c.unary),
            Self::Rule(r) => Some(&r.unary),
            _ => None,
        }
    }
}

/// Root of a function's network: fans incoming tuples out to all rules using the function.
// bg3: CReteStartNode::CReteStartNode(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StartNode {
    pub successors: Vec<Connection>,
}

/// Two-parent join (`CReteBinaryNode`).
// bg3: CReteBinaryNode::CReteBinaryNode(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct JoinNode {
    pub next: Connection,
    pub left_parent: Option<NodeId>,
    pub right_parent: Option<NodeId>,
    pub left_adapter: Option<AdapterId>,
    pub right_adapter: Option<AdapterId>,
    pub left_db: DbLink,
    pub right_db: DbLink,
}

/// Single-parent node (`CReteUnaryNode`), base of compare and rule nodes.
// bg3: CReteUnaryNode::CReteUnaryNode(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct UnaryNode {
    pub next: Connection,
    pub parent: Option<NodeId>,
    pub adapter: Option<AdapterId>,
    pub db: DbLink,
}

/// Comparison operator of a [`CompareNode`] (`TOsiRelOp`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RelOp {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Equal,
    NotEqual,
    Unknown(u32),
}

impl RelOp {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Less,
            1 => Self::LessOrEqual,
            2 => Self::Greater,
            3 => Self::GreaterOrEqual,
            4 => Self::Equal,
            5 => Self::NotEqual,
            v => Self::Unknown(v),
        }
    }

    pub fn to_raw(self) -> u32 {
        match self {
            Self::Less => 0,
            Self::LessOrEqual => 1,
            Self::Greater => 2,
            Self::GreaterOrEqual => 3,
            Self::Equal => 4,
            Self::NotEqual => 5,
            Self::Unknown(v) => v,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Unknown(_) => "?op?",
        }
    }
}

/// `left OP right` filter over the parent's tuple.
// bg3: CReteRelCondition::CReteRelCondition(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct CompareNode {
    pub unary: UnaryNode,
    /// Variable index of the left operand, or `-1` to use `left_value`.
    pub left_var: i8,
    /// Variable index of the right operand, or `-1` to use `right_value`.
    pub right_var: i8,
    pub left_value: Value,
    pub right_value: Value,
    pub op: RelOp,
}

/// Rule terminal: executes the THEN actions for every tuple that reaches it.
// bg3: CReteRuleActionPart::CReteRuleActionPart(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct RuleNode {
    pub unary: UnaryNode,
    pub actions: Vec<Action>,
    /// The rule's variables (`_Var0`, `_Var1`, ...), indexed by logical column.
    pub variables: Vec<Value>,
    /// Source line of the THEN part (`u32::MAX` if unknown).
    pub line: u32,
    /// Rule defines a user query (`QRY`), format 1.6+.
    pub is_query: bool,
}

/// A call in an action list (rule THEN part, goal init/exit, global actions).
// bg3: COsiFunctionParseData::COsiFunctionParseData(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// Called function; empty for a goal-completion pseudo action.
    pub name: String,
    /// `None` when the call was compiled without a parameter list.
    pub params: Option<Vec<Value>>,
    /// `NOT DB_Foo(...)`: delete instead of insert.
    pub negate: bool,
    /// `0` for a normal call; `> 0` (with an empty name): `GoalCompleted` for that goal id;
    /// `< 0`: run debug hook `-n` instead of a call.
    // bg3: COsiFunctionParseData::Fire (4.1.1.7631656)
    pub goal_or_hook: i32,
}

/// Tuple adaptor: maps a parent's physical columns onto the rule's logical variables and
/// supplies constants for columns bound to literals.
// bg3: CReteAdaptor::CReteAdaptor(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Adapter {
    pub id: AdapterId,
    /// Constant columns; [`Value::index`] is the physical column.
    pub constants: Vec<Value>,
    /// Per physical column: logical (variable) index, or `-1` if the column is a constant or
    /// ignored.
    pub logical_indices: Vec<i8>,
    /// Logical → physical column pairs.
    pub logical_to_physical: Vec<(u8, u8)>,
}

impl Keyed for Adapter {
    type Id = AdapterId;
    fn id(&self) -> AdapterId {
        self.id
    }
}

/// A database with its initial facts.
// bg3: CReteDBase::CReteDBase(COsiSmartBuf*, char const*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Database {
    pub id: DatabaseId,
    pub param_types: Vec<TypeId>,
    /// Each fact is one row (one value per column).
    pub facts: Vec<Vec<Value>>,
}

impl Keyed for Database {
    type Id = DatabaseId;
    fn id(&self) -> DatabaseId {
        self.id
    }
}

/// How a goal's parents gate it: `Or` = enabled when any parent completes, `And` = when all
/// parents have completed.
// bg3: COsiGoal::CheckEnableRules (4.1.1.7631656)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubGoalCombiner {
    Or,
    And,
    Unknown(u8),
}

impl SubGoalCombiner {
    pub fn from_raw(v: u8) -> Self {
        match v {
            0 => Self::Or,
            1 => Self::And,
            v => Self::Unknown(v),
        }
    }

    pub fn to_raw(self) -> u8 {
        match self {
            Self::Or => 0,
            Self::And => 1,
            Self::Unknown(v) => v,
        }
    }
}

/// A goal (story script file).
// bg3: COsiGoal::COsiGoal(COsiSmartBuf*) (4.1.1.7631656)
#[derive(Debug, Clone, PartialEq)]
pub struct Goal {
    pub id: GoalId,
    pub name: String,
    pub combiner: SubGoalCombiner,
    pub parents: Vec<GoalId>,
    pub subgoals: Vec<GoalId>,
    /// Goal state flags (`COsiGoal` byte 0x68): bit 0 completed, bit 1 rules disabled
    /// (sleeping; compiled stories have it set on every goal with parents), bit 2 completion
    /// in progress/done. See `docs/research/osiris-vm.md`.
    pub flags: u8,
    pub init_actions: Vec<Action>,
    pub exit_actions: Vec<Action>,
}

impl Keyed for Goal {
    type Id = GoalId;
    fn id(&self) -> GoalId {
        self.id
    }
}

/// Alias resolution for type ids.
#[derive(Debug, Clone, Default)]
pub(crate) struct TypeIndex {
    /// `alias[id]`, 0 = none; also marks presence via `known`.
    alias: Vec<u8>,
    known: Vec<bool>,
}

impl TypeIndex {
    pub(crate) fn new(types: &[TypeInfo]) -> Self {
        let mut alias = vec![0u8; 256];
        let mut known = vec![false; 256];
        for t in types {
            alias[t.index as usize] = t.alias;
            known[t.index as usize] = true;
        }
        Self { alias, known }
    }

    /// Root of the alias chain: a built-in type, or `Err(id)` for an object type.
    // bg3: COsiTypedValueBase::COsiTypedValueBase(TOsiValueSpecializationType, COsiSmartBuf*) (4.1.1.7631656)
    pub(crate) fn root(&self, id: TypeId) -> Result<BuiltinType, TypeId> {
        let mut cur = id;
        // Bounded walk; the header loader rejects alias cycles in Osiris (VerifyTypeAliases).
        for _ in 0..256 {
            if let Some(b) = BuiltinType::from_id(cur) {
                return Ok(b);
            }
            if cur == 0x7f || cur as usize >= self.alias.len() || !self.known[cur as usize] {
                return Err(cur);
            }
            let next = self.alias[cur as usize] as TypeId;
            if next == 0 {
                return Err(cur);
            }
            cur = next;
        }
        Err(cur)
    }
}
