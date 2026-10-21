//! Builder for self-authored test dialogs, producing the same LSF tree shape as compiled
//! dialogs (`dialog` → `speakerlist`, `nodes` → `node`…, `RootNodes`).

use bg3_lsf::{Guid, Node, Region, Resource, TranslatedString, Value};

use crate::Dialog;

/// A deterministic GUID from a small number (`00000000-0000-0000-0000-0000000000nn`).
pub fn g(n: u32) -> Guid {
    let mut b = [0u8; 16];
    b[12..16].copy_from_slice(&n.to_be_bytes());
    Guid(b)
}

/// Node ids in fixtures: 1000 + n.
pub fn node_id(n: u32) -> Guid {
    g(1000 + n)
}

/// A flag in a fixture: (uuid, value, paramval).
pub type FlagSpec = (Guid, bool, Option<i32>);
/// Flag groups: (type, flags).
pub type Groups = Vec<(String, Vec<FlagSpec>)>;
/// A tag rule group: (group op, [(rule op, speaker, tags)]).
pub type RuleSpec = Option<(u8, Vec<(u8, i32, Vec<Guid>)>)>;

pub struct NodeBuilder {
    node: Node,
    children: Vec<u32>,
    check: Groups,
    set: Groups,
    validated: Option<(bool, Groups)>,
    texts: Vec<Node>,
}

pub fn node(n: u32, constructor: &str) -> NodeBuilder {
    NodeBuilder {
        node: Node::new("node")
            .with_attr("constructor", Value::FixedString(constructor.into()))
            .with_attr("UUID", Value::FixedString(node_id(n).to_string())),
        children: Vec::new(),
        check: Vec::new(),
        set: Vec::new(),
        validated: None,
        texts: Vec::new(),
    }
}

impl NodeBuilder {
    pub fn attr(mut self, name: &str, value: Value) -> Self {
        self.node = self.node.with_attr(name, value);
        self
    }
    pub fn speaker(self, s: i32) -> Self {
        self.attr("speaker", Value::Int(s))
    }
    pub fn flag(self, name: &str) -> Self {
        self.attr(name, Value::Bool(true))
    }
    pub fn children(mut self, c: &[u32]) -> Self {
        self.children.extend_from_slice(c);
        self
    }
    /// A text line with handle `h<n>` and no tag rule.
    pub fn text(self, n: u32) -> Self {
        self.tagged_text(&[n], None)
    }
    /// A TaggedText with the given lines, guarded by `rule` = (group op, [(rule op, speaker,
    /// tags)]) if given.
    pub fn tagged_text(mut self, lines: &[u32], rule: RuleSpec) -> Self {
        let mut tt = Node::new("TaggedText").with_attr("HasTagRule", Value::Bool(true));
        let mut group = Node::new("RuleGroup");
        let mut rules = Node::new("Rules");
        if let Some((op, rs)) = rule {
            group = group.with_attr("TagCombineOp", Value::Byte(op));
            for (rop, speaker, tags) in rs {
                let mut tn = Node::new("Tags");
                for t in tags {
                    tn = tn.with_child(Node::new("Tag").with_attr("Object", Value::Uuid(t)));
                }
                rules = rules.with_child(
                    Node::new("Rule")
                        .with_attr("HasChildRules", Value::Bool(false))
                        .with_attr("TagCombineOp", Value::Byte(rop))
                        .with_attr("speaker", Value::Int(speaker))
                        .with_child(tn),
                );
            }
        } else {
            group = group.with_attr("TagCombineOp", Value::Byte(0));
        }
        tt = tt.with_child(group.with_child(rules));
        let mut texts = Node::new("TagTexts");
        for &l in lines {
            texts = texts.with_child(
                Node::new("TagText")
                    .with_attr(
                        "TagText",
                        Value::TranslatedString(TranslatedString {
                            value: None,
                            handle: format!("h{l}"),
                            version: 1,
                        }),
                    )
                    .with_attr("LineId", Value::Uuid(g(5000 + l)))
                    .with_attr("stub", Value::Bool(true)),
            );
        }
        self.texts.push(tt.with_child(texts));
        self
    }
    pub fn check(mut self, kind: &str, flags: &[(Guid, bool, Option<i32>)]) -> Self {
        self.check.push((kind.into(), flags.to_vec()));
        self
    }
    pub fn set(mut self, kind: &str, flags: &[(Guid, bool, Option<i32>)]) -> Self {
        self.set.push((kind.into(), flags.to_vec()));
        self
    }
    pub fn validated(
        mut self,
        has_value: bool,
        kind: &str,
        flags: &[(Guid, bool, Option<i32>)],
    ) -> Self {
        self.validated = Some((has_value, vec![(kind.into(), flags.to_vec())]));
        self
    }

