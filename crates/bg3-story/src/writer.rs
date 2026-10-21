//! Test-only serialiser: the exact inverse of `parse.rs`, used to round-trip synthetic stories
//! and to check that parsing the shipped stories loses no information.

use crate::model::*;

pub(crate) fn write_story(story: &Story) -> Vec<u8> {
    let mut w = Writer {
        out: Vec::new(),
        key: 0,
        old_values: story.header.minor < 14,
    };
    let h = &story.header;
    w.u8(h.buffer_flag);
    w.string(&h.banner);
    w.u8(h.major);
    w.u8(h.minor);
    w.u8(h.big_endian as u8);
    w.u8(h.unused);
    w.out.extend_from_slice(&h.story_version);
    w.u32(h.debug_flags);
    w.key = 0xad;

    w.u32(story.types.len() as u32);
    for t in &story.types {
        w.string(&t.name);
        w.u8(t.index);
        w.u8(t.alias);
    }
    w.u32(story.enums.len() as u32);
    for e in &story.enums {
        w.u16(e.type_id);
        w.u32(e.elements.len() as u32);
        for (label, value) in &e.elements {
            w.string(label);
            w.out.extend_from_slice(&value.to_le_bytes());
        }
    }

    w.u32(story.objects.len() as u32);
    for o in &story.objects {
        w.string(&o.name);
        w.u8(o.type_id);
        w.keys(&o.keys);
    }

    w.u32(story.functions.len() as u32);
    for f in &story.functions {
        w.u32(f.line);
        w.u32(f.condition_refs);
        w.u32(f.action_refs);
        w.u32(raw(f.node));
        w.u8(f.kind.to_raw());
        w.keys(&f.keys);
        w.string(&f.signature.name);
        w.u32(f.signature.out_param_mask.len() as u32);
        w.out.extend_from_slice(&f.signature.out_param_mask);
        w.types(&f.signature.params);
    }

    w.u32(story.nodes.len() as u32);
    for n in &story.nodes {
        w.node(n);
    }

    w.u32(story.adapters.len() as u32);
    for a in &story.adapters {
        w.u32(a.id.get());
        w.u8(a.constants.len() as u8);
        for c in &a.constants {
            if w.old_values {
                w.u8(c.index as u8);
            }
            w.value(c);
        }
        w.u8(a.logical_indices.len() as u8);
        for &i in &a.logical_indices {
            w.u8(i as u8);
        }
        w.u8(a.logical_to_physical.len() as u8);
        for &(l, p) in &a.logical_to_physical {
            w.u8(l);
            w.u8(p);
        }
    }

    w.u32(story.databases.len() as u32);
    for d in &story.databases {
        w.u32(d.id.get());
        w.types(&d.param_types);
        w.u32(d.facts.len() as u32);
        for row in &d.facts {
            w.u8(row.len() as u8);
            for v in row {
                w.value(v);
            }
        }
    }

    w.u32(story.goals.len() as u32);
    for g in &story.goals {
        w.u32(g.id.get());
        w.string(&g.name);
        w.u8(g.combiner.to_raw());
        for list in [&g.parents, &g.subgoals] {
            w.u32(list.len() as u32);
            for p in list {
                w.u32(p.get());
            }
        }
        w.u8(g.flags);
        w.actions(&g.init_actions);
        w.actions(&g.exit_actions);
    }

    w.actions(&story.global_actions);
    w.out
}

struct Writer {
    out: Vec<u8>,
    key: u8,
    old_values: bool,
}

impl Writer {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn string(&mut self, s: &str) {
        let key = self.key;
        self.out.extend(s.bytes().map(|b| b ^ key));
        self.out.push(key);
    }

    fn keys(&mut self, k: &[u32; 4]) {
        for &v in k {
            self.u32(v);
        }
    }

    fn types(&mut self, t: &[TypeId]) {
        self.u8(t.len() as u8);
        for &v in t {
            self.u16(v);
        }
    }

    fn connection(&mut self, c: &Connection) {
        self.u32(raw(c.node));
        self.u32(c.entry.to_raw());
        self.u32(raw(c.goal));
    }

    fn db_link(&mut self, d: &DbLink) {
        self.u32(raw(d.node));
        self.connection(&d.connection);
        self.u8(d.indirection);
    }

