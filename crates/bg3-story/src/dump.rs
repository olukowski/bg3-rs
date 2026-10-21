//! Human-readable dump of a story as pseudo-Osiris source, for debugging.
//!
//! Rules are reconstructed by walking from each rule node up through its parents: start nodes
//! become conditions (`DB_Foo(_Var0, 1)`), `NotAnd` right sides become `NOT ...`, compare nodes
//! become `_Var0 > 5`. Variable names are synthetic (`_Var<n>`, `n` = logical column), since
//! compiled stories do not keep the source names.

use std::fmt::{self, Write as _};

use crate::model::*;

impl Story {
    /// One-line-per-section summary.
    pub fn dump_summary(&self) -> String {
        let rules = self
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Rule(_)))
            .count();
        let facts: usize = self.databases.iter().map(|d| d.facts.len()).sum();
        let mut s = String::new();
        let h = &self.header;
        let _ = writeln!(s, "// {}", h.banner);
        let _ = writeln!(
            s,
            "// format {}.{}, story version {}, debug flags 0x{:08x}",
            h.major,
            h.minor,
            h.story_version_str(),
            h.debug_flags
        );
        let _ = writeln!(
            s,
            "// {} types, {} enums, {} objects, {} functions, {} nodes ({rules} rules), \
             {} adapters, {} databases ({facts} facts), {} goals, {} global actions",
            self.types.len(),
            self.enums.len(),
            self.objects.len(),
            self.functions.len(),
            self.nodes.len(),
            self.adapters.len(),
            self.databases.len(),
            self.goals.len(),
            self.global_actions.len()
        );
        s
    }

    /// Signature line, e.g. `query QRY_Foo([in](GUIDSTRING)_Arg0, [out](INTEGER)_Arg1)`.
    pub fn dump_function(&self, f: &Function) -> String {
        let sig = &f.signature;
        let params: Vec<String> = sig
            .params
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let dir = if sig.is_out(i) { "[out]" } else { "[in]" };
                format!("{dir}({})_Arg{i}", self.type_name(t))
            })
            .collect();
        format!("{} {}({})", f.kind.keyword(), sig.name, params.join(", "))
    }

    /// Format a constant as Osiris source (`(TYPE)` prefix for alias types).
    pub fn dump_constant(&self, c: &Constant) -> String {
        let body = match &c.data {
            ConstantData::None => "NULL".to_owned(),
            ConstantData::Integer(v) | ConstantData::Raw(v) => v.to_string(),
            ConstantData::Integer64(v) => v.to_string(),
            ConstantData::Real(v) => format!("{v:?}"),
            ConstantData::String(Some(s)) => format!("\"{s}\""),
            ConstantData::GuidString(Some(s)) | ConstantData::Object(s) | ConstantData::Enum(s) => {
                s.clone()
            }
            ConstantData::String(None) | ConstantData::GuidString(None) => "NULL".to_owned(),
        };
        if BuiltinType::from_id(c.type_id).is_some() {
            body
        } else {
            format!("({}){body}", self.type_name(c.type_id))
        }
    }

    fn dump_value(&self, v: &Value) -> String {
        if v.is_variable() {
            if v.is_unused() {
                return "_".to_owned();
            }
            return format!("_Var{}", v.index);
        }
        match &v.constant {
            Some(c) => self.dump_constant(c),
            None => "_".to_owned(),
        }
    }

    /// `Name(args)`, `NOT DB_Foo(...)` or `GoalCompleted`.
    pub fn dump_action(&self, a: &Action) -> String {
        if a.name.is_empty() {
            return "GoalCompleted".to_owned();
        }
        if a.goal_or_hook < 0 {
            return format!("/* debug hook {} */", -a.goal_or_hook);
        }
        let args: Vec<String> = a
            .params
            .iter()
            .flatten()
            .map(|p| self.dump_value(p))
            .collect();
        let not = if a.negate { "NOT " } else { "" };
        format!("{not}{}({})", a.name, args.join(", "))
    }

    /// One rule as `IF/PROC/QRY ... THEN ...;`. `None` if `id` is not a rule node.
    pub fn dump_rule(&self, id: NodeId) -> Option<String> {
        let node = self.node(id)?;
        let NodeKind::Rule(rule) = &node.kind else {
            return None;
        };
        let mut conds = Vec::new();
        self.conditions(rule.unary.parent, rule.unary.adapter, false, &mut conds);
        let head = if rule.is_query {
            "QRY"
        } else if self
            .first_start_node(rule.unary.parent)
            .and_then(|n| self.node_function(n))
            .is_some_and(|f| f.kind == FunctionKind::Proc)
        {
            "PROC"
        } else {
            "IF"
        };
        let mut s = String::new();
        let line = if rule.line == u32::MAX {
            String::new()
        } else {
            format!(" line {}", rule.line)
        };
        let _ = writeln!(s, "// rule {id}{line}");
        let _ = writeln!(s, "{head}");
        let _ = writeln!(s, "{}", conds.join("\nAND\n"));
        let _ = writeln!(s, "THEN");
        for a in &rule.actions {
            let _ = writeln!(s, "{};", self.dump_action(a));
        }
        Some(s)
    }

    fn first_start_node(&self, mut id: Option<NodeId>) -> Option<&Node> {
        while let Some(n) = id.and_then(|i| self.node(i)) {
            match &n.kind {
                NodeKind::And(j) | NodeKind::NotAnd(j) => id = j.left_parent,
                NodeKind::Compare(c) => id = c.unary.parent,
                NodeKind::Rule(r) => id = r.unary.parent,
                _ => return Some(n),
            }
        }
        None
    }

    fn conditions(
        &self,
        id: Option<NodeId>,
        adapter: Option<AdapterId>,
        negate: bool,
        out: &mut Vec<String>,
    ) {
        let Some(node) = id.and_then(|i| self.node(i)) else {
            out.push("/* missing node */".to_owned());
            return;
        };
        match &node.kind {
            NodeKind::And(j) | NodeKind::NotAnd(j) => {
                self.conditions(j.left_parent, j.left_adapter, negate, out);
                let not = matches!(node.kind, NodeKind::NotAnd(_));
                self.conditions(j.right_parent, j.right_adapter, negate ^ not, out);
            }
            NodeKind::Compare(c) => {
                self.conditions(c.unary.parent, c.unary.adapter, negate, out);
                let side = |var: i8, v: &Value| {
                    if var >= 0 {
                        format!("_Var{var}")
                    } else {
                        self.dump_value(v)
                    }
                };
                out.push(format!(
                    "{}{} {} {}",
                    if negate { "NOT " } else { "" },
                    side(c.left_var, &c.left_value),
                    c.op.symbol(),
                    side(c.right_var, &c.right_value)
                ));
            }
            NodeKind::Rule(_) => out.push("/* unexpected rule parent */".to_owned()),
            _ => {
                let adapter = adapter.and_then(|a| self.adapter(a));
                let args: Vec<String> = (0..node.arity as usize)
                    .map(|col| self.adapted_arg(adapter, col))
                    .collect();
                let not = if negate { "NOT " } else { "" };
                out.push(format!("{not}{}({})", node.name, args.join(", ")));
            }
        }
    }

    /// Argument text for physical column `col` of a start node seen through `adapter`.
    fn adapted_arg(&self, adapter: Option<&Adapter>, col: usize) -> String {
        let Some(a) = adapter else {
            return "_".to_owned();
        };
        if let Some(&li) = a.logical_indices.get(col)
            && li >= 0
        {
            return format!("_Var{li}");
        }
        match a.constants.iter().find(|c| c.index as usize == col) {
            Some(c) => self.dump_value(c),
            None => "_".to_owned(),
        }
    }

    /// A goal as it would appear in a goal source file.
    pub fn dump_goal(&self, goal: &Goal) -> String {
        let node_goals = self.node_goals();
        self.dump_goal_with(goal, &node_goals)
    }

    fn dump_goal_with(&self, goal: &Goal, node_goals: &[Option<GoalId>]) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "// goal {} \"{}\" flags 0x{:02x}",
            goal.id, goal.name, goal.flags
        );
        let _ = writeln!(s, "Version 1");
        let combiner = match goal.combiner {
            SubGoalCombiner::Or => "SGC_OR".to_owned(),
            SubGoalCombiner::And => "SGC_AND".to_owned(),
            SubGoalCombiner::Unknown(v) => format!("SGC_{v}"),
        };
        let _ = writeln!(s, "SubGoalCombiner {combiner}");
        let _ = writeln!(s, "INITSECTION");
        for a in &goal.init_actions {
            let _ = writeln!(s, "{};", self.dump_action(a));
        }
        let _ = writeln!(s, "KBSECTION");
        let mut rules: Vec<(u32, NodeId)> = self
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Rule(r) if node_goals[n.id.get() as usize] == Some(goal.id) => {
                    Some((r.line, n.id))
                }
                _ => None,
            })
            .collect();
        rules.sort();
        for (_, id) in rules {
            if let Some(r) = self.dump_rule(id) {
                let _ = writeln!(s, "{r}");
            }
        }
        let _ = writeln!(s, "EXITSECTION");
        for a in &goal.exit_actions {
            let _ = writeln!(s, "{};", self.dump_action(a));
        }
        let _ = writeln!(s, "ENDEXITSECTION");
        for p in &goal.parents {
            let name = self.goal(*p).map_or("?", |g| g.name.as_str());
            let _ = writeln!(s, "ParentTargetEdge \"{name}\"");
        }
        s
    }

    /// Name of each database (from its start node), indexed by raw database id.
    pub fn database_names(&self) -> Vec<Option<&str>> {
        let mut out = vec![None; self.databases.id_bound()];
        for n in &self.nodes {
            if let (NodeKind::Database(_), Some(db)) = (&n.kind, n.database)
                && let Some(slot) = out.get_mut(db.get() as usize)
            {
                *slot = Some(n.name.as_str());
            }
        }
        out
    }
}

