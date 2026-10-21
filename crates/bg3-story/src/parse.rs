//! Story file parser. Section order and field layout follow `COsiris::Load` in libOsiris.

use std::collections::HashMap;

use crate::model::*;
use crate::reader::Reader;
use crate::{Error, Result};

/// Oldest format we implement and have verified (BG3 ships 1.13 and 1.15).
pub const MIN_VERSION: u16 = 0x010d;
/// Newest format seen (BG3 4.1.1.7631656 ships 1.15).
pub const MAX_VERSION: u16 = 0x010f;

// bg3: COsiris::Load (4.1.1.7631656)
pub(crate) fn parse_story(data: &[u8]) -> Result<Story> {
    let mut r = Reader::new(data);
    let (header, types, enums) = read_header(&mut r)?;
    let type_index = TypeIndex::new(&types);
    let mut p = Parser {
        r,
        types: type_index,
        old_values: header.minor < 14,
    };

    // bg3: COsiris::_ReadDIVObjects (4.1.1.7631656)
    let objects = p.r.list("object count", |r| {
        Ok(DivObject {
            name: r.string("object name")?,
            type_id: r.u8("object type")?,
            keys: four_key(r)?,
        })
    })?;
    // bg3: COsiris::_ReadFunctions (4.1.1.7631656)
    let functions = p.r.list("function count", function)?;
    // bg3: COsiris::_ReadReteNodes (4.1.1.7631656)
    let nodes = p.list("node count", Parser::node)?;
    // bg3: COsiris::_ReadReteAdaptors (4.1.1.7631656)
    let adapters = p.list("adapter count", Parser::adapter)?;
    // bg3: COsiris::_ReadReteDBases (4.1.1.7631656)
    let databases = p.list("database count", Parser::database)?;
    // bg3: COsiris::_ReadGoalData (4.1.1.7631656)
    let goals = p.list("goal count", Parser::goal)?;
    // bg3: COsiris::_ReadGlobalActionList (4.1.1.7631656)
    let global_actions = p.actions()?;

    if p.r.remaining() != 0 {
        return Err(Error::TrailingBytes {
            offset: p.r.pos(),
            count: p.r.remaining(),
        });
    }

    let mut function_index = HashMap::new();
    for (i, f) in functions.iter().enumerate() {
        function_index
            .entry((f.signature.name.clone(), f.signature.arity()))
            .or_insert(i);
    }

    Ok(Story {
        header,
        types,
        enums,
        objects,
        functions,
        nodes: table(nodes, "node")?,
        adapters: table(adapters, "adapter")?,
        databases: table(databases, "database")?,
        goals: table(goals, "goal")?,
        global_actions,
        type_index: p.types,
        function_index,
    })
}

fn table<T: Keyed>(items: Vec<T>, what: &'static str) -> Result<Table<T>> {
    Table::from_items(items).map_err(|id| Error::DuplicateId { what, id })
}

type HeaderParts = (Header, Vec<TypeInfo>, Vec<EnumDef>);

