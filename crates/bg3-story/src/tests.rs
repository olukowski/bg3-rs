use std::collections::HashMap;

use crate::*;

// ---- synthetic story (self-authored; no game content) ----

const T_CHARACTER: TypeId = 6;
const T_MOOD: TypeId = 7;

fn nid(n: u32) -> Option<NodeId> {
    NodeId::from_raw(n)
}

fn conn(node: u32, entry: EntryPoint, goal: u32) -> Connection {
    Connection {
        node: nid(node),
        entry,
        goal: GoalId::from_raw(goal),
    }
}

fn no_db() -> DbLink {
    DbLink {
        node: None,
        connection: conn(0, EntryPoint::None, 0),
        indirection: 0,
    }
}

/// Builds values the way each format stores them: before 1.14 every value carries a constant
/// and plain values have no flag bits beyond their specialisation.
struct Values {
    old: bool,
}

impl Values {
    fn constant(&self, index: i8, type_id: TypeId, data: ConstantData) -> Value {
        Value {
            index: if self.old { -1 } else { index },
            flags: value_flags::SPEC_VALUE | if self.old { 0 } else { value_flags::VALID },
            constant: Some(Constant { type_id, data }),
        }
    }

    fn column(&self, column: i8, type_id: TypeId, data: ConstantData) -> Value {
        let mut v = self.constant(column, type_id, data);
        v.index = column;
        v
    }

    fn var(&self, index: i8, type_id: TypeId) -> Value {
        let null = ConstantData::GuidString(None);
        Value {
            index,
            flags: value_flags::SPEC_VARIABLE | value_flags::VALID | value_flags::ADAPTED,
            constant: Some(Constant {
                type_id,
                data: if type_id == 1 {
                    ConstantData::Integer(0)
                } else {
                    null
                },
            }),
        }
    }

    fn unbound(&self) -> Value {
        Value {
            index: -1,
            flags: value_flags::SPEC_VALUE,
            constant: (self.old).then_some(Constant {
                type_id: 0,
                data: ConstantData::None,
            }),
        }
    }
}

fn sig(name: &str, params: &[TypeId], out_mask: u8) -> Signature {
    Signature {
        name: name.into(),
        out_param_mask: vec![out_mask],
        params: params.to_vec(),
    }
}

fn func(kind: FunctionKind, node: u32, signature: Signature) -> Function {
    Function {
        line: 1,
        condition_refs: 1,
        action_refs: 0,
        node: nid(node),
        kind,
        keys: [0, 0, 0, 0],
        signature,
    }
}