    fn unary(&mut self, u: &UnaryNode) {
        self.connection(&u.next);
        self.u32(raw(u.parent));
        self.u32(raw(u.adapter));
        self.db_link(&u.db);
    }

    fn node(&mut self, n: &Node) {
        self.u8(n.kind.type_byte());
        self.u32(n.id.get());
        self.u32(raw(n.database));
        self.string(&n.name);
        if !n.name.is_empty() {
            self.u8(n.arity);
        }
        match &n.kind {
            NodeKind::Database(s) | NodeKind::Event(s) => {
                self.u32(s.successors.len() as u32);
                for c in &s.successors {
                    self.connection(c);
                }
            }
            NodeKind::DivQuery | NodeKind::InternalQuery | NodeKind::UserQuery => {}
            NodeKind::And(j) | NodeKind::NotAnd(j) => {
                self.connection(&j.next);
                self.u32(raw(j.left_parent));
                self.u32(raw(j.right_parent));
                self.u32(raw(j.left_adapter));
                self.u32(raw(j.right_adapter));
                self.db_link(&j.left_db);
                self.db_link(&j.right_db);
            }
            NodeKind::Compare(c) => {
                self.unary(&c.unary);
                self.u8(c.left_var as u8);
                self.u8(c.right_var as u8);
                self.value(&c.left_value);
                self.value(&c.right_value);
                self.u32(c.op.to_raw());
            }
            NodeKind::Rule(r) => {
                self.unary(&r.unary);
                self.actions(&r.actions);
                self.u8(r.variables.len() as u8);
                for v in &r.variables {
                    if self.old_values {
                        // Parameter-kind byte, always "variable" for rule variables.
                        self.u8(1);
                    }
                    self.value(v);
                }
                self.u32(r.line);
                self.u8(r.is_query as u8);
            }
        }
    }

    fn actions(&mut self, actions: &[Action]) {
        self.u32(actions.len() as u32);
        for a in actions {
            self.string(&a.name);
            if !a.name.is_empty() {
                match &a.params {
                    None => self.u8(0),
                    Some(params) => {
                        self.u8(1);
                        self.u8(params.len() as u8);
                        for p in params {
                            if self.old_values {
                                self.u8(match p.spec() {
                                    value_flags::SPEC_TYPED => 0,
                                    value_flags::SPEC_VARIABLE => 1,
                                    _ => 2,
                                });
                            }
                            self.value(p);
                        }
                    }
                }
                self.u8(a.negate as u8);
            }
            self.u32(a.goal_or_hook as u32);
        }
    }

    fn value(&mut self, v: &Value) {
        if self.old_values {
            self.constant(
                v.constant
                    .as_ref()
                    .expect("old values always carry a constant"),
            );
            let spec = v.spec();
            if spec & 6 == 2 {
                for bit in [3, 4, 5] {
                    self.u8((v.flags >> bit) & 1);
                }
            }
            if spec == value_flags::SPEC_VARIABLE {
                self.u8(v.index as u8);
                for bit in [6, 7] {
                    self.u8((v.flags >> bit) & 1);
                }
            }
            return;
        }
        self.u8(v.index as u8);
        self.u8(v.flags);
        if let Some(c) = &v.constant {
            self.constant(c);
        }
    }

    fn constant(&mut self, c: &Constant) {
        let marker = match c.data {
            ConstantData::Enum(_) => b'e',
            ConstantData::Raw(_) => b'1',
            _ => b'0',
        };
        self.u8(marker);
        self.u16(c.type_id);
        match &c.data {
            ConstantData::None => {}
            ConstantData::Integer(v) | ConstantData::Raw(v) => self.u32(*v as u32),
            ConstantData::Integer64(v) => self.out.extend_from_slice(&v.to_le_bytes()),
            ConstantData::Real(v) => self.u32(v.to_bits()),
            ConstantData::String(s) | ConstantData::GuidString(s) => match s {
                None => self.u8(0),
                Some(s) => {
                    self.u8(1);
                    self.string(s);
                }
            },
            ConstantData::Object(s) | ConstantData::Enum(s) => self.string(s),
        }
    }
}
