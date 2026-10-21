//! Install-free tests on synthetic data.

use bg3_osiris::builder::*;
use bg3_osiris::{Value, Vm};

use crate::entity::EntityKind;
use crate::events::SimValue;
use crate::flags::{FlagDef, FlagError, FlagUsage};
use crate::guid::Guid;
use crate::host::{CALLS, QUERIES, SimHost, implements};
use crate::sim::{GameData, NULL_OBJECT, PlacedObject, Sim, TemplateInfo};
use crate::status::StatusDef;

const HERO: &str = "11111111-0000-0000-0000-000000000001";
const BOX: &str = "11111111-0000-0000-0000-000000000002";
const TRIGGER: &str = "11111111-0000-0000-0000-000000000003";
const TMPL: &str = "22222222-0000-0000-0000-000000000001";
const TAG_A: &str = "33333333-0000-0000-0000-00000000000a";
const TAG_B: &str = "33333333-0000-0000-0000-00000000000b";
const F_GLOBAL: &str = "44444444-0000-0000-0000-000000000001";
const F_OBJECT: &str = "44444444-0000-0000-0000-000000000002";
const F_USER: &str = "44444444-0000-0000-0000-000000000003";
const F_DIALOG: &str = "44444444-0000-0000-0000-000000000004";

fn g(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

fn obj(id: &str, name: &str, kind: EntityKind, tags: &[&str]) -> PlacedObject {
    PlacedObject {
        id: g(id),
        name: name.to_owned(),
        kind,
        template: Some(g(TMPL)),
        level: Some("TestLevel".to_owned()),
        global: true,
        position: [0.0; 3],
        tags: tags.iter().map(|t| g(t)).collect(),
        stats: None,
        statuses: Vec::new(),
        race: None,
        faction: None,
    }
}

fn data() -> GameData {
    let mut d = GameData::default();
    for (id, name, usage) in [
        (F_GLOBAL, "Test_Global", 5),
        (F_OBJECT, "Test_Object", 4),
        (F_USER, "Test_User", 2),
        (F_DIALOG, "Test_DialogOnly", 1),
    ] {
        d.flags.insert(FlagDef {
            id: g(id),
            name: name.to_owned(),
            usage: FlagUsage::from_raw(usage),
            description: String::new(),
        });
    }
    d.templates.insert(
        g(TMPL),
        TemplateInfo {
            id: g(TMPL),
            name: "BASE_Hero".to_owned(),
            kind: EntityKind::Character,
            parent: None,
            tags: vec![g(TAG_A)],
            race: None,
            faction: None,
            stats: None,
            statuses: Vec::new(),
        },
    );
    d.globals = vec![
        obj(HERO, "S_Hero", EntityKind::Character, &[]),
        obj(BOX, "S_Box", EntityKind::Item, &[TAG_B]),
        obj(TRIGGER, "S_Trigger", EntityKind::Trigger, &[]),
    ];
    d.statuses.insert(StatusDef {
        name: "SLEEPING".to_owned(),
        status_type: "INCAPACITATED".to_owned(),
        groups: vec!["SG_Sleeping".to_owned(), "SG_Unconscious".to_owned()],
    });
    d
}

fn sim() -> Sim {
    let mut s = Sim::new(data());
    s.take_events();
    s
}

fn names(events: &[crate::SimEvent]) -> Vec<String> {
    events.iter().map(|e| e.key()).collect()
}

#[test]
fn spawn_announces_template_and_instance_tags() {
    let mut s = Sim::new(data());
    let ev = names(&s.take_events());
    assert_eq!(
        ev,
        [
            format!("TagSet(S_Hero_{HERO}, {TAG_A})"),
            format!("TagSet(S_Box_{BOX}, {TAG_A})"),
            format!("TagSet(S_Box_{BOX}, {TAG_B})"),
            format!("TagSet(S_Trigger_{TRIGGER}, {TAG_A})"),
        ],
        "template tags, then the instance's"
    );
}

#[test]
fn global_flag_raises_flag_set_once() {
    let mut s = sim();
    for _ in 0..3 {
        s.set_flag(g(F_GLOBAL), Guid::NULL, true, 0, true).unwrap();
    }
    assert_eq!(
        names(&s.take_events()),
        [format!("FlagSet(Test_Global_{F_GLOBAL}, {NULL_OBJECT}, 0)")]
    );
    assert_eq!(s.get_flag(g(F_GLOBAL), g(HERO)), Ok(true), "object ignored");
    s.set_flag(g(F_GLOBAL), Guid::NULL, false, 0, true).unwrap();
    s.set_flag(g(F_GLOBAL), Guid::NULL, false, 0, true).unwrap();
    assert_eq!(
        names(&s.take_events()),
        [format!(
            "FlagCleared(Test_Global_{F_GLOBAL}, {NULL_OBJECT}, 0)"
        )]
    );
}

#[test]
fn object_and_user_flags_need_the_right_object() {
    let mut s = sim();
    assert_eq!(
        s.set_flag(g(F_OBJECT), Guid::NULL, true, 0, true),
        Err(FlagError::NoObject)
    );
    assert_eq!(
        s.set_flag(g(F_OBJECT), g(TRIGGER), true, 0, true),
        Err(FlagError::NotGameObject)
    );
    assert_eq!(
        s.set_flag(g(F_USER), g(BOX), true, 0, true),
        Err(FlagError::NotCharacter)
    );
    assert_eq!(
        s.set_flag(g(F_DIALOG), Guid::NULL, true, 0, true),
        Err(FlagError::Unscriptable)
    );
    assert_eq!(
        s.set_flag(Guid::NULL, Guid::NULL, true, 0, true),
        Err(FlagError::UnknownFlag)
    );
    s.set_flag(g(F_OBJECT), g(BOX), true, 3, true).unwrap();
    assert_eq!(s.get_flag(g(F_OBJECT), g(BOX)), Ok(true));
    assert_eq!(s.get_flag(g(F_OBJECT), g(HERO)), Ok(false));
    assert_eq!(
        names(&s.take_events()),
        [format!("FlagSet(Test_Object_{F_OBJECT}, S_Box_{BOX}, 3)")]
    );
    s.set_flag(g(F_OBJECT), g(HERO), true, 0, false).unwrap();
    assert!(s.take_events().is_empty(), "no event without notify");
}

#[test]
fn story_tags_merge_like_the_tag_system() {
    let mut s = sim();
    assert!(s.set_tag(g(HERO), g(TAG_A)), "already a template tag");
    assert!(s.set_tag(g(HERO), g(TAG_B)));
    assert!(s.is_tagged(g(HERO), g(TAG_B)), "visible at once");
    assert!(
        !s.set_tag(g(TRIGGER), g(TAG_B)),
        "triggers carry no story tags"
    );
    assert_eq!(
        names(&s.take_events()),
        [format!("TagSet(S_Hero_{HERO}, {TAG_B})")]
    );
    s.clear_tag(g(HERO), g(TAG_A));
    s.clear_tag(g(HERO), g(TAG_B));
    assert!(s.is_tagged(g(HERO), g(TAG_B)), "until the next merge");
    assert_eq!(
        names(&s.take_events()),
        [format!("TagCleared(S_Hero_{HERO}, {TAG_B})")]
    );
    assert!(s.is_tagged(g(HERO), g(TAG_A)), "template tag stays");
}

#[test]
fn timers_fire_on_the_clock() {
    let mut s = sim();
    assert!(s.object_timer_launch(g(HERO), "Wake", 1500, true, false));
    assert!(!s.object_timer_launch(Guid(7), "Wake", 1500, true, false));
    assert!(s.timer_launch("Frame", 0));
    assert!(s.timer_launch("Late", 5000));
    s.advance(0.0);
    assert_eq!(names(&s.take_events()), ["TimerFinished(\"Frame\")"]);
    s.advance(1.0);
    assert!(s.take_events().is_empty());
    s.timer_cancel("Late");
    s.advance(1.0);
    assert_eq!(
        names(&s.take_events()),
        [format!("ObjectTimerFinished(S_Hero_{HERO}, \"Wake\")")]
    );
    s.advance(10.0);
    assert!(s.take_events().is_empty());
}

#[test]
fn statuses_and_groups() {
    let mut s = sim();
    assert!(s.apply_status(g(HERO), "SLEEPING", -1.0, Guid::NULL));
    assert!(s.has_active_status(g(HERO), "SLEEPING"));
    assert!(s.is_status_from_group("SLEEPING", "SG_Unconscious"));
    assert!(!s.is_status_from_group("SLEEPING", "SG_Invisible"));
    let null = Guid::NULL.to_string();
    assert_eq!(
        names(&s.take_events()),
        [
            format!("StatusAttempt(S_Hero_{HERO}, \"SLEEPING\", {null}, -1)"),
            format!("StatusApplied(S_Hero_{HERO}, \"SLEEPING\", {null}, -1)"),
        ]
    );
    assert!(s.remove_status(g(HERO), "SLEEPING"));
    assert!(!s.has_active_status(g(HERO), "SLEEPING"));
}

#[test]
fn implemented_list_matches_dispatch() {
    for (n, a) in CALLS.iter().chain(QUERIES) {
        assert!(implements(n, *a));
    }
    assert!(!implements("SetFlag", 3));
}

/// The story sets a tag, then reacts to the engine's `TagSet` delivered after the call.
#[test]
fn story_runs_on_the_sim() {
    let mut b = StoryBuilder::new();
    b.event("Start", &[]);
    b.event("TagSet", &[GUIDSTRING, GUIDSTRING]);
    b.call("SetTag", &[GUIDSTRING, GUIDSTRING]);
    b.query(
        "IsTagged",
        &[GUIDSTRING, GUIDSTRING, INT],
        &[false, false, true],
    );
    b.db("DB_Tagged", &[GUIDSTRING, GUIDSTRING]);
    b.db("DB_SeenAtOnce", &[INT]);
    let goal = b.goal("Main", &[]);
    let hero = format!("S_Hero_{HERO}");
    b.rule(
        goal,
        Rule::when("Start", [])
            .then("SetTag", [guid(&hero), guid(TAG_B)])
            .then("SetTag", [guid(&hero), guid(TAG_A)]),
    );
    b.rule(
        goal,
        Rule::when("Start", [])
            .and("IsTagged", [guid(&hero), guid(TAG_B), v("Has")])
            .then("DB_SeenAtOnce", [v("Has")]),
    );
    b.rule(
        goal,
        Rule::when("TagSet", [v("X"), v("T")]).then("DB_Tagged", [v("X"), v("T")]),
    );
    let story = b.build();
    let mut vm = Vm::new(&story);
    let mut s = sim();
    vm.init_game(&mut SimHost::new(&mut s));
    vm.event(&mut SimHost::new(&mut s), "Start", &[]);
    assert_eq!(
        vm.db("DB_Tagged", 2).unwrap().len(),
        0,
        "nothing delivered yet"
    );
    let seen = vm.db("DB_SeenAtOnce", 1).unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0][0].as_int(), Some(1), "IsTagged sees SetTag at once");
    for e in s.take_events() {
        let args: Vec<Value> = e
            .args
            .iter()
            .map(|a| match a {
                SimValue::Guid(t) => vm.guid(t),
                SimValue::Str(t) => vm.string(t),
                SimValue::Int(i) => Value::int(*i),
                SimValue::Real(r) => Value::real(*r),
            })
            .collect();
        vm.event(&mut SimHost::new(&mut s), &e.name, &args);
    }
    let rows = vm.db("DB_Tagged", 2).unwrap();
    assert_eq!(rows.len(), 1, "TAG_A was a template tag: no event");
    assert_eq!(vm.text(&rows[0][1]), Some(TAG_B));
}