/// A one-goal story with the rule
/// `IF Ev(_Var0) AND DB_Foo(_Var0, _Var1) AND _Var1 > 5 THEN Act("hi"); NOT DB_Foo(_Var0, _Var1);`
fn synthetic_story(minor: u8) -> Story {
    let v = Values { old: minor < 14 };
    let types = vec![
        TypeInfo {
            name: "INTEGER".into(),
            index: 1,
            alias: 0,
        },
        TypeInfo {
            name: "STRING".into(),
            index: 4,
            alias: 0,
        },
        TypeInfo {
            name: "GUIDSTRING".into(),
            index: 5,
            alias: 0,
        },
        TypeInfo {
            name: "CHARACTER".into(),
            index: 6,
            alias: 5,
        },
        TypeInfo {
            name: "MOOD".into(),
            index: 7,
            alias: 2,
        },
    ];
    let enums = vec![EnumDef {
        type_id: T_MOOD,
        elements: vec![("HAPPY".into(), 1), ("GRUMPY".into(), 2)],
    }];
    let functions = vec![
        func(FunctionKind::Event, 1, sig("Ev", &[T_CHARACTER], 0)),
        func(
            FunctionKind::Database,
            2,
            sig("DB_Foo", &[T_CHARACTER, 1], 0),
        ),
        func(FunctionKind::Call, 0, sig("Act", &[4], 0)),
        func(
            FunctionKind::Query,
            0,
            sig("GetMood", &[T_CHARACTER, T_MOOD], 0x40),
        ),
    ];
    let nodes = vec![
        Node {
            id: nid(1).unwrap(),
            database: None,
            name: "Ev".into(),
            arity: 1,
            kind: NodeKind::Event(StartNode {
                successors: vec![conn(3, EntryPoint::Left, 1)],
            }),
        },
        Node {
            id: nid(2).unwrap(),
            database: DatabaseId::from_raw(1),
            name: "DB_Foo".into(),
            arity: 2,
            kind: NodeKind::Database(StartNode {
                successors: vec![conn(3, EntryPoint::Right, 1)],
            }),
        },
        Node {
            id: nid(3).unwrap(),
            database: None,
            name: String::new(),
            arity: 0,
            kind: NodeKind::And(JoinNode {
                next: conn(4, EntryPoint::None, 1),
                left_parent: nid(1),
                right_parent: nid(2),
                left_adapter: AdapterId::from_raw(1),
                right_adapter: AdapterId::from_raw(2),
                left_db: no_db(),
                right_db: DbLink {
                    node: nid(2),
                    connection: conn(3, EntryPoint::Right, 1),
                    indirection: 1,
                },
            }),
        },
        Node {
            id: nid(4).unwrap(),
            database: None,
            name: String::new(),
            arity: 0,
            kind: NodeKind::Compare(CompareNode {
                unary: UnaryNode {
                    next: conn(5, EntryPoint::None, 1),
                    parent: nid(3),
                    adapter: AdapterId::from_raw(3),
                    db: no_db(),
                },
                left_var: 1,
                right_var: -1,
                left_value: v.unbound(),
                right_value: v.constant(-1, 1, ConstantData::Integer(5)),
                op: RelOp::Greater,
            }),
        },
        Node {
            id: nid(5).unwrap(),
            database: None,
            name: String::new(),
            arity: 0,
            kind: NodeKind::Rule(RuleNode {
                unary: UnaryNode {
                    next: conn(0, EntryPoint::None, 0),
                    parent: nid(4),
                    adapter: AdapterId::from_raw(3),
                    db: no_db(),
                },
                actions: vec![
                    Action {
                        name: "Act".into(),
                        params: Some(vec![v.constant(
                            -1,
                            4,
                            ConstantData::String(Some("hi".into())),
                        )]),
                        negate: false,
                        goal_or_hook: 0,
                    },
                    Action {
                        name: "DB_Foo".into(),
                        params: Some(vec![v.var(0, T_CHARACTER), v.var(1, 1)]),
                        negate: true,
                        goal_or_hook: 0,
                    },
                ],
                variables: vec![v.var(0, T_CHARACTER), v.var(1, 1)],
                line: 12,
                is_query: false,
            }),
        },
        Node {
            id: nid(6).unwrap(),
            database: None,
            name: "GetMood".into(),
            arity: 2,
            kind: NodeKind::DivQuery,
        },
    ];
    let adapters = vec![
        Adapter {
            id: AdapterId::from_raw(1).unwrap(),
            constants: vec![],
            logical_indices: vec![0],
            logical_to_physical: vec![(0, 0)],
        },
        Adapter {
            id: AdapterId::from_raw(2).unwrap(),
            constants: vec![],
            logical_indices: vec![0, 1],
            logical_to_physical: vec![(0, 0), (1, 1)],
        },
        Adapter {
            id: AdapterId::from_raw(3).unwrap(),
            constants: vec![v.column(2, T_MOOD, ConstantData::Enum("HAPPY".into()))],
            logical_indices: vec![0, 1, -1],
            logical_to_physical: vec![(0, 0), (1, 1)],
        },
    ];
    let guid = || {
        ConstantData::GuidString(Some(
            "S_Someone_00000000-0000-0000-0000-000000000001".into(),
        ))
    };
    let databases = vec![Database {
        id: DatabaseId::from_raw(1).unwrap(),
        param_types: vec![T_CHARACTER, 1],
        facts: vec![vec![
            v.constant(-1, T_CHARACTER, guid()),
            v.constant(-1, 1, ConstantData::Integer(7)),
        ]],
    }];
    let goals = vec![Goal {
        id: GoalId::from_raw(1).unwrap(),
        name: "Test_Goal".into(),
        combiner: SubGoalCombiner::And,
        parents: vec![],
        subgoals: vec![],
        flags: 0,
        init_actions: vec![Action {
            name: "DB_Foo".into(),
            params: Some(vec![
                v.constant(-1, T_CHARACTER, guid()),
                v.constant(-1, 1, ConstantData::Integer(7)),
            ]),
            negate: false,
            goal_or_hook: 0,
        }],
        exit_actions: vec![Action {
            name: String::new(),
            params: None,
            negate: false,
            goal_or_hook: 1,
        }],
    }];
    let mut story_version = [0u8; 0x80];
    story_version[..7].copy_from_slice(b"1.2.3.4");
    Story {
        header: Header {
            buffer_flag: 0,
            banner: "Synthetic test story".into(),
            major: 1,
            minor,
            big_endian: false,
            unused: 0,
            story_version,
            debug_flags: 0x1234,
        },
        type_index: TypeIndex::new(&types),
        types,
        enums,
        objects: vec![],
        functions,
        nodes: Table::from_items(nodes).unwrap(),
        adapters: Table::from_items(adapters).unwrap(),
        databases: Table::from_items(databases).unwrap(),
        goals: Table::from_items(goals).unwrap(),
        global_actions: vec![],
        function_index: HashMap::new(),
    }
}