    fn build(self) -> Node {
        fn groups(name: &str, gs: &[(String, Vec<FlagSpec>)]) -> Node {
            let mut n = Node::new(name);
            for (kind, flags) in gs {
                let mut fg =
                    Node::new("flaggroup").with_attr("type", Value::FixedString(kind.clone()));
                for (id, value, param) in flags {
                    let mut f = Node::new("flag")
                        .with_attr("UUID", Value::FixedString(id.to_string()))
                        .with_attr("value", Value::Bool(*value));
                    if let Some(p) = param {
                        f = f.with_attr("paramval", Value::Int(*p));
                    }
                    fg = fg.with_child(f);
                }
                n = n.with_child(fg);
            }
            n
        }
        let mut children = Node::new("children");
        for c in &self.children {
            children = children.with_child(
                Node::new("child").with_attr("UUID", Value::FixedString(node_id(*c).to_string())),
            );
        }
        let mut n = self
            .node
            .with_child(children)
            .with_child(groups("checkflags", &self.check))
            .with_child(groups("setflags", &self.set))
            .with_child(Node::new("Tags"));
        if let Some((has_value, gs)) = &self.validated {
            let v = groups("ValidatedFlags", gs)
                .with_attr("ValidatedHasValue", Value::Bool(*has_value));
            n = n.with_child(v);
        }
        if !self.texts.is_empty() {
            let mut tts = Node::new("TaggedTexts");
            for t in self.texts {
                tts = tts.with_child(t);
            }
            n = n.with_child(tts);
        }
        n
    }
}

pub struct DialogBuilder {
    uuid: Guid,
    attrs: Vec<(String, Value)>,
    speakers: Vec<Vec<Guid>>,
    nodes: Vec<NodeBuilder>,
    roots: Vec<u32>,
}

pub fn dialog(uuid: u32) -> DialogBuilder {
    DialogBuilder {
        uuid: g(uuid),
        attrs: Vec::new(),
        speakers: Vec::new(),
        nodes: Vec::new(),
        roots: Vec::new(),
    }
}

impl DialogBuilder {
    pub fn attr(mut self, name: &str, value: Value) -> Self {
        self.attrs.push((name.into(), value));
        self
    }
    /// Adds a speaker slot whose list holds these ids.
    pub fn speaker(mut self, list: &[Guid]) -> Self {
        self.speakers.push(list.to_vec());
        self
    }
    pub fn node(mut self, n: NodeBuilder) -> Self {
        self.nodes.push(n);
        self
    }
    pub fn roots(mut self, r: &[u32]) -> Self {
        self.roots.extend_from_slice(r);
        self
    }

    pub fn resource(self) -> Resource {
        let mut root = Node::new("dialog")
            .with_attr("category", Value::LSString("Test".into()))
            .with_attr("UUID", Value::FixedString(self.uuid.to_string()));
        for (k, v) in self.attrs {
            root = root.with_attr(k, v);
        }
        let mut sl = Node::new("speakerlist");
        for (i, list) in self.speakers.iter().enumerate() {
            let list: Vec<String> = list.iter().map(|g| g.to_string()).collect();
            sl = sl.with_child(
                Node::new("speaker")
                    .with_attr("index", Value::FixedString(i.to_string()))
                    .with_attr("list", Value::LSString(list.join(";")))
                    .with_attr("SpeakerMappingId", Value::Uuid(g(9000 + i as u32))),
            );
        }
        let mut nodes = Node::new("nodes");
        for n in self.nodes {
            nodes = nodes.with_child(n.build());
        }
        for r in self.roots {
            nodes = nodes.with_child(
                Node::new("RootNodes")
                    .with_attr("RootNodes", Value::FixedString(node_id(r).to_string())),
            );
        }
        root = root
            .with_child(Node::new("DefaultAddressedSpeakers"))
            .with_child(sl)
            .with_child(nodes);
        Resource {
            regions: vec![Region {
                name: "dialog".into(),
                node: root,
            }],
            ..Default::default()
        }
    }

    pub fn build(self) -> Dialog {
        Dialog::from_resource(&self.resource()).expect("fixture dialog loads")
    }
}