// bg3: COsiSmartBuf::COsiSmartBuf (4.1.1.7631656) — first byte
// bg3: COsiris::_ReadHeader (4.1.1.7631656)
fn read_header(r: &mut Reader) -> Result<HeaderParts> {
    let buffer_flag = r.u8("buffer flag")?;
    let banner = r.string("banner")?;
    let major = r.u8("major version")?;
    let minor = r.u8("minor version")?;
    let version = (major as u16) << 8 | minor as u16;
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(Error::UnsupportedVersion { major, minor });
    }
    let big_endian = r.bool("endianness")?;
    if big_endian {
        return Err(Error::BigEndian);
    }
    let unused = r.u8("header padding")?;
    let mut story_version = [0u8; 0x80];
    story_version.copy_from_slice(r.bytes(0x80, "story version")?);
    let debug_flags = r.u32("debug flags")?;
    // From 1.4 on every string after the header is XOR-scrambled.
    r.scramble = 0xad;

    let types = r.list("type count", |r| {
        let name = r.string("type name")?;
        let index = r.u8("type index")?;
        let alias = r.u8("type alias")?;
        if index == 0 || index > 0x7f || alias > 0x7f {
            return Err(Error::Invalid {
                offset: r.pos() - 2,
                msg: format!("type {name}: index {index} / alias {alias} out of range"),
            });
        }
        Ok(TypeInfo { name, index, alias })
    })?;
    // bg3: COsiris::_ReadEnumElements (4.1.1.7631656)
    let enums = r.list("enum count", |r| {
        let type_id = r.u16("enum type")?;
        let elements = r.list("enum element count", |r| {
            Ok((r.string("enum label")?, r.u64("enum value")?))
        })?;
        Ok(EnumDef { type_id, elements })
    })?;

    Ok((
        Header {
            buffer_flag,
            banner,
            major,
            minor,
            big_endian,
            unused,
            story_version,
            debug_flags,
        },
        types,
        enums,
    ))
}

// bg3: CFourKey::Read (4.1.1.7631656)
fn four_key(r: &mut Reader) -> Result<[u32; 4]> {
    Ok([r.u32("key")?, r.u32("key")?, r.u32("key")?, r.u32("key")?])
}

// bg3: COsiFunctionData::COsiFunctionData(COsiSmartBuf*) (4.1.1.7631656)
fn function(r: &mut Reader) -> Result<Function> {
    let line = r.u32("function line")?;
    let condition_refs = r.u32("function condition refs")?;
    let action_refs = r.u32("function action refs")?;
    let node = NodeId::from_raw(r.u32("function node")?);
    let kind = FunctionKind::from_raw(r.u8("function kind")?);
    let keys = four_key(r)?;
    // bg3: COsiFunctionDef::COsiFunctionDef(COsiSmartBuf*) (4.1.1.7631656)
    let name = r.string("function name")?;
    // bg3: COsiBitVector::COsiBitVector(COsiSmartBuf*) (4.1.1.7631656)
    let mask_len = r.u32("out param mask length")? as usize;
    let out_param_mask = r.bytes(mask_len, "out param mask")?.to_vec();
    let params = type_list(r)?;
    Ok(Function {
        line,
        condition_refs,
        action_refs,
        node,
        kind,
        keys,
        signature: Signature {
            name,
            out_param_mask,
            params,
        },
    })
}

// bg3: COsiValueTypeList::Read (4.1.1.7631656)
fn type_list(r: &mut Reader) -> Result<Vec<TypeId>> {
    r.short_list("type list", |r| r.u16("param type"))
}

// bg3: CReteConnection::Read (4.1.1.7631656)
fn connection(r: &mut Reader) -> Result<Connection> {
    Ok(Connection {
        node: NodeId::from_raw(r.u32("connection node")?),
        entry: EntryPoint::from_raw(r.u32("connection entry point")?),
        goal: GoalId::from_raw(r.u32("connection goal")?),
    })
}

fn db_link(r: &mut Reader) -> Result<DbLink> {
    Ok(DbLink {
        node: NodeId::from_raw(r.u32("db node")?),
        connection: connection(r)?,
        indirection: r.u8("db indirection")?,
    })
}

fn read_id<T>(r: &mut Reader, what: &'static str, f: impl Fn(u32) -> Option<T>) -> Result<T> {
    let offset = r.pos();
    f(r.u32(what)?).ok_or(Error::Invalid {
        offset,
        msg: format!("{what}: id 0"),
    })
}

struct Parser<'a> {
    r: Reader<'a>,
    types: TypeIndex,
    /// Format < 1.14: values without the leading index/flags word.
    old_values: bool,
}