fn assert_round_trip(minor: u8) -> Story {
    let original = synthetic_story(minor);
    let bytes = writer::write_story(&original);
    let parsed = Story::parse(&bytes).expect("parse synthetic story");
    assert_eq!(parsed.header, original.header);
    assert_eq!(parsed.types, original.types);
    assert_eq!(parsed.enums, original.enums);
    assert_eq!(parsed.functions, original.functions);
    assert_eq!(parsed.nodes.as_slice(), original.nodes.as_slice());
    assert_eq!(parsed.adapters.as_slice(), original.adapters.as_slice());
    assert_eq!(parsed.databases.as_slice(), original.databases.as_slice());
    assert_eq!(parsed.goals.as_slice(), original.goals.as_slice());
    assert_eq!(writer::write_story(&parsed), bytes);
    assert!(parsed.validate().is_empty(), "{:?}", parsed.validate());
    parsed
}

#[test]
fn round_trip_1_15() {
    assert_round_trip(15);
}

#[test]
fn round_trip_1_13() {
    assert_round_trip(13);
}

#[test]
fn strings_are_scrambled() {
    let bytes = writer::write_story(&synthetic_story(15));
    // The banner (before the scramble key is set) is plain; names after the header are not.
    assert!(bytes.windows(9).any(|w| w == b"Synthetic"));
    assert!(!bytes.windows(9).any(|w| w == b"Test_Goal"));
}

#[test]
fn lookups() {
    let story = assert_round_trip(15);
    let f = story.function("GetMood", 2).unwrap();
    assert_eq!(f.kind, FunctionKind::Query);
    assert!(!f.signature.is_out(0) && f.signature.is_out(1));
    assert!(story.function("GetMood", 1).is_none());
    assert_eq!(story.root_type(T_CHARACTER), Some(BuiltinType::GuidString));
    assert_eq!(story.root_type(T_MOOD), Some(BuiltinType::Integer64));
    assert_eq!(story.type_name(T_CHARACTER), "CHARACTER");
    assert_eq!(story.goal_by_name("Test_Goal").unwrap().id.get(), 1);
    let goals = story.node_goals();
    assert_eq!(goals[5], GoalId::from_raw(1));
    assert_eq!(story.database_names()[1], Some("DB_Foo"));
}

