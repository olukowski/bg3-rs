//! Install-free tests of the sim as the world of dialogs and rules, on synthetic data.

use std::collections::HashMap;
use std::sync::Arc;

use bg3_dialog::library::DialogResource;
use bg3_dialog::model::{
    Dialog, Flag, FlagGroup, FlagKind, Node, NodeKind, Roll, Speaker, TaggedText, TextLine,
};
use bg3_osiris::builder::*;
use bg3_rules::condition::{ConditionContext, ConditionEngine};
use bg3_rules::context::{Ability, RulesContext, Skill};
use bg3_rules::dice::ScriptedDice;
use bg3_rules::exec::Executor;
use bg3_rules::functor::FunctorList;

use crate::character::CharacterDef;
use crate::dialog::{DialogBank, DialogScript, InstanceState, Notification, to_lsf};
use crate::driver::Game;
use crate::events::SimEvent;
use crate::flags::{FlagDef, FlagUsage};
use crate::guid::Guid;
use crate::sim::{GameData, NULL_OBJECT, PlacedObject, Sim, TemplateInfo};
use crate::status::StatusDef;
use crate::tags::TagDef;
use crate::{EntityKind, StoryState};

const HERO: &str = "11111111-0000-0000-0000-000000000001";
const BOX: &str = "11111111-0000-0000-0000-000000000002";
const TMPL: &str = "22222222-0000-0000-0000-000000000001";
const TAG_A: &str = "33333333-0000-0000-0000-00000000000a";
const TAG_D: &str = "33333333-0000-0000-0000-00000000000d";
const F_GLOBAL: &str = "44444444-0000-0000-0000-000000000001";
const F_OBJECT: &str = "44444444-0000-0000-0000-000000000002";
const DLG: &str = "55555555-0000-0000-0000-000000000001";
const DC: &str = "66666666-0000-0000-0000-000000000001";

