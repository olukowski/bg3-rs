//! Behaviour tests on self-authored stories (see [`crate::builder`]). Expected results follow
//! the libOsiris semantics documented in `docs/research/osiris-vm.md`.

use bg3_story::{RelOp, SubGoalCombiner};

use crate::builder::*;
use crate::db::Db;
use crate::value::{StringTable, osi_compare, osi_eq};
use crate::*;

fn boot(b: StoryBuilder) -> (Vm, RecordingHost) {
    boot_with(b, RecordingHost::new())
}

fn boot_with(b: StoryBuilder, mut host: RecordingHost) -> (Vm, RecordingHost) {
    let story = b.build();
    let problems = story.validate();
    assert!(problems.is_empty(), "{problems:?}");
    let mut vm = Vm::new(&story);
    vm.init_game(&mut host);
    (vm, host)
}

fn calls(h: &RecordingHost) -> Vec<String> {
    h.calls
        .iter()
        .map(|c| format!("{}({})", c.name, c.args.join(", ")))
        .collect()
}

fn ints(vm: &Vm, name: &str, arity: usize) -> Vec<Vec<i32>> {
    vm.db(name, arity)
        .unwrap_or_else(|| panic!("no db {name}/{arity}"))
        .iter()
        .map(|r| r.iter().map(|v| v.as_int().expect("int")).collect())
        .collect()
}

fn no_errors(vm: &Vm) {
    let (errors, n) = vm.errors();
    assert_eq!(n, 0, "{errors:?}");
}

#[test]
fn builder_output_is_consistent() {
    let mut b = StoryBuilder::new();
    b.event("Ping", &[INT]);
    b.db("DB_A", &[INT]);
    b.db("DB_B", &[INT, STRING]);
    b.call("Log", &[INT]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Ping", [v("X")])
            .and("DB_A", [v("X")])
            .and_not("DB_B", [v("X"), s("no")])
            .cmp(v("X"), RelOp::Greater, int(0))
            .then("Log", [v("X")]),
    );
    let story = b.build();
    assert!(story.validate().is_empty());
    let dump = story.dump_goal(story.goal_by_name("Main").unwrap());
    assert!(dump.contains("NOT DB_B"), "{dump}");
}

#[test]
fn event_inserts_fact_once() {
    let mut b = StoryBuilder::new();
    b.event("Ping", &[INT]);
    b.db("DB_Seen", &[INT]);
    b.call("Log", &[INT]);
    let g = b.goal("Main", &[]);
    b.rule(g, Rule::when("Ping", [v("X")]).then("DB_Seen", [v("X")]));
    b.rule(g, Rule::when("DB_Seen", [v("X")]).then("Log", [v("X")]));
    let (mut vm, mut h) = boot(b);
    assert_eq!(
        vm.event(&mut h, "Ping", &[Value::int(7)]),
        EventOutcome::Processed
    );
    assert_eq!(
        vm.event(&mut h, "Ping", &[Value::int(7)]),
        EventOutcome::Processed
    );
    assert_eq!(
        vm.event(&mut h, "Ping", &[Value::int(8)]),
        EventOutcome::Processed
    );
    assert_eq!(ints(&vm, "DB_Seen", 1), vec![vec![7], vec![8]]);
    // A duplicate fact does not trigger rules again.
    assert_eq!(calls(&h), ["Log(7)", "Log(8)"]);
    assert_eq!(vm.event(&mut h, "Nope", &[]), EventOutcome::Unknown);
    no_errors(&vm);
}

