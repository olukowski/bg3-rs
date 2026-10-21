//! Loader for compiled Osiris story files (`story.div.osi`) as used by Baldur's Gate 3.
//!
//! A compiled story is the persisted state of the Osiris rule engine: type table, function
//! signatures, the rete network (start/join/compare/rule nodes and the adaptors between them),
//! databases with their facts, and the goal tree. The same format is embedded in save games.
//!
//! The format write-up is in `docs/formats/osiris-story.md`; how the network is evaluated is
//! described in `docs/research/osiris-vm.md`. Layout verified against libOsiris
//! (`COsiris::Load` and the `COsiSmartBuf` constructors it calls) of BG3 4.1.1.7631656, and
//! by loading every shipped story with zero leftover bytes.
//!
//! ```no_run
//! let bytes = std::fs::read("story.div.osi").unwrap();
//! let story = bg3_story::Story::parse(&bytes).unwrap();
//! println!("{} goals, {} nodes", story.goals.len(), story.nodes.len());
//! print!("{}", story.dump_goal(story.goal_by_name("GLO_Main").unwrap()));
//! ```

mod dump;
mod model;
mod parse;
mod reader;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod writer;

pub use model::*;
pub use parse::{MAX_VERSION, MIN_VERSION};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of file at 0x{offset:x} while reading {what}")]
    Eof { offset: usize, what: &'static str },
    #[error("invalid story data at 0x{offset:x}: {msg}")]
    Invalid { offset: usize, msg: String },
    #[error("unsupported story format {major}.{minor} (supported: 1.13-1.15)")]
    UnsupportedVersion { major: u8, minor: u8 },
    #[error("big-endian story files are not supported")]
    BigEndian,
    #[error("duplicate {what} id {id}")]
    DuplicateId { what: &'static str, id: u32 },
    #[error("{count} unread bytes after the last section (at 0x{offset:x})")]
    TrailingBytes { offset: usize, count: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Story {
    /// Parse a complete story file. Fails unless every byte is consumed.
    pub fn parse(data: &[u8]) -> Result<Self> {
        parse::parse_story(data)
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn adapter(&self, id: AdapterId) -> Option<&Adapter> {
        self.adapters.get(id)
    }

    pub fn database(&self, id: DatabaseId) -> Option<&Database> {
        self.databases.get(id)
    }

    pub fn goal(&self, id: GoalId) -> Option<&Goal> {
        self.goals.get(id)
    }

    pub fn goal_by_name(&self, name: &str) -> Option<&Goal> {
        self.goals.iter().find(|g| g.name == name)
    }

    /// Look up a function by name and arity (Osiris allows overloading by arity).
    pub fn function(&self, name: &str, arity: u8) -> Option<&Function> {
        self.function_index
            .get(&(name.to_owned(), arity))
            .map(|&i| &self.functions[i])
    }

    /// The function a start node belongs to.
    pub fn node_function(&self, node: &Node) -> Option<&Function> {
        if node.name.is_empty() {
            None
        } else {
            self.function(&node.name, node.arity)
        }
    }

    pub fn type_info(&self, id: TypeId) -> Option<&TypeInfo> {
        self.types.iter().find(|t| t.index as TypeId == id)
    }

    /// Display name of a type id (built-in, alias or enum).
    pub fn type_name(&self, id: TypeId) -> String {
        if let Some(b) = BuiltinType::from_id(id) {
            return b.name().to_owned();
        }
        match self.type_info(id) {
            Some(t) => t.name.clone(),
            None => format!("TYPE_{id}"),
        }
    }

    /// Root built-in type of `id`, following aliases; `None` for object/enum types.
    pub fn root_type(&self, id: TypeId) -> Option<BuiltinType> {
        self.type_index.root(id).ok()
    }

    pub fn enum_def(&self, type_id: TypeId) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.type_id == type_id)
    }

    /// Goal owning each node, derived from the goal ids on incoming connections. Indexed by
    /// raw node id (slot 0 unused).
    pub fn node_goals(&self) -> Vec<Option<GoalId>> {
        let mut out = vec![None; self.nodes.id_bound()];
        for node in &self.nodes {
            for c in node.kind.successors() {
                if let (Some(n), Some(g)) = (c.node, c.goal) {
                    out[n.get() as usize] = Some(g);
                }
            }
        }
        out
    }

    /// Basic referential checks: every node/adapter/database/goal id referenced anywhere
    /// exists. Returns human-readable problems (empty = consistent).
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let node = |id: Option<NodeId>, ctx: &dyn Fn() -> String, p: &mut Vec<String>| {
            if let Some(id) = id
                && self.nodes.get(id).is_none()
            {
                p.push(format!("{}: missing node {id}", ctx()));
            }
        };
        let goal = |id: Option<GoalId>, ctx: &dyn Fn() -> String, p: &mut Vec<String>| {
            if let Some(id) = id
                && self.goals.get(id).is_none()
            {
                p.push(format!("{}: missing goal {id}", ctx()));
            }
        };
        let adapter = |id: Option<AdapterId>, ctx: &dyn Fn() -> String, p: &mut Vec<String>| {
            if let Some(id) = id
                && self.adapters.get(id).is_none()
            {
                p.push(format!("{}: missing adapter {id}", ctx()));
            }
        };
        let conn = |c: &Connection, ctx: &dyn Fn() -> String, p: &mut Vec<String>| {
            node(c.node, ctx, p);
            goal(c.goal, ctx, p);
        };
        for f in &self.functions {
            let ctx = || format!("function {}", f.signature.name);
            node(f.node, &ctx, &mut problems);
        }
        for n in &self.nodes {
            let ctx = || format!("node {}", n.id);
            if let Some(db) = n.database
                && self.databases.get(db).is_none()
            {
                problems.push(format!("{}: missing database {db}", ctx()));
            }
            for c in n.kind.successors() {
                conn(c, &ctx, &mut problems);
            }
            match &n.kind {
                NodeKind::And(j) | NodeKind::NotAnd(j) => {
                    node(j.left_parent, &ctx, &mut problems);
                    node(j.right_parent, &ctx, &mut problems);
                    adapter(j.left_adapter, &ctx, &mut problems);
                    adapter(j.right_adapter, &ctx, &mut problems);
                    for db in [&j.left_db, &j.right_db] {
                        node(db.node, &ctx, &mut problems);
                        conn(&db.connection, &ctx, &mut problems);
                    }
                }
                NodeKind::Compare(_) | NodeKind::Rule(_) => {
                    let u = n.kind.unary().expect("unary");
                    node(u.parent, &ctx, &mut problems);
                    adapter(u.adapter, &ctx, &mut problems);
                    node(u.db.node, &ctx, &mut problems);
                    conn(&u.db.connection, &ctx, &mut problems);
                }
                _ => {}
            }
        }
        for g in &self.goals {
            let ctx = || format!("goal {}", g.name);
            for &p in g.parents.iter().chain(&g.subgoals) {
                goal(Some(p), &ctx, &mut problems);
            }
        }
        problems
    }
}