fn g(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

fn lg(s: &str) -> bg3_lsf::Guid {
    to_lsf(g(s))
}

fn node_uuid(n: u32) -> bg3_lsf::Guid {
    to_lsf(Guid(0x7777_0000_0000_0000_0000_0000_0000_0000 + n as u128))
}

fn placed(id: &str, name: &str, kind: EntityKind) -> PlacedObject {
    PlacedObject {
        id: g(id),
        name: name.to_owned(),
        kind,
        template: Some(g(TMPL)),
        level: Some("TestLevel".to_owned()),
        global: true,
        position: [0.0; 3],
        tags: Vec::new(),
        stats: None,
        statuses: Vec::new(),
        race: None,
        faction: None,
        level_override: None,
    }
}

/// A hero (Wisdom 14, proficient in Medicine, 20 HP) and a box.
fn data() -> GameData {
    let mut d = GameData::default();
    for (id, name, usage) in [(F_GLOBAL, "Test_Global", 5), (F_OBJECT, "Test_Object", 4)] {
        d.flags.insert(FlagDef {
            id: g(id),
            name: name.to_owned(),
            usage: FlagUsage::from_raw(usage),
            description: String::new(),
        });
    }
    for (id, name) in [(TAG_A, "TEST_A"), (TAG_D, "TEST_DIALOG")] {
        d.tags.insert(TagDef {
            id: g(id),
            name: name.to_owned(),
            categories: Vec::new(),
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
            stats: Some("Test_Hero".to_owned()),
            statuses: Vec::new(),
            faction: None,
            level_override: Some(3),
        },
    );
    let mut abilities = [10; 6];
    abilities[Ability::Wisdom as usize] = 14;
    d.characters.insert(
        "Test_Hero".to_owned(),
        CharacterDef {
            name: "Test_Hero".to_owned(),
            abilities,
            level: 1,
            vitality: 20,
            armor: 12,
            proficiency_bonus: None,
            proficiency_scaling: None,
            spellcasting_ability: Some(Ability::Wisdom),
            saving_throws: vec![Ability::Wisdom],
            skills: vec![Skill::Medicine],
            expertise: Vec::new(),
            skill_bonus: Vec::new(),
            passives: vec!["Test_Passive".to_owned()],
        },
    );
    d.statuses.insert(StatusDef {
        name: "SLEEPING".to_owned(),
        status_type: "INCAPACITATED".to_owned(),
        groups: Vec::new(),
    });
    d.difficulty_classes.insert(g(DC), 13);
    d.globals = vec![
        placed(HERO, "S_Hero", EntityKind::Character),
        placed(BOX, "S_Box", EntityKind::Item),
    ];
    d
}

fn flags(kind: FlagKind, list: &[(&str, bool, Option<i32>)]) -> FlagGroup {
    FlagGroup {
        kind,
        flags: list
            .iter()
            .map(|(f, value, param)| Flag {
                uuid: lg(f),
                value: *value,
                param: *param,
            })
            .collect(),
    }
}

fn text(handle: &str) -> Vec<TaggedText> {
    vec![TaggedText {
        has_tag_rule: false,
        rule: Default::default(),
        lines: vec![TextLine {
            handle: handle.to_owned(),
            version: 1,
            line_id: None,
            stub: false,
            custom_sequence_id: None,
            dialog_variables: None,
        }],
    }]
}

fn node(n: u32, kind: NodeKind, speaker: i32, children: &[usize]) -> Node {
    let mut x = Node::new(node_uuid(n), kind);
    x.speaker = Some(speaker);
    x.children = children.to_vec();
    x.texts = text(&format!("h{n}"));
    x
}

/// Greeting (sets a global flag, an object flag and a dialog tag on the hero) → the hero
/// either asks (needs the global flag; the answer clears it) or tries a Medicine check (DC
/// 13; success clears the object flag).
fn dialog() -> Dialog {
    let mut d = Dialog::default();
    d.uuid = lg(DLG);
    d.speakers = vec![
        Speaker {
            index: 0,
            list: vec![lg(HERO)],
            ..Speaker::default()
        },
        Speaker {
            index: 1,
            list: vec![lg(BOX)],
            ..Speaker::default()
        },
    ];
    let mut n0 = node(0, NodeKind::Greeting, 0, &[1, 2]);
    n0.root = true;
    n0.set_flags = vec![
        flags(FlagKind::Global, &[(F_GLOBAL, true, None)]),
        flags(FlagKind::Object, &[(F_OBJECT, true, Some(0))]),
        flags(FlagKind::Tag, &[(TAG_D, true, Some(0))]),
    ];
    let mut n1 = node(1, NodeKind::Question, 0, &[5]);
    n1.check_flags = vec![flags(FlagKind::Global, &[(F_GLOBAL, true, None)])];
    let roll = Roll {
        ability: "Wisdom".to_owned(),
        skill: "Medicine".to_owned(),
        roll_type: "SkillCheck".to_owned(),
        difficulty_class: Some(lg(DC)),
        ..Roll::default()
    };
    let n2 = node(2, NodeKind::ActiveRoll(roll), 0, &[3, 4]);
    let mut n3 = Node::new(node_uuid(3), NodeKind::RollResult { success: true });
    n3.children = vec![6];
    let mut n4 = Node::new(node_uuid(4), NodeKind::RollResult { success: false });
    n4.children = vec![7];
    let mut n5 = node(5, NodeKind::Answer, 1, &[]);
    n5.end_node = true;
    n5.set_flags = vec![flags(FlagKind::Global, &[(F_GLOBAL, false, None)])];
    let mut n6 = node(6, NodeKind::Answer, 1, &[]);
    n6.end_node = true;
    n6.set_flags = vec![flags(FlagKind::Object, &[(F_OBJECT, false, Some(0))])];
    let mut n7 = node(7, NodeKind::Answer, 1, &[]);
    n7.end_node = true;
    d.nodes = vec![n0, n1, n2, n3, n4, n5, n6, n7];
    d.roots = vec![0];
    let resource = DialogResource {
        id: lg(DLG),
        name: "TEST_Talk".to_owned(),
        ..DialogResource::default()
    };
    d.resource = Some(resource);
    d
}

fn bank() -> Arc<DialogBank> {
    let d = dialog();
    let resource = d.resource.clone().unwrap();
    let mut map: HashMap<bg3_lsf::Guid, Arc<Dialog>> = HashMap::new();
    map.insert(lg(DLG), Arc::new(d));
    Arc::new(DialogBank::new(Arc::new(map), [resource]))
}

fn sim() -> Sim {
    let mut s = Sim::new(data());
    s.set_dialog_bank(bank());
    s.take_events();
    s
}

fn keys(events: &[SimEvent]) -> Vec<String> {
    events.iter().map(|e| e.key()).collect()
}

#[test]
fn characters_get_sheets_from_their_stats() {
    let s = sim();
    let hero = s.entity(g(HERO)).unwrap().sheet.clone().unwrap();
    assert_eq!((hero.level, hero.hp, hero.armor_class), (3, 20, 12));
    assert_eq!(hero.proficiency_bonus, 2, "5e table without a level map");
    assert!(s.entity(g(BOX)).unwrap().sheet.is_none(), "items have none");
}

#[test]
fn dialog_plays_on_the_sim() {
    let mut s = sim();
    let dlg = format!("TEST_Talk_{DLG}");
    let hero = format!("S_Hero_{HERO}");
    let bx = format!("S_Box_{BOX}");
    let id = s
        .request_dialog(g(DLG), &[g(HERO), Guid::NULL, g(BOX)])
        .unwrap();
    assert_eq!(id, 1);
    assert!(s.is_speaker_reserved(g(HERO)));
    assert_eq!(s.dialog(id).unwrap().state, InstanceState::Requested);
    assert!(s.take_events().is_empty(), "starts at the next update");

    s.update_dialogs();
    assert_eq!(
        keys(&s.take_events()),
        [
            format!("DialogStarted({dlg}, 1)"),
            format!("DialogActorJoined({dlg}, 1, {hero}, 0)"),
            format!("DialogActorJoined({dlg}, 1, {bx}, 1)"),
            format!("FlagSet(Test_Object_{F_OBJECT}, {hero}, 1)"),
            format!("FlagSet(Test_Global_{F_GLOBAL}, {NULL_OBJECT}, 1)"),
            // The dialog tag reaches the merged tags at the end of the frame.
            format!("TagSet({hero}, {TAG_D})"),
        ]
    );
    assert_eq!(StoryState::flag(&s, g(F_OBJECT), g(HERO)), Ok(true));
    assert!(s.is_tagged(g(HERO), g(TAG_D)));
    let n = s.take_notifications();
    assert!(matches!(
        n[0],
        Notification::DialogStarted { instance: 1, .. }
    ));
    assert!(
        matches!(&n[1], Notification::DialogLine { line, .. } if line.text.as_ref().unwrap().handle == "h0")
    );

    s.dialog_advance(id).unwrap();
    let Some(Notification::DialogChoices { choices, .. }) = s.take_notifications().pop() else {
        panic!("choices expected");
    };
    assert_eq!(choices.len(), 2);
    assert!(choices[1].roll.is_some());

    // The Medicine check: d20 9 + 2 (Wisdom) + 2 (proficiency) = 13 vs DC 13.
    s.dice = crate::rules::SimDice::new(0);
    s.set_dialog_script(
        id,
        DialogScript {
            rolls: [true].into(),
            ..DialogScript::default()
        },
    );
    s.dialog_choose(id, 1).unwrap();
    let roll = s.dialog(id).unwrap().rolls[0].clone();
    let check = roll.check.as_ref().unwrap();
    assert_eq!(check.dc, 13);
    assert_eq!(check.total - check.d20.natural as i32, 4, "{check}");
    assert!(roll.success && roll.forced && roll.consistent);
    let node = node_uuid(2);
    assert_eq!(
        keys(&s.take_events()),
        [
            format!("RollResult(\"{node}_1\", {hero}, {NULL_OBJECT}, 1, 1, CRITICALITYTYPE.NONE)"),
            format!("DialogRollResult({hero}, 1, {dlg}, 0, CRITICALITYTYPE.NONE)"),
            format!("FlagCleared(Test_Object_{F_OBJECT}, {hero}, 1)"),
        ]
    );

    s.dialog_advance(id).unwrap();
    assert_eq!(
        keys(&s.take_events()),
        [
            format!("DialogActorLeft({dlg}, 1, {hero}, 1)"),
            format!("DialogActorLeft({dlg}, 1, {bx}, 1)"),
            format!("DialogEnded({dlg}, 1)"),
        ]
    );
    assert_eq!(s.dialog(id).unwrap().state, InstanceState::Ended);
    assert!(!s.is_speaker_reserved(g(HERO)));
    assert_eq!(s.dialog_involved(id, false), Some(vec![g(HERO), g(BOX)]));
}

#[test]
fn unscripted_rolls_use_the_dice_and_scripts_drive_choices() {
    let roll = |seed: u64| {
        let mut s = sim();
        s.dice = crate::rules::SimDice::new(seed);
        let id = s.request_dialog(g(DLG), &[g(HERO), g(BOX)]).unwrap();
        s.set_dialog_script(
            id,
            DialogScript {
                choices: [1].into(),
                ..DialogScript::default()
            },
        );
        while s.dialog_autostep(id) {}
        let i = s.dialog(id).unwrap();
        assert_eq!(i.state, InstanceState::Ended);
        let r = i.rolls[0].clone();
        assert!(!r.forced);
        r.check.unwrap()
    };
    assert_eq!(roll(7), roll(7), "same seed, same roll");
    let r = roll(7);
    assert_eq!(r.success, r.total >= 13 || r.d20.natural == 20);
}

#[test]
fn request_stop_ends_a_running_dialog() {
    let mut s = sim();
    let id = s.request_dialog(g(DLG), &[g(HERO), g(BOX)]).unwrap();
    s.dialog_request_stop(g(HERO));
    assert!(
        s.dialog(id).is_none(),
        "a dialog that has not started is dropped"
    );
    let id = s.request_dialog(g(DLG), &[g(HERO), g(BOX)]).unwrap();
    s.update_dialogs();
    s.take_events();
    s.dialog_request_stop(g(BOX));
    let ev = keys(&s.take_events());
    assert_eq!(
        ev.last().unwrap(),
        &format!("DialogEnded(TEST_Talk_{DLG}, {id})")
    );
}

/// The story requests a dialog through `StartDialog_Internal`, the game ticks, and the
/// story hears `DialogStarted` and the dialog's `FlagSet`s; the player's input finishes it.
#[test]
fn game_drives_story_and_dialog() {
    let mut b = StoryBuilder::new();
    b.event("Start", &[]);
    b.event("DialogStarted", &[GUIDSTRING, INT]);
    b.event("DialogEnded", &[GUIDSTRING, INT]);
    b.event("FlagCleared", &[GUIDSTRING, GUIDSTRING, INT]);
    let mut params = vec![GUIDSTRING, INT];
    params.extend([GUIDSTRING; 6]);
    params.extend([INT, INT, INT]);
    let mut out = vec![false; 8];
    out.extend([true, true, false]);
    b.query("StartDialog_Internal", &params, &out);
    b.db("DB_Requested", &[INT]);
    b.db("DB_Started", &[GUIDSTRING, INT]);
    b.db("DB_Ended", &[INT]);
    b.db("DB_Cleared", &[GUIDSTRING, INT]);
    let goal = b.goal("Main", &[]);
    let null = guid(NULL_OBJECT);
    b.rule(
        goal,
        Rule::when("Start", [])
            .and(
                "StartDialog_Internal",
                [
                    guid(&format!("TEST_Talk_{DLG}")),
                    int(0),
                    guid(&format!("S_Hero_{HERO}")),
                    guid(&format!("S_Box_{BOX}")),
                    null.clone(),
                    null.clone(),
                    null.clone(),
                    null,
                    v("Ok"),
                    v("Inst"),
                    int(0),
                ],
            )
            .then("DB_Requested", [v("Inst")]),
    );
    b.rule(
        goal,
        Rule::when("DialogStarted", [v("D"), v("I")]).then("DB_Started", [v("D"), v("I")]),
    );
    b.rule(
        goal,
        Rule::when("DialogEnded", [any(), v("I")]).then("DB_Ended", [v("I")]),
    );
    b.rule(
        goal,
        Rule::when("FlagCleared", [v("F"), any(), v("I")]).then("DB_Cleared", [v("F"), v("I")]),
    );
    let story = b.build();

    let mut game = Game::new(&story, sim());
    game.init();
    game.event(SimEvent::new("Start", Vec::new()));
    assert_eq!(game.vm.db("DB_Requested", 1).unwrap().len(), 1);
    assert!(game.vm.db("DB_Started", 2).unwrap().is_empty());
    game.tick(0.0);
    let started = game.vm.db("DB_Started", 2).unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0][1].as_int(), Some(1));
    // Line → choices → the question (index 0) → its answer → end.
    game.dialog_advance(1).unwrap();
    game.dialog_choose(1, 0).unwrap();
    game.dialog_advance(1).unwrap();
    game.tick(0.0);
    let cleared = game.vm.db("DB_Cleared", 2).unwrap();
    assert_eq!(cleared.len(), 1);
    assert_eq!(
        game.vm.text(&cleared[0][0]),
        Some(format!("Test_Global_{F_GLOBAL}").as_str())
    );
    assert_eq!(game.vm.db("DB_Ended", 1).unwrap().len(), 1);
    let n = game.take_notifications();
    assert!(matches!(
        n.last(),
        Some(Notification::DialogEnded { instance: 1 })
    ));
}