#[test]
fn join_returns_rows_in_insertion_order() {
    let mut b = StoryBuilder::new();
    b.event("Ask", &[INT]);
    b.event("Drop", &[INT, STRING]);
    b.db("DB_B", &[INT, STRING]);
    b.call("Log", &[STRING]);
    let g = b.goal("Main", &[]);
    b.init(
        g,
        [
            Act::call("DB_B", [int(1), s("a")]),
            Act::call("DB_B", [int(2), s("x")]),
            Act::call("DB_B", [int(1), s("b")]),
            Act::call("DB_B", [int(1), s("c")]),
        ],
    );
    b.rule(
        g,
        Rule::when("Ask", [v("X")])
            .and("DB_B", [v("X"), v("Y")])
            .then("Log", [v("Y")]),
    );
    b.rule(
        g,
        Rule::when("Drop", [v("X"), v("Y")])
            .then_not("DB_B", [v("X"), v("Y")])
            .then("DB_B", [v("X"), v("Y")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Ask", &[Value::int(1)]);
    // Deleting and re-inserting moves a row to the end.
    let a = vm.string("a");
    vm.event(&mut h, "Drop", &[Value::int(1), a]);
    vm.event(&mut h, "Ask", &[Value::int(1)]);
    assert_eq!(
        calls(&h),
        [
            r#"Log("a")"#,
            r#"Log("b")"#,
            r#"Log("c")"#,
            r#"Log("b")"#,
            r#"Log("c")"#,
            r#"Log("a")"#
        ]
    );
    no_errors(&vm);
}

#[test]
fn db_joins_fire_from_either_side() {
    let mut b = StoryBuilder::new();
    b.event("SetA", &[INT]);
    b.event("SetB", &[INT]);
    b.db("DB_A", &[INT]);
    b.db("DB_B", &[INT]);
    b.db("DB_C", &[INT]);
    let g = b.goal("Main", &[]);
    b.rule(g, Rule::when("SetA", [v("X")]).then("DB_A", [v("X")]));
    b.rule(g, Rule::when("SetB", [v("X")]).then("DB_B", [v("X")]));
    b.rule(
        g,
        Rule::when("DB_A", [v("X")])
            .and("DB_B", [v("X")])
            .then("DB_C", [v("X")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "SetB", &[Value::int(1)]);
    vm.event(&mut h, "SetA", &[Value::int(1)]); // left side arrives second
    vm.event(&mut h, "SetA", &[Value::int(2)]);
    vm.event(&mut h, "SetB", &[Value::int(2)]); // right side arrives second
    vm.event(&mut h, "SetA", &[Value::int(3)]); // never matched
    assert_eq!(ints(&vm, "DB_C", 1), vec![vec![1], vec![2]]);
    no_errors(&vm);
}

#[test]
fn three_way_join_uses_generator_replay() {
    // DB_A ⋈ DB_B ⋈ DB_C: a fact arriving in DB_C must regenerate the (unmaterialised) A⋈B
    // tokens by replaying DB_A through the first join.
    let mut b = StoryBuilder::new();
    b.event("Add", &[INT, INT]);
    b.db("DB_A", &[INT]);
    b.db("DB_B", &[INT, INT]);
    b.db("DB_C", &[INT]);
    b.call("Log", &[INT, INT]);
    let g = b.goal("Main", &[]);
    b.init(
        g,
        [
            Act::call("DB_A", [int(1)]),
            Act::call("DB_A", [int(2)]),
            Act::call("DB_B", [int(1), int(10)]),
            Act::call("DB_B", [int(2), int(20)]),
            Act::call("DB_B", [int(1), int(11)]),
        ],
    );
    b.rule(
        g,
        Rule::when("DB_A", [v("X")])
            .and("DB_B", [v("X"), v("Y")])
            .and("DB_C", [v("Y")])
            .then("Log", [v("X"), v("Y")]),
    );
    b.rule(
        g,
        Rule::when("Add", [v("K"), v("V")])
            .cmp(v("K"), RelOp::Equal, int(3))
            .then("DB_C", [v("V")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Add", &[Value::int(3), Value::int(11)]);
    vm.event(&mut h, "Add", &[Value::int(3), Value::int(20)]);
    vm.event(&mut h, "Add", &[Value::int(4), Value::int(10)]); // filtered by the compare
    assert_eq!(calls(&h), ["Log(1, 11)", "Log(2, 20)"]);
    no_errors(&vm);
}

#[test]
fn event_rules_ignore_later_facts() {
    let mut b = StoryBuilder::new();
    b.event("Ping", &[INT]);
    b.event("Set", &[INT]);
    b.db("DB_A", &[INT]);
    b.call("Log", &[INT]);
    let g = b.goal("Main", &[]);
    b.rule(g, Rule::when("Set", [v("X")]).then("DB_A", [v("X")]));
    b.rule(
        g,
        Rule::when("Ping", [v("X")])
            .and("DB_A", [v("X")])
            .then("Log", [v("X")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Ping", &[Value::int(1)]);
    vm.event(&mut h, "Set", &[Value::int(1)]); // the event is gone: no firing
    assert!(h.calls.is_empty());
    vm.event(&mut h, "Ping", &[Value::int(1)]);
    assert_eq!(calls(&h), ["Log(1)"]);
    no_errors(&vm);
}

#[test]
fn negation_and_retraction() {
    let mut b = StoryBuilder::new();
    b.event("Ping", &[INT]);
    b.event("Block", &[INT]);
    b.event("Unblock", &[INT]);
    b.event("Have", &[INT]);
    b.db("DB_A", &[INT]);
    b.db("DB_Block", &[INT]);
    b.call("Log", &[STRING, INT]);
    let g = b.goal("Main", &[]);
    b.rule(g, Rule::when("Block", [v("X")]).then("DB_Block", [v("X")]));
    b.rule(
        g,
        Rule::when("Unblock", [v("X")]).then_not("DB_Block", [v("X")]),
    );
    b.rule(g, Rule::when("Have", [v("X")]).then("DB_A", [v("X")]));
    b.rule(
        g,
        Rule::when("Ping", [v("X")])
            .and_not("DB_Block", [v("X")])
            .then("Log", [s("ping"), v("X")]),
    );
    b.rule(
        g,
        Rule::when("DB_A", [v("X")])
            .and_not("DB_Block", [v("X")])
            .then("Log", [s("have"), v("X")]),
    );
    let (mut vm, mut h) = boot(b);
    let e = |vm: &mut Vm, h: &mut RecordingHost, n: &str, x: i32| {
        vm.event(h, n, &[Value::int(x)]);
    };
    e(&mut vm, &mut h, "Block", 1);
    e(&mut vm, &mut h, "Ping", 1); // blocked
    e(&mut vm, &mut h, "Ping", 2);
    e(&mut vm, &mut h, "Have", 1); // blocked
    e(&mut vm, &mut h, "Unblock", 1); // right-side delete re-joins DB_A(1): fires
    e(&mut vm, &mut h, "Ping", 1);
    e(&mut vm, &mut h, "Block", 1); // right-side add retracts (nothing materialised)
    e(&mut vm, &mut h, "Unblock", 1); // fires again
    assert_eq!(
        calls(&h),
        [
            r#"Log("ping", 2)"#,
            r#"Log("have", 1)"#,
            r#"Log("ping", 1)"#,
            r#"Log("have", 1)"#
        ]
    );
    no_errors(&vm);
}

#[test]
fn compares_mix_numbers_and_fold_strings() {
    let mut b = StoryBuilder::new();
    b.event("Num", &[INT]);
    b.event("Real", &[REAL]);
    b.event("Str", &[STRING]);
    b.call("Log", &[STRING]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Num", [v("X")])
            .cmp(v("X"), RelOp::Greater, int(3))
            .then("Log", [s("num>3")]),
    );
    b.rule(
        g,
        Rule::when("Real", [v("X")])
            .cmp(v("X"), RelOp::LessOrEqual, int(2))
            .then("Log", [s("real<=2")]),
    );
    b.rule(
        g,
        Rule::when("Str", [v("S")])
            .cmp(v("S"), RelOp::Equal, s("hello"))
            .then("Log", [s("eq")]),
    );
    b.rule(
        g,
        Rule::when("Str", [v("S")])
            .cmp(v("S"), RelOp::Less, s("M"))
            .then("Log", [s("lt")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Num", &[Value::int(3)]);
    vm.event(&mut h, "Num", &[Value::int(4)]);
    vm.event(&mut h, "Real", &[Value::real(2.0)]);
    vm.event(&mut h, "Real", &[Value::real(2.5)]);
    let hello = vm.string("HeLLo");
    vm.event(&mut h, "Str", &[hello]);
    let z = vm.string("zeta");
    vm.event(&mut h, "Str", &[z]);
    assert_eq!(
        calls(&h),
        [
            r#"Log("num>3")"#,
            r#"Log("real<=2")"#,
            r#"Log("eq")"#,
            r#"Log("lt")"#
        ]
    );
    no_errors(&vm);
}

#[test]
fn guid_strings_match_by_uuid() {
    let uuid = "1a2b3c4d-5e6f-4a1b-8c2d-3e4f5a6b7c8d";
    let mut b = StoryBuilder::new();
    let character = b.alias("CHARACTER", GUIDSTRING);
    b.event("Hello", &[character]);
    b.db("DB_Friend", &[character]);
    b.call("Log", &[character]);
    let g = b.goal("Main", &[]);
    b.init(
        g,
        [Act::call(
            "DB_Friend",
            [typed(character, Lit::Str(format!("S_Player_Hero_{uuid}")))],
        )],
    );
    b.rule(
        g,
        Rule::when("Hello", [v("C")])
            .and("DB_Friend", [v("C")])
            .then("Log", [v("C")]),
    );
    let (mut vm, mut h) = boot(b);
    let bare = vm.guid(uuid);
    let upper = vm.guid(&uuid.to_uppercase());
    let other = vm.guid("S_X_00000000-0000-0000-0000-000000000001");
    vm.event(&mut h, "Hello", &[bare]);
    vm.event(&mut h, "Hello", &[upper]);
    vm.event(&mut h, "Hello", &[other]);
    assert_eq!(h.calls.len(), 2, "{:?}", calls(&h));
    // The rule variable keeps the *event's* text.
    assert_eq!(h.calls[0].args, [uuid]);
    no_errors(&vm);
}

#[test]
fn proc_calls_run_depth_first() {
    let mut b = StoryBuilder::new();
    b.event("Go", &[]);
    b.proc("PROC_Inner", &[INT]);
    b.db("DB_Mark", &[INT]);
    b.call("Log", &[STRING]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Go", [])
            .then("Log", [s("before")])
            .then("PROC_Inner", [int(1)])
            .then("DB_Mark", [int(5)])
            .then("Log", [s("after")]),
    );
    b.rule(
        g,
        Rule::when("PROC_Inner", [v("X")]).then("Log", [s("inner1")]),
    );
    b.rule(
        g,
        Rule::when("PROC_Inner", [v("X")]).then("Log", [s("inner2")]),
    );
    b.rule(g, Rule::when("DB_Mark", [v("X")]).then("Log", [s("mark")]));
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Go", &[]);
    vm.event(&mut h, "Go", &[]); // PROCs fire every time, DB_Mark only when new
    assert_eq!(
        calls(&h),
        [
            r#"Log("before")"#,
            r#"Log("inner1")"#,
            r#"Log("inner2")"#,
            r#"Log("mark")"#,
            r#"Log("after")"#,
            r#"Log("before")"#,
            r#"Log("inner1")"#,
            r#"Log("inner2")"#,
            r#"Log("after")"#
        ]
    );
    no_errors(&vm);
}

#[test]
fn user_queries_succeed_if_any_rule_fires() {
    let mut b = StoryBuilder::new();
    b.event("Test", &[INT]);
    b.user_query("QRY_IsSpecial", &[INT]);
    b.db("DB_Special", &[INT]);
    b.call("Log", &[INT]);
    b.call("Side", &[INT]);
    let g = b.goal("Main", &[]);
    b.init(g, [Act::call("DB_Special", [int(5)])]);
    b.query_rule(
        g,
        Rule::when("QRY_IsSpecial", [v("X")])
            .cmp(v("X"), RelOp::Greater, int(10))
            .then("Side", [v("X")]),
    );
    b.query_rule(
        g,
        Rule::when("QRY_IsSpecial", [v("X")])
            .and("DB_Special", [v("X")])
            .then("Side", [v("X")]),
    );
    b.rule(
        g,
        Rule::when("Test", [v("X")])
            .and("QRY_IsSpecial", [v("X")])
            .then("Log", [v("X")]),
    );
    let (mut vm, mut h) = boot(b);
    for x in [1, 5, 11] {
        vm.event(&mut h, "Test", &[Value::int(x)]);
    }
    // The QRY rules' actions run as part of the query (before the caller's actions).
    assert_eq!(calls(&h), ["Side(5)", "Log(5)", "Side(11)", "Log(11)"]);
    no_errors(&vm);
}

#[test]
fn engine_queries_bind_out_parameters() {
    let mut b = StoryBuilder::new();
    b.event("Test", &[INT]);
    b.query("Double", &[INT, INT], &[false, true]);
    b.call("Log", &[INT, INT]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Test", [v("X")])
            .and("Double", [v("X"), v("Y")])
            .then("Log", [v("X"), v("Y")]),
    );
    // Bound out-parameter: the result must equal the constant.
    b.rule(
        g,
        Rule::when("Test", [v("X")])
            .and("Double", [v("X"), int(8)])
            .then("Log", [v("X"), int(-1)]),
    );
    let host = RecordingHost::with_queries(|_, name, args, out| {
        assert_eq!(name, "Double");
        assert_eq!(out, [false, true]);
        assert!(!args[1].is_valid());
        let x = args[0].as_int()?;
        if x < 0 {
            return Some(false);
        }
        args[1] = Value::int(x * 2);
        Some(true)
    });
    let (mut vm, mut h) = boot_with(b, host);
    vm.event(&mut h, "Test", &[Value::int(4)]);
    vm.event(&mut h, "Test", &[Value::int(-3)]);
    vm.event(&mut h, "Test", &[Value::int(5)]);
    assert_eq!(calls(&h), ["Log(4, 8)", "Log(4, -1)", "Log(5, 10)"]);
    assert_eq!(h.queries.len(), 6);
    no_errors(&vm);
}

#[test]
fn unknown_engine_queries_fail() {
    let mut b = StoryBuilder::new();
    b.event("Test", &[]);
    b.query("IsThing", &[INT], &[]);
    b.call("Log", &[]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Test", [])
            .and("IsThing", [int(1)])
            .then("Log", []),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Test", &[]);
    assert!(h.calls.is_empty());
    assert_eq!(h.queries.len(), 1);
    no_errors(&vm);
}

#[test]
fn goals_complete_and_wake_subgoals() {
    let mut b = StoryBuilder::new();
    b.event("Finish", &[]);
    b.event("Ping", &[]);
    b.db("DB_Log", &[STRING]);
    b.call("Log", &[STRING]);
    let main = b.goal("Main", &[]);
    let sub = b.goal("Sub", &[main]);
    let other = b.goal("Other", &[]);
    b.init(main, [Act::call("Log", [s("main init")])]);
    b.exit(main, [Act::call("Log", [s("main exit")])]);
    b.init(sub, [Act::call("Log", [s("sub init")])]);
    b.init(other, [Act::call("Log", [s("other init")])]);
    b.rule(
        main,
        Rule::when("Finish", [])
            .then("Log", [s("finishing")])
            .then_complete()
            .then("Log", [s("after complete")]),
    );
    b.rule(sub, Rule::when("Ping", []).then("Log", [s("sub ping")]));
    b.rule(main, Rule::when("Ping", []).then("Log", [s("main ping")]));
    let (mut vm, mut h) = boot(b);
    assert_eq!(calls(&h), [r#"Log("main init")"#, r#"Log("other init")"#]);
    assert_eq!(vm.goal_state("Sub"), Some(GoalState::Sleeping));
    vm.event(&mut h, "Ping", &[]);
    vm.event(&mut h, "Finish", &[]);
    vm.event(&mut h, "Ping", &[]);
    vm.event(&mut h, "Finish", &[]); // Main's rules are off now
    assert_eq!(
        calls(&h)[2..],
        [
            r#"Log("main ping")"#,
            r#"Log("finishing")"#,
            r#"Log("main exit")"#,
            r#"Log("sub init")"#,
            r#"Log("after complete")"#,
            r#"Log("sub ping")"#
        ]
    );
    assert_eq!(vm.goal_state("Main"), Some(GoalState::Completed));
    assert_eq!(vm.goal_state("sub"), Some(GoalState::Active));
    no_errors(&vm);
}

#[test]
fn and_combiner_waits_for_all_parents() {
    let mut b = StoryBuilder::new();
    b.call("Log", &[STRING]);
    let a = b.goal("A", &[]);
    let bb = b.goal("B", &[]);
    let both = b.goal("Both", &[a, bb]);
    let either = b.goal("Either", &[a, bb]);
    b.combiner(either, SubGoalCombiner::Or);
    b.init(both, [Act::call("Log", [s("both")])]);
    b.init(either, [Act::call("Log", [s("either")])]);
    b.init(a, [Act::complete()]);
    let (vm, h) = boot(b);
    // A completes during its own INIT: Either (OR) starts; Both waits for B.
    assert_eq!(calls(&h), [r#"Log("either")"#, r#"Log("either")"#]);
    // Note the second "either": InitGame reaches Either (id 4) after it was woken and runs
    // its INIT again, as COsiris::InitGame checks the flags when it gets there.
    assert_eq!(vm.goal_state("Both"), Some(GoalState::Sleeping));
    assert_eq!(vm.goal_state("A"), Some(GoalState::Completed));
    no_errors(&vm);
}

#[test]
fn sleeping_goals_miss_facts() {
    let mut b = StoryBuilder::new();
    b.event("Wake", &[]);
    b.event("Set", &[INT]);
    b.db("DB_A", &[INT]);
    b.call("Log", &[INT]);
    let main = b.goal("Main", &[]);
    let sub = b.goal("Sub", &[main]);
    b.init(main, [Act::call("DB_A", [int(1)])]);
    b.rule(main, Rule::when("Wake", []).then_complete());
    b.rule(main, Rule::when("Set", [v("X")]).then("DB_A", [v("X")]));
    b.rule(sub, Rule::when("DB_A", [v("X")]).then("Log", [v("X")]));
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Wake", &[]);
    // Facts inserted while Sub slept are not replayed when it wakes.
    assert!(h.calls.is_empty());
    // Main's rules are disabled now, so Set no longer inserts.
    vm.event(&mut h, "Set", &[Value::int(2)]);
    assert!(h.calls.is_empty());
    assert_eq!(ints(&vm, "DB_A", 1), vec![vec![1]]);
    no_errors(&vm);
}

#[test]
fn sys_calls_and_queries() {
    let mut b = StoryBuilder::new();
    b.event("Go", &[]);
    b.db("DB_A", &[INT]);
    b.db("DB_Pick", &[INT]);
    b.call("Log", &[STRING, INT]);
    let main = b.goal("Main", &[]);
    let sub = b.goal("Sub", &[main]);
    b.init(
        main,
        [Act::call("DB_A", [int(4)]), Act::call("DB_A", [int(9)])],
    );
    b.init(sub, [Act::call("Log", [s("sub"), int(0)])]);
    b.rule(
        main,
        Rule::when("Go", [])
            .and("SysCount", [s("DB_A"), int(1), v("N")])
            .and("SysStatus", [s("Sub"), v("S")])
            .then("Log", [s("count"), v("N")])
            .then("Log", [s("status"), v("S")])
            .then("SysActivateGoal", [s("Sub")])
            .then("SysFactAtIndex", [s("DB_A"), int(1), int(2), s("DB_Pick")])
            .then("SysClear", [s("DB_A"), int(1)]),
    );
    b.rule(
        main,
        Rule::when("Go", [])
            .and("SysIsActive", [s("Sub")])
            .then("Log", [s("active"), int(1)]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Go", &[]);
    // SysFactAtIndex is a sysquery; as an action it is rejected (recorded as an error).
    assert_eq!(
        calls(&h),
        [
            r#"Log("count", 2)"#,
            r#"Log("status", 1)"#,
            r#"Log("sub", 0)"#,
            r#"Log("active", 1)"#
        ]
    );
    assert_eq!(vm.db_len("DB_A", 1), Some(0));
    assert_eq!(vm.errors().1, 1);
}

#[test]
fn fact_at_index_copies_a_row() {
    let mut b = StoryBuilder::new();
    b.event("Go", &[INT]);
    b.db("DB_A", &[INT]);
    b.db("DB_Pick", &[INT]);
    let g = b.goal("Main", &[]);
    b.init(
        g,
        [
            Act::call("DB_A", [int(4)]),
            Act::call("DB_A", [int(9)]),
            Act::call("DB_Pick", [int(0)]),
        ],
    );
    b.rule(
        g,
        Rule::when("Go", [v("I")])
            .and("SysFactAtIndex", [s("DB_A"), int(1), v("I"), s("DB_Pick")])
            .then("DB_A", [int(100)]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Go", &[Value::int(2)]);
    assert_eq!(ints(&vm, "DB_Pick", 1), vec![vec![9]]);
    vm.event(&mut h, "Go", &[Value::int(7)]); // out of range: fails
    assert_eq!(ints(&vm, "DB_A", 1), vec![vec![4], vec![9], vec![100]]);
    no_errors(&vm);
}

#[test]
fn recursion_clobbers_unsaved_rule_variables() {
    // libOsiris saves/restores only the variables an action passes around a nested PROC/DB
    // call. Re-entering the same rule overwrites the others (here N), which later actions of
    // the outer firing then see.
    let mut b = StoryBuilder::new();
    b.event("Start", &[]);
    b.proc("PROC_Count", &[INT]);
    b.query("Add1", &[INT, INT], &[false, true]);
    b.call("Log", &[INT]);
    b.call("Log2", &[INT, INT]);
    let g = b.goal("Main", &[]);
    b.rule(g, Rule::when("Start", []).then("PROC_Count", [int(0)]));
    b.rule(
        g,
        Rule::when("PROC_Count", [v("N")])
            .cmp(v("N"), RelOp::Less, int(3))
            .and("Add1", [v("N"), v("M")])
            .then("Log", [v("N")])
            .then("PROC_Count", [v("M")])
            .then("Log2", [v("N"), v("M")]),
    );
    let host = RecordingHost::with_queries(|_, _, args, _| {
        args[1] = Value::int(args[0].as_int()? + 1);
        Some(true)
    });
    let (mut vm, mut h) = boot_with(b, host);
    vm.event(&mut h, "Start", &[]);
    assert_eq!(
        calls(&h),
        [
            "Log(0)",
            "Log(1)",
            "Log(2)",
            "Log2(2, 3)",
            "Log2(2, 2)",
            "Log2(2, 1)"
        ]
    );
    assert_eq!(vm.stats().max_rule_nesting, 4);
    no_errors(&vm);
}

#[test]
fn nesting_limit_stops_runaway_recursion() {
    let mut b = StoryBuilder::new();
    b.event("Start", &[]);
    b.proc("PROC_Loop", &[]);
    let g = b.goal("Main", &[]);
    b.rule(g, Rule::when("Start", []).then("PROC_Loop", []));
    b.rule(g, Rule::when("PROC_Loop", []).then("PROC_Loop", []));
    let (mut vm, mut h) = boot(b);
    vm.set_max_rule_nesting(50);
    vm.event(&mut h, "Start", &[]);
    assert_eq!(vm.errors().1, 1);
}

#[test]
fn constants_in_conditions_filter() {
    let mut b = StoryBuilder::new();
    b.event("Pair", &[INT, STRING]);
    b.call("Log", &[INT]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Pair", [v("X"), s("yes")]).then("Log", [v("X")]),
    );
    b.rule(
        g,
        Rule::when("Pair", [int(2), any()]).then("Log", [int(-2)]),
    );
    let (mut vm, mut h) = boot(b);
    let yes = vm.string("YES");
    let no = vm.string("no");
    vm.event(&mut h, "Pair", &[Value::int(1), yes]);
    vm.event(&mut h, "Pair", &[Value::int(2), no]);
    assert_eq!(calls(&h), ["Log(1)", "Log(-2)"]);
    no_errors(&vm);
}

#[test]
fn repeated_variable_requires_equal_columns() {
    let mut b = StoryBuilder::new();
    b.event("Pair", &[INT, INT]);
    b.call("Log", &[INT]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Pair", [v("X"), v("X")]).then("Log", [v("X")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Pair", &[Value::int(1), Value::int(2)]);
    vm.event(&mut h, "Pair", &[Value::int(3), Value::int(3)]);
    assert_eq!(calls(&h), ["Log(3)"]);
    no_errors(&vm);
}

// ---- values and databases ----

#[test]
fn string_and_guid_equality() {
    let mut t = StringTable::default();
    let s = |t: &mut StringTable, x: &str| Value {
        ty: 4,
        root: Root::String,
        data: Data::Str(t.intern(x, false)),
    };
    let g = |t: &mut StringTable, x: &str| Value {
        ty: 5,
        root: Root::GuidString,
        data: Data::Str(t.intern(x, true)),
    };
    assert!(osi_eq(&s(&mut t, "Hello"), &s(&mut t, "hELLO")).unwrap());
    assert!(!osi_eq(&s(&mut t, "Hello"), &s(&mut t, "Hell")).unwrap());
    let u = "9f8e7d6c-5b4a-4321-8fed-cba987654321";
    assert!(osi_eq(&g(&mut t, &format!("S_Hero_{u}")), &g(&mut t, u)).unwrap());
    // Everything without a valid non-zero UUID shares id 0.
    let null = g(&mut t, "NULL_00000000-0000-0000-0000-000000000000");
    assert!(osi_eq(&null, &g(&mut t, "")).unwrap());
    assert!(osi_eq(&g(&mut t, "not a guid"), &null).unwrap());
    // Mixed roots are an invalid comparison.
    assert!(osi_eq(&s(&mut t, "x"), &g(&mut t, "x")).is_err());
    assert!(osi_eq(&Value::int(1), &Value::int64(1)).is_err());
    // Compare: GUID vs STRING uses the last 36 characters case-insensitively.
    let a = g(&mut t, &format!("S_Hero_{u}"));
    let b = s(&mut t, &u.to_uppercase());
    assert_eq!(osi_compare(&t, &a, &b, RelOp::Equal), Ok(true));
    assert!(osi_compare(&t, &b, &a, RelOp::Equal).is_err());
    assert_eq!(
        osi_compare(&t, &Value::int(2), &Value::real(2.5), RelOp::Less),
        Ok(true)
    );
    assert_eq!(
        osi_compare(&t, &Value::int64(1 << 40), &Value::int(1), RelOp::Greater),
        Ok(true)
    );
    let one = s(&mut t, "1");
    assert!(osi_compare(&t, &Value::int(1), &one, RelOp::Equal).is_err());
}

#[test]
fn db_order_select_and_wildcard_erase() {
    let mut db = Db::new(2);
    for (a, b) in [(1, 1), (2, 1), (1, 2), (3, 3), (1, 3)] {
        assert!(db.insert(&[Value::int(a), Value::int(b)]));
    }
    assert!(!db.insert(&[Value::int(1), Value::int(2)]));
    let col0 = |db: &Db, x| {
        db.select(&[(0, Value::int(x))])
            .into_iter()
            .map(|s| db.row(s)[1].as_int().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(col0(&db, 1), [1, 2, 3]);
    assert!(db.valid(&[Value::int(3), Value::UNDEF]));
    assert!(!db.valid(&[Value::int(4), Value::UNDEF]));
    // A tuple with an unset column erases every match.
    assert!(db.erase(&[Value::int(1), Value::UNDEF]));
    assert_eq!(db.len(), 2);
    assert!(db.insert(&[Value::int(1), Value::int(1)]));
    let rows: Vec<Vec<i32>> = db
        .rows()
        .map(|r| r.iter().map(|v| v.as_int().unwrap()).collect())
        .collect();
    assert_eq!(rows, [[2, 1], [3, 3], [1, 1]]);
    // Many deletions compact without changing order.
    for i in 0..500 {
        db.insert(&[Value::int(100 + i), Value::int(0)]);
    }
    for i in 0..500 {
        db.erase(&[Value::int(100 + i), Value::int(0)]);
    }
    let rows: Vec<Vec<i32>> = db
        .rows()
        .map(|r| r.iter().map(|v| v.as_int().unwrap()).collect())
        .collect();
    assert_eq!(rows, [[2, 1], [3, 3], [1, 1]]);
    assert_eq!(col0(&db, 1), [1]);
}

#[test]
fn zero_arity_databases_store_nothing() {
    let mut db = Db::new(0);
    assert!(db.insert(&[]));
    assert!(db.insert(&[]));
    assert_eq!(db.len(), 0);
}

// ---- Findings from replaying a runtime log of the game (docs/research/osiris-runtime-log.md) ----

#[test]
fn engine_values_keep_their_type_id() {
    // A GUIDSTRING event argument for a CHARACTER parameter is not retagged; neither is a
    // GUIDSTRING answer for a TAG out-parameter. The log prints both without the alias prefix.
    let mut b = StoryBuilder::new();
    let character = b.alias("CHARACTER", GUIDSTRING);
    let tag = b.alias("TAG", GUIDSTRING);
    b.event("Hello", &[character]);
    b.query("GetTag", &[character, tag], &[false, true]);
    b.db("DB_Seen", &[character, tag]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Hello", [v("C")])
            .and("GetTag", [v("C"), v("T")])
            .then("DB_Seen", [v("C"), v("T")]),
    );
    let host = RecordingHost::with_queries(|ctx, _, args, _| {
        args[1] = ctx.guid("TAG_00000000-0000-0000-0000-000000000002");
        Some(true)
    });
    let (mut vm, mut h) = boot_with(b, host);
    let c = vm.guid("S_A_00000000-0000-0000-0000-000000000001");
    vm.event(&mut h, "Hello", &[c]);
    let rows = vm.db("DB_Seen", 2).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].ty, GUIDSTRING);
    assert_eq!(rows[0][1].ty, GUIDSTRING);
    no_errors(&vm);
}

#[test]
fn unset_out_parameters_carry_the_root_type() {
    // `exec [DIV query] DataGetReallyTagAt( …, [out] (GUIDSTRING) )` for a TAG out-parameter.
    let mut b = StoryBuilder::new();
    let amount = b.alias("AMOUNT", INT);
    b.event("Test", &[]);
    b.query("GetAmount", &[amount], &[true]);
    b.call("Log", &[amount]);
    let g = b.goal("Main", &[]);
    b.rule(
        g,
        Rule::when("Test", [])
            .and("GetAmount", [v("A")])
            .then("Log", [v("A")]),
    );
    let host = RecordingHost::with_queries(|_, _, args, _| {
        assert_eq!(args[0].ty, INT, "out-parameter typed with the root type");
        assert!(!args[0].is_valid());
        args[0] = Value::int(3);
        Some(true)
    });
    let (mut vm, mut h) = boot_with(b, host);
    vm.set_logging(true);
    vm.event(&mut h, "Test", &[]);
    assert_eq!(calls(&h), ["Log(3)"]);
    let log = vm.take_log();
    assert!(
        log.iter()
            .any(|r| r.to_string() == "exec [DIV query] GetAmount( [out] (INTEGER) )"),
        "{log:#?}"
    );
    no_errors(&vm);
}

#[test]
fn user_query_arguments_arrive_with_root_types() {
    // The QRY rules receive the arguments as an engine argument list: a CHARACTER value
    // becomes a GUIDSTRING inside the query (seen as `Rule variables: S_…` without prefix).
    let mut b = StoryBuilder::new();
    let character = b.alias("CHARACTER", GUIDSTRING);
    b.event("Test", &[]);
    b.user_query("QRY_Check", &[character]);
    b.db("DB_Who", &[character]);
    b.db("DB_Inside", &[character]);
    let g = b.goal("Main", &[]);
    b.init(
        g,
        [Act::call(
            "DB_Who",
            [typed(
                character,
                Lit::Guid("S_A_00000000-0000-0000-0000-000000000001".into()),
            )],
        )],
    );
    b.query_rule(
        g,
        Rule::when("QRY_Check", [v("C")]).then("DB_Inside", [v("C")]),
    );
    b.rule(
        g,
        Rule::when("Test", [])
            .and("DB_Who", [v("C")])
            .and("QRY_Check", [v("C")])
            .then("DB_Inside", [v("C")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.event(&mut h, "Test", &[]);
    let rows = vm.db("DB_Inside", 1).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].ty, GUIDSTRING);
    assert_eq!(vm.db("DB_Who", 1).unwrap()[0][0].ty, character);
    no_errors(&vm);
}

#[test]
fn runtime_log_matches_libosiris_order() {
    let mut b = StoryBuilder::new();
    b.event("Go", &[]);
    b.db("DB_A", &[INT]);
    b.query("Fails", &[INT], &[true]);
    b.call("Log", &[INT]);
    let main = b.goal("Main", &[]);
    let sub = b.goal("Sub", &[main]);
    b.exit(main, [Act::call("DB_A", [int(1)])]);
    b.init(sub, [Act::call("DB_A", [int(2)])]);
    b.rule(main, Rule::when("Go", []).then_complete());
    b.rule(
        sub,
        Rule::when("Go", [])
            .and("Fails", [v("X")])
            .then("Log", [v("X")]),
    );
    let (mut vm, mut h) = boot(b);
    vm.set_logging(true);
    vm.event(&mut h, "Go", &[]);
    vm.event(&mut h, "Go", &[]);
    let text: Vec<String> = vm.take_log().iter().map(|r| r.to_string()).collect();
    assert_eq!(
        text,
        [
            ">>> call COsiris::Event",
            ">>> event Go()",
            "0> RuleActionPart of THEN at line 0 fires. Rule variables: ",
            "1--> GoalCompleted; (\"Main\") (GoalID == 1)",
            "exec EXIT section of goal \"Main\" (#1):",
            "1--> DB_A( 1 ) [add fact]",
            "exec INIT section of goal \"Sub\" (#2):",
            "1--> DB_A( 2 ) [add fact]",
            "exec INIT section of goal \"Sub\" (#2) END",
            // END of the EXIT section only after the subgoals have started.
            "exec EXIT section of goal \"Main\" (#1) END",
            "0> [end of rule action part]",
            // Sub woke during this event; its rule is reached later in the same event.
            "exec [DIV query] Fails( [out] (INTEGER) )",
            "*** Query failed! ***",
            "     Query returns: Fails( (undef value) )",
            ">>> call COsiris::Event",
            ">>> event Go()",
            "exec [DIV query] Fails( [out] (INTEGER) )",
            "*** Query failed! ***",
            "     Query returns: Fails( (undef value) )",
        ],
    );
    no_errors(&vm);
}

#[test]
fn replay_of_own_log_has_no_divergences() {
    let build = || {
        let mut b = StoryBuilder::new();
        b.event("Ping", &[INT]);
        b.query("Double", &[INT, INT], &[false, true]);
        b.db("DB_Seen", &[INT, INT]);
        b.call("Log", &[INT]);
        let g = b.goal("Main", &[]);
        b.init(g, [Act::call("DB_Seen", [int(0), int(0)])]);
        b.rule(
            g,
            Rule::when("Ping", [v("X")])
                .and("Double", [v("X"), v("Y")])
                .then("DB_Seen", [v("X"), v("Y")])
                .then("Log", [v("Y")]),
        );
        b.build()
    };
    let story = build();
    let mut vm = Vm::new(&story);
    vm.set_logging(true);
    let mut host = RecordingHost::with_queries(|_, _, args, _| {
        args[1] = Value::int(args[0].as_int()? * 2);
        Some(true)
    });
    vm.init_game(&mut host);
    for x in [1, 2, 1] {
        vm.event(&mut host, "Ping", &[Value::int(x)]);
    }
    let log = crate::trace::render_log(&vm.take_log());
    let report = crate::replay::replay(&story, &log);
    assert_eq!(report.divergences(), 0, "{}", report.summary());
    assert_eq!(report.segments, 4);
    assert_eq!(report.host.exact, 3);
    assert_eq!(report.type_mismatches, 0);

    // A different logged answer changes what the VM does: detected.
    let bad = log.replacen(
        "Query returns: Double( 2, 4 )",
        "Query returns: Double( 2, 5 )",
        1,
    );
    let report = crate::replay::replay(&story, &bad);
    let d = report.first.as_ref().expect("divergence");
    assert_eq!(d.segment, 2);
    assert!(
        d.trace
            .as_deref()
            .unwrap()
            .ends_with("Rule variables: 2, 4"),
        "{d:?}"
    );
    assert!(
        d.vm.as_deref().unwrap().ends_with("Rule variables: 2, 5"),
        "{d:?}"
    );
}