/// Full dump: summary, types, enums, functions, databases with facts, global actions, goals.
impl fmt::Display for Story {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dump_summary())?;
        writeln!(f, "\n// ---- types")?;
        for t in &self.types {
            let alias = if t.alias == 0 {
                String::new()
            } else {
                format!(" alias of {}", self.type_name(t.alias as TypeId))
            };
            writeln!(f, "type {} = {}{alias}", t.index, t.name)?;
        }
        writeln!(f, "\n// ---- enums")?;
        for e in &self.enums {
            writeln!(f, "enum {}", self.type_name(e.type_id))?;
            for (label, v) in &e.elements {
                writeln!(f, "    {label} = {v}")?;
            }
        }
        if !self.objects.is_empty() {
            writeln!(f, "\n// ---- objects")?;
            for o in &self.objects {
                writeln!(
                    f,
                    "object {} : {}",
                    o.name,
                    self.type_name(o.type_id as TypeId)
                )?;
            }
        }
        writeln!(f, "\n// ---- functions")?;
        for func in &self.functions {
            writeln!(f, "{}", self.dump_function(func))?;
        }
        writeln!(f, "\n// ---- databases")?;
        let names = self.database_names();
        for db in &self.databases {
            let name = names
                .get(db.id.get() as usize)
                .copied()
                .flatten()
                .unwrap_or("?");
            let types: Vec<String> = db.param_types.iter().map(|&t| self.type_name(t)).collect();
            writeln!(
                f,
                "// db {} {name}({}) — {} facts",
                db.id,
                types.join(", "),
                db.facts.len()
            )?;
            for row in &db.facts {
                let vals: Vec<String> = row.iter().map(|v| self.dump_value(v)).collect();
                writeln!(f, "{name}({});", vals.join(", "))?;
            }
        }
        if !self.global_actions.is_empty() {
            writeln!(f, "\n// ---- global actions")?;
            for a in &self.global_actions {
                writeln!(f, "{};", self.dump_action(a))?;
            }
        }
        writeln!(f, "\n// ---- goals")?;
        let node_goals = self.node_goals();
        for goal in &self.goals {
            writeln!(f, "{}", self.dump_goal_with(goal, &node_goals))?;
        }
        Ok(())
    }
}