#[test]
fn rules_evaluate_against_sim_entities() {
    let mut s = sim();
    let hero = s.entity_id(g(HERO)).unwrap();
    let bx = s.entity_id(g(BOX)).unwrap();
    assert_eq!(RulesContext::level(&s, hero), 3);
    assert_eq!(s.ability_modifier(hero, Ability::Wisdom), 2);
    assert!(s.saving_throw_proficient(hero, Ability::Wisdom));
    assert!(s.has_passive(hero, "Test_Passive"));
    assert!(s.has_tag(hero, "TEST_A") && !s.has_tag(hero, "TEST_DIALOG"));
    assert!(RulesContext::is_item(&s, bx) && !s.is_dead(bx));

    let mut engine = ConditionEngine::new();
    let mut rng = ScriptedDice::new(&[10]);
    let data = ConditionContext {
        source: Some(hero),
        target: Some(hero),
        ..ConditionContext::default()
    };
    let mut check = |s: &mut Sim, c: &str| {
        engine
            .evaluate_str(c, s, &mut rng, &data)
            .unwrap()
            .result
            .result
    };
    assert!(check(
        &mut s,
        "Character() and Tagged('TEST_A') and not Dead()"
    ));
    assert!(!check(&mut s, "HasStatus('SLEEPING')"));

    // Functors change the sim and raise the engine's events.
    let list = FunctorList::parse("ApplyStatus(SLEEPING,100,2);DealDamage(5,Fire)").unwrap();
    let mut engine2 = ConditionEngine::new();
    let mut dice = ScriptedDice::new(&[1]);
    Executor::new(&mut engine2, &mut s, &mut dice).run_list(&list, hero, hero, false);
    assert!(check(&mut s, "HasStatus('SLEEPING')"));
    assert_eq!(s.hp(hero), 15);
    let who = format!("S_Hero_{HERO}");
    assert_eq!(
        keys(&s.take_events()),
        [
            format!("StatusAttempt({who}, \"SLEEPING\", {HERO}, 0)"),
            format!("StatusApplied({who}, \"SLEEPING\", {HERO}, 0)"),
            format!("HitpointsChanged({who}, 75.0)"),
        ]
    );
    assert_eq!(s.entity(g(HERO)).unwrap().statuses[0].remaining, 12.0);
}