impl Parser<'_> {
    fn list<T>(
        &mut self,
        what: &'static str,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let n = self.r.u32(what)? as usize;
        if n > self.r.remaining() {
            return Err(Error::Invalid {
                offset: self.r.pos() - 4,
                msg: format!("{what}: count {n} exceeds remaining bytes"),
            });
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(item(self)?);
        }
        Ok(out)
    }

    /// Read a typed value. `spec` is the specialisation the caller constructs (1 = value,
    /// 2 = typed parameter, 3 = variable); from format 1.14 on the file stores it itself.
    // bg3: COsiTypedValueBase::COsiTypedValueBase(TOsiValueSpecializationType, COsiSmartBuf*) (4.1.1.7631656)
    fn value(&mut self, spec: u8) -> Result<Value> {
        if self.old_values {
            return self.value_pre_1_14(spec);
        }
        let index = self.r.i8("value index")?;
        let flags = self.r.u8("value flags")?;
        let constant = if flags & value_flags::VALID != 0 {
            Some(self.constant()?)
        } else {
            None
        };
        Ok(Value {
            index,
            flags,
            constant,
        })
    }

    /// Formats before 1.14: the constant always comes first, then per-specialisation flag bytes.
    fn value_pre_1_14(&mut self, spec: u8) -> Result<Value> {
        let constant = Some(self.constant()?);
        let r = &mut self.r;
        let mut flags = spec;
        let mut index = -1;
        if spec & 6 == 2 {
            flags |= (r.u8("valid flag")? & 1) << 3;
            flags |= (r.u8("out param flag")? & 1) << 4;
            flags |= (r.u8("type marker flag")? & 1) << 5;
        }
        if spec == value_flags::SPEC_VARIABLE {
            index = r.i8("variable index")?;
            flags |= (r.u8("unused flag")? & 1) << 6;
            flags |= (r.u8("adapted flag")? & 1) << 7;
        }
        Ok(Value {
            index,
            flags,
            constant,
        })
    }

    fn constant(&mut self) -> Result<Constant> {
        let r = &mut self.r;
        let marker_at = r.pos();
        let marker = r.u8("value marker")?;
        let type_id = r.u16("value type")?;
        let data = match marker {
            b'0' => match self.types.root(type_id) {
                Ok(BuiltinType::None) => ConstantData::None,
                Ok(BuiltinType::Integer) => ConstantData::Integer(r.i32("int value")?),
                Ok(BuiltinType::Integer64) => ConstantData::Integer64(r.u64("int64 value")? as i64),
                Ok(BuiltinType::Real) => ConstantData::Real(r.f32("real value")?),
                Ok(BuiltinType::String) => ConstantData::String(if r.bool("string present")? {
                    Some(r.string("string value")?)
                } else {
                    None
                }),
                Ok(BuiltinType::GuidString) => {
                    // bg3: COsiSmartBuf::readGuidString (4.1.1.7631656)
                    ConstantData::GuidString(if r.bool("guid present")? {
                        Some(r.string("guid value")?)
                    } else {
                        None
                    })
                }
                Err(_) => ConstantData::Object(r.string("object value")?),
            },
            b'e' => ConstantData::Enum(r.string("enum label")?),
            b'1' => ConstantData::Raw(r.i32("raw value")?),
            m => {
                return Err(Error::Invalid {
                    offset: marker_at,
                    msg: format!("invalid value marker 0x{m:02x}"),
                });
            }
        };
        Ok(Constant { type_id, data })
    }

    // bg3: CReteNodeFactory::CreateReteFact etc. (4.1.1.7631656); query nodes (CReteDIVQuery,
    // CReteInternalQuery, CReteOsiQuery) are plain CReteNodes without extra fields.
    fn node(&mut self) -> Result<Node> {
        let at = self.r.pos();
        let ty = self.r.u8("node type")?;
        let id = read_id(&mut self.r, "node id", NodeId::from_raw)?;
        // bg3: CReteNode::CReteNode(COsiSmartBuf*) (4.1.1.7631656)
        let database = DatabaseId::from_raw(self.r.u32("node database")?);
        let name = self.r.string("node name")?;
        let arity = if name.is_empty() {
            0
        } else {
            self.r.u8("node arity")?
        };
        let kind = match ty {
            1 => NodeKind::Database(self.start()?),
            2 => NodeKind::Event(self.start()?),
            3 => NodeKind::DivQuery,
            4 => NodeKind::And(self.join()?),
            5 => NodeKind::NotAnd(self.join()?),
            6 => NodeKind::Compare(self.compare()?),
            7 => NodeKind::Rule(self.rule()?),
            8 => NodeKind::InternalQuery,
            9 => NodeKind::UserQuery,
            t => {
                return Err(Error::Invalid {
                    offset: at,
                    msg: format!("invalid rete node type {t}"),
                });
            }
        };
        Ok(Node {
            id,
            database,
            name,
            arity,
            kind,
        })
    }

    // bg3: CReteStartNode::CReteStartNode(COsiSmartBuf*) (4.1.1.7631656)
    fn start(&mut self) -> Result<StartNode> {
        Ok(StartNode {
            successors: self.r.list("start node successor count", connection)?,
        })
    }

    // bg3: CReteBinaryNode::CReteBinaryNode(COsiSmartBuf*) (4.1.1.7631656)
    fn join(&mut self) -> Result<JoinNode> {
        let r = &mut self.r;
        Ok(JoinNode {
            next: connection(r)?,
            left_parent: NodeId::from_raw(r.u32("left parent")?),
            right_parent: NodeId::from_raw(r.u32("right parent")?),
            left_adapter: AdapterId::from_raw(r.u32("left adapter")?),
            right_adapter: AdapterId::from_raw(r.u32("right adapter")?),
            left_db: db_link(r)?,
            right_db: db_link(r)?,
        })
    }

    // bg3: CReteUnaryNode::CReteUnaryNode(COsiSmartBuf*) (4.1.1.7631656)
    fn unary(&mut self) -> Result<UnaryNode> {
        let r = &mut self.r;
        Ok(UnaryNode {
            next: connection(r)?,
            parent: NodeId::from_raw(r.u32("parent")?),
            adapter: AdapterId::from_raw(r.u32("adapter")?),
            db: db_link(r)?,
        })
    }

    // bg3: CReteRelCondition::CReteRelCondition(COsiSmartBuf*) (4.1.1.7631656)
    fn compare(&mut self) -> Result<CompareNode> {
        let unary = self.unary()?;
        let left_var = self.r.i8("left variable")?;
        let right_var = self.r.i8("right variable")?;
        let left_value = self.value(value_flags::SPEC_VALUE)?;
        let right_value = self.value(value_flags::SPEC_VALUE)?;
        let op = RelOp::from_raw(self.r.u32("relop")?);
        Ok(CompareNode {
            unary,
            left_var,
            right_var,
            left_value,
            right_value,
            op,
        })
    }

    // bg3: CReteRuleActionPart::CReteRuleActionPart(COsiSmartBuf*) (4.1.1.7631656)
    fn rule(&mut self) -> Result<RuleNode> {
        let unary = self.unary()?;
        let actions = self.actions()?;
        // bg3: COsiVariableVector::Read (4.1.1.7631656)
        let n = self.r.u8("variable count")?;
        let mut variables = Vec::with_capacity(n as usize);
        for _ in 0..n {
            if self.old_values {
                self.r.u8("variable kind")?;
            }
            variables.push(self.value(value_flags::SPEC_VARIABLE)?);
        }
        let line = self.r.u32("rule line")?;
        let is_query = self.r.bool("rule is query")?;
        Ok(RuleNode {
            unary,
            actions,
            variables,
            line,
            is_query,
        })
    }

    // bg3: CpOsiFunctionParseDataList::Read (4.1.1.7631656)
    fn actions(&mut self) -> Result<Vec<Action>> {
        self.list("action count", Parser::action)
    }

    // bg3: COsiFunctionParseData::COsiFunctionParseData(COsiSmartBuf*) (4.1.1.7631656)
    fn action(&mut self) -> Result<Action> {
        let name = self.r.string("action name")?;
        let mut params = None;
        let mut negate = false;
        if !name.is_empty() {
            if self.r.bool("action has params")? {
                // bg3: COsipParameterList::Read (4.1.1.7631656)
                let n = self.r.u8("action param count")?;
                let mut list = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let spec = if self.old_values {
                        match self.r.u8("param kind")? {
                            0 => value_flags::SPEC_TYPED,
                            1 => value_flags::SPEC_VARIABLE,
                            _ => value_flags::SPEC_VALUE,
                        }
                    } else {
                        value_flags::SPEC_VALUE
                    };
                    list.push(self.value(spec)?);
                }
                params = Some(list);
            }
            negate = self.r.bool("action negate")?;
        }
        let goal_or_hook = self.r.i32("action goal/hook")?;
        Ok(Action {
            name,
            params,
            negate,
            goal_or_hook,
        })
    }

    // bg3: CReteAdaptorFactory::CreateReteAdaptor / CReteAdaptor::CReteAdaptor (4.1.1.7631656)
    fn adapter(&mut self) -> Result<Adapter> {
        let id = read_id(&mut self.r, "adapter id", AdapterId::from_raw)?;
        // bg3: COsiColumnIdxValuePairList::Read (4.1.1.7631656)
        let n = self.r.u8("adapter constant count")?;
        let mut constants = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let column = if self.old_values {
                Some(self.r.i8("constant column")?)
            } else {
                None
            };
            let mut v = self.value(value_flags::SPEC_VALUE)?;
            if let Some(c) = column {
                v.index = c;
            }
            constants.push(v);
        }
        let logical_indices = self
            .r
            .short_list("adapter logical indices", |r| r.i8("logical index"))?;
        let logical_to_physical = self.r.short_list("adapter column map", |r| {
            Ok((r.u8("logical column")?, r.u8("physical column")?))
        })?;
        Ok(Adapter {
            id,
            constants,
            logical_indices,
            logical_to_physical,
        })
    }

    // bg3: CReteDBaseFactory::CreateReteDBase / CReteDBase::CReteDBase (4.1.1.7631656)
    fn database(&mut self) -> Result<Database> {
        let id = read_id(&mut self.r, "database id", DatabaseId::from_raw)?;
        let param_types = type_list(&mut self.r)?;
        let facts = self.list("fact count", |p| {
            // bg3: CTuple::Read (4.1.1.7631656)
            let n = p.r.u8("fact column count")?;
            let mut row = Vec::with_capacity(n as usize);
            for _ in 0..n {
                row.push(p.value(value_flags::SPEC_VALUE)?);
            }
            Ok(row)
        })?;
        Ok(Database {
            id,
            param_types,
            facts,
        })
    }

    // bg3: COsiGoal::COsiGoal(COsiSmartBuf*) (4.1.1.7631656)
    fn goal(&mut self) -> Result<Goal> {
        let id = read_id(&mut self.r, "goal id", GoalId::from_raw)?;
        let name = self.r.string("goal name")?;
        let combiner = SubGoalCombiner::from_raw(self.r.u8("goal combiner")?);
        let parents = self.r.list("goal parent count", |r| {
            read_id(r, "parent goal", GoalId::from_raw)
        })?;
        let subgoals = self.r.list("goal subgoal count", |r| {
            read_id(r, "subgoal", GoalId::from_raw)
        })?;
        let flags = self.r.u8("goal flags")?;
        let init_actions = self.actions()?;
        let exit_actions = self.actions()?;
        Ok(Goal {
            id,
            name,
            combiner,
            parents,
            subgoals,
            flags,
            init_actions,
            exit_actions,
        })
    }
}