#[test]
fn dump_reconstructs_rule() {
    let story = assert_round_trip(15);
    let rule = story.dump_rule(nid(5).unwrap()).unwrap();
    let expected = "IF
Ev(_Var0)
AND
DB_Foo(_Var0, _Var1)
AND
_Var1 > 5
THEN
\
                    Act(\"hi\");
NOT DB_Foo(_Var0, _Var1);
";
    assert!(rule.ends_with(expected), "{rule}");
    let goal = story.dump_goal(story.goal_by_name("Test_Goal").unwrap());
    assert!(
        goal.contains(
            "INITSECTION
DB_Foo((CHARACTER)S_Someone_"
        ),
        "{goal}"
    );
    assert!(
        goal.contains(
            "EXITSECTION
GoalCompleted;
"
        ),
        "{goal}"
    );
    let full = story.to_string();
    assert!(full.contains("query GetMood([in](CHARACTER)_Arg0, [out](MOOD)_Arg1)"));
    assert!(full.contains("DB_Foo((CHARACTER)S_Someone_00000000-0000-0000-0000-000000000001, 7);"));
}

#[test]
fn rejects_truncated_and_trailing() {
    let bytes = writer::write_story(&synthetic_story(15));
    for cut in [1, bytes.len() / 2, bytes.len() - 1] {
        assert!(Story::parse(&bytes[..cut]).is_err(), "cut at {cut}");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(matches!(
        Story::parse(&longer),
        Err(Error::TrailingBytes { count: 1, .. })
    ));
}

#[test]
fn rejects_unknown_version() {
    let mut story = synthetic_story(15);
    story.header.minor = 16;
    let bytes = writer::write_story(&story);
    assert!(matches!(
        Story::parse(&bytes),
        Err(Error::UnsupportedVersion {
            major: 1,
            minor: 16
        })
    ));
}

// ---- shipped stories ----

/// Story files shipped with the reference build: (pak stem, path inside the pak).
const SHIPPED_STORIES: &[(&str, &str)] = &[
    ("Gustav", "Mods/GustavDev/Story/story.div.osi"),
    ("Gustav", "Mods/Honour/Story/story.div.osi"),
    ("Gustav", "Mods/Gustav/Story/story.div.osi"),
    ("Shared", "Mods/SharedDev/Story/story.div.osi"),
    ("GustavX", "Mods/GustavX/Story/story.div.osi"),
    ("GustavX", "Mods/HonourX/Story/story.div.osi"),
];

fn read_shipped(pak_stem: &str, path: &str) -> Option<Vec<u8>> {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let pak_path = install.data_dir().join(format!("{pak_stem}.pak"));
    let pak = bg3_pak::Package::open(&pak_path).expect("open pak");
    let entry = pak
        .find(path)
        .unwrap_or_else(|| panic!("{path} not in {pak_stem}"));
    Some(pak.read(entry).expect("read story"))
}

#[test]
#[ignore = "needs BG3 install"]
fn load_shipped_stories() {
    for &(pak, path) in SHIPPED_STORIES {
        let Some(data) = read_shipped(pak, path) else {
            return;
        };
        let story = Story::parse(&data).unwrap_or_else(|e| panic!("{path}: {e}"));
        let rules = story
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Rule(_)))
            .count();
        let facts: usize = story.databases.iter().map(|d| d.facts.len()).sum();
        eprintln!(
            "{path}: v{}.{} ({}), {} types, {} enums, {} functions, {} nodes ({rules} rules), \
             {} adapters, {} dbs ({facts} facts), {} goals, {} global actions",
            story.header.major,
            story.header.minor,
            story.header.story_version_str(),
            story.types.len(),
            story.enums.len(),
            story.functions.len(),
            story.nodes.len(),
            story.adapters.len(),
            story.databases.len(),
            story.goals.len(),
            story.global_actions.len(),
        );
        assert!((MIN_VERSION..=MAX_VERSION).contains(&story.header.version()));
        assert!(story.goals.len() > 50, "{path}");
        assert!(story.databases.len() > 500, "{path}");
        assert!(story.nodes.len() > 5_000, "{path}");
        assert!(rules > 1000, "{path}");
        assert!(story.functions.len() > 1000, "{path}");
        let problems = story.validate();
        assert!(
            problems.is_empty(),
            "{path}: {:?}",
            &problems[..problems.len().min(10)]
        );
        // Re-serialising must reproduce the file byte for byte: nothing was dropped.
        let written = crate::writer::write_story(&story);
        if let Some(at) = written.iter().zip(&data).position(|(a, b)| a != b) {
            panic!(
                "{path}: round-trip differs at 0x{at:x}: {:02x?} vs {:02x?}",
                &data[at.saturating_sub(24)..at + 8],
                &written[at.saturating_sub(24)..at + 8]
            );
        }
        assert_eq!(written.len(), data.len(), "{path}: round-trip length");
    }
}
