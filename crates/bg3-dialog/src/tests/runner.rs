use std::collections::HashMap;
use std::sync::Arc;

use bg3_lsf::Value;

use super::fixture::{dialog, g, node, node_id};
use crate::{
    DialogEvent, DialogRunner, DialogWorld, FlagType, MemoryWorld, RollOutcome, StartParams, Step,
};

const PLAYER: u32 = 1;
const NPC: u32 = 2;

/// Speaker 0 = NPC, speaker 1 = player (as `GROUP_Players`).
fn players_group() -> crate::Guid {
    crate::Guid::parse("e0d1ff71-04a8-4340-ae64-9684d846eb83").unwrap()
}

fn start(d: crate::Dialog, world: &mut MemoryWorld) -> DialogRunner {
    let mut r = DialogRunner::new(
        Arc::new(d),
        Arc::new(()),
        StartParams {
            instance: 7,
            speakers: vec![Some(g(NPC)), Some(g(PLAYER))],
        },
    );
    r.start(world);
    r
}

fn line_handle(step: &Step) -> String {
    match step {
        Step::Line(l) => l
            .text
            .as_ref()
            .map(|t| t.handle.clone())
            .unwrap_or_default(),
        other => panic!("expected a line, got {other:?}"),
    }
}

fn choice_handles(step: &Step) -> Vec<String> {
    match step {
        Step::Choices(c) => c
            .iter()
            .map(|c| {
                c.text
                    .as_ref()
                    .map(|t| t.handle.clone())
                    .unwrap_or_default()
            })
            .collect(),
        other => panic!("expected choices, got {other:?}"),
    }
}

fn flag_events(r: &mut DialogRunner) -> Vec<(crate::Guid, Option<crate::Guid>, bool)> {
    r.take_events()
        .into_iter()
        .filter_map(|e| match e {
            DialogEvent::FlagSet { flag, target, .. } => Some((flag, target, true)),
            DialogEvent::FlagCleared { flag, target, .. } => Some((flag, target, false)),
            _ => None,
        })
        .collect()
}

#[test]
fn first_valid_root_starts_and_sets_flags() {
    let (met, global) = (g(100), g(101));
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .text(1)
                .check("Object", &[(met, true, Some(1))]),
        )
        .node(
            node(2, "TagGreeting")
                .speaker(0)
                .text(2)
                .check("Object", &[(met, false, Some(1))])
                .set("Object", &[(met, true, Some(1))])
                .set("Global", &[(global, true, None)]),
        )
        .roots(&[1, 2])
        .build();
    let mut w = MemoryWorld::new();
    w.define_flag(met, FlagType::Character);
    w.define_flag(global, FlagType::Global);
    let mut r = start(d.clone(), &mut w);
    assert_eq!(line_handle(r.step()), "h2");
    let events = r.take_events();
    assert_eq!(
        events[0],
        DialogEvent::Started {
            dialog: crate::DialogRef {
                id: g(1),
                name: String::new()
            },
            instance: 7,
            automated: false
        }
    );
    let flags: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, DialogEvent::FlagSet { .. }))
        .cloned()
        .collect();
    assert_eq!(
        flags,
        vec![
            DialogEvent::FlagSet {
                flag: met,
                target: Some(g(PLAYER)),
                instance: 7
            },
            DialogEvent::FlagSet {
                flag: global,
                target: None,
                instance: 7
            },
        ]
    );
    // No children: the dialog ends.
    r.advance(&mut w).unwrap();
    assert_eq!(r.step(), &Step::Ended);
    assert!(matches!(
        r.take_events().last(),
        Some(DialogEvent::Ended { .. })
    ));

    // Second run: the flag is now set, so the first root plays.
    let r = start(d, &mut w);
    assert_eq!(line_handle(r.step()), "h1");
}

/// Greeting → questions; the answer is a leaf, so the runner returns to the hub, where the
/// `ShowOnce` question is gone. An end node ends the dialog.
#[test]
fn hub_choices_show_once_and_end_node() {
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .text(1)
                .children(&[2, 3, 4]),
        )
        .node(
            node(2, "TagQuestion")
                .speaker(1)
                .text(2)
                .flag("ShowOnce")
                .children(&[5]),
        )
        .node(node(3, "TagQuestion").speaker(1).text(3).children(&[6]))
        // An NPC child next to questions: player options win.
        .node(node(4, "TagAnswer").speaker(0).text(4))
        .node(node(5, "TagAnswer").speaker(0).text(5))
        .node(
            node(6, "TagAnswer")
                .speaker(0)
                .text(6)
                .flag("endnode")
                .children(&[1]),
        )
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    let mut r = start(d, &mut w);
    assert_eq!(line_handle(r.step()), "h1");
    r.advance(&mut w).unwrap();
    assert_eq!(choice_handles(r.step()), ["h2", "h3"]);
    r.choose(&mut w, 0).unwrap();
    assert_eq!(line_handle(r.step()), "h5");
    r.advance(&mut w).unwrap();
    // Back at the hub; question 2 was shown once.
    assert_eq!(choice_handles(r.step()), ["h3"]);
    r.choose(&mut w, 0).unwrap();
    assert_eq!(line_handle(r.step()), "h6");
    r.advance(&mut w).unwrap();
    assert!(r.is_ended());
}

#[test]
fn jumps_alias_and_pop() {
    let (f, g_set) = (g(100), g(101));
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(node(1, "TagGreeting").speaker(0).text(1).children(&[10]))
        // Jump to node 2's children (point 2) → node 3.
        .node(
            node(10, "Jump")
                .attr("jumptarget", Value::FixedString(node_id(2).to_string()))
                .attr("jumptargetpoint", Value::Byte(2))
                // A jump's own flags are never used.
                .check("Global", &[(f, true, None)]),
        )
        .node(node(2, "TagAnswer").speaker(0).text(2).children(&[3]))
        .node(node(3, "TagAnswer").speaker(0).text(3).children(&[11]))
        // Alias of node 2: source text, own flags, own children.
        .node(
            node(11, "Alias")
                .speaker(-1)
                .attr("SourceNode", Value::FixedString(node_id(2).to_string()))
                .set("Global", &[(g_set, true, None)])
                .children(&[12]),
        )
        // Jump to node 2 itself (point 1).
        .node(
            node(12, "Jump")
                .attr("jumptarget", Value::FixedString(node_id(4).to_string()))
                .attr("jumptargetpoint", Value::Byte(1)),
        )
        .node(node(4, "TagAnswer").speaker(0).text(4).flag("endnode"))
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    w.define_flag(f, FlagType::Global);
    w.define_flag(g_set, FlagType::Global);
    let mut r = start(d, &mut w);
    let mut lines = vec![line_handle(r.step())];
    while !r.is_ended() {
        r.advance(&mut w).unwrap();
        if let Step::Line(_) = r.step() {
            lines.push(line_handle(r.step()));
        }
    }
    assert_eq!(lines, ["h1", "h3", "h2", "h4"]);
    assert!(w.get_flag(&g_set, FlagType::Global, None));
}

#[test]
fn pop_returns_past_the_latest_hub() {
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(node(1, "TagGreeting").speaker(0).text(1).children(&[2, 3]))
        .node(node(2, "TagQuestion").speaker(1).text(2).children(&[4]))
        .node(node(3, "TagQuestion").speaker(1).text(3).flag("endnode"))
        .node(node(4, "TagAnswer").speaker(0).text(4).children(&[5, 6]))
        .node(node(5, "TagQuestion").speaker(1).text(5).children(&[7]))
        .node(node(6, "TagQuestion").speaker(1).text(6))
        .node(node(7, "Pop").attr("PopLevel", Value::Int(1)))
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    let mut r = start(d, &mut w);
    r.advance(&mut w).unwrap();
    r.choose(&mut w, 0).unwrap(); // h2
    assert_eq!(line_handle(r.step()), "h4");
    r.advance(&mut w).unwrap();
    assert_eq!(choice_handles(r.step()), ["h5", "h6"]);
    r.choose(&mut w, 0).unwrap(); // h5 → Pop(1): back to the first hub, not to node 4.
    assert_eq!(choice_handles(r.step()), ["h2", "h3"]);
}

#[test]
fn rolls_pick_the_matching_result() {
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(node(1, "TagGreeting").speaker(0).text(1).children(&[2]))
        .node(
            node(2, "PassiveRoll")
                .speaker(1)
                .attr("Ability", Value::String("Wisdom".into()))
                .attr("Skill", Value::String("Perception".into()))
                .attr("RollType", Value::String("SkillCheck".into()))
                .attr("RollTargetSpeaker", Value::Int(0))
                .attr("DifficultyClassID", Value::Uuid(g(77)))
                .children(&[3, 4]),
        )
        .node(
            node(3, "RollResult")
                .attr("Success", Value::Bool(true))
                .children(&[5]),
        )
        .node(
            node(4, "RollResult")
                .attr("Success", Value::Bool(false))
                .children(&[6]),
        )
        .node(node(5, "TagAnswer").speaker(0).text(5).children(&[7]))
        .node(node(6, "TagAnswer").speaker(0).text(6).children(&[7]))
        .node(
            node(7, "ActiveRoll")
                .speaker(1)
                .text(7)
                .attr("Ability", Value::String("Charisma".into()))
                .attr("Skill", Value::String("Persuasion".into()))
                .attr("RollType", Value::String("SkillCheck".into()))
                .attr("Advantage", Value::Byte(1))
                .children(&[8, 9]),
        )
        .node(
            node(8, "RollResult")
                .attr("Success", Value::Bool(true))
                .children(&[10]),
        )
        .node(
            node(9, "RollResult")
                .attr("Success", Value::Bool(false))
                .children(&[11]),
        )
        .node(node(10, "TagAnswer").speaker(0).text(10))
        .node(node(11, "TagAnswer").speaker(0).text(11))
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    // Passive rolls fail, active rolls succeed.
    w.set_roller(|req| RollOutcome {
        success: !req.passive,
        critical: None,
    });
    let mut r = start(d, &mut w);
    r.advance(&mut w).unwrap();
    assert_eq!(line_handle(r.step()), "h6");
    r.advance(&mut w).unwrap();
    match r.step() {
        Step::Choices(c) => {
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].roll.as_ref().unwrap().advantage, 1);
        }
        other => panic!("{other:?}"),
    }
    r.choose(&mut w, 0).unwrap();
    assert_eq!(line_handle(r.step()), "h10");
    let rolls: Vec<_> = r
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            DialogEvent::Roll {
                active,
                outcome,
                roller,
                target,
                ..
            } => Some((active, outcome.success, roller, target)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rolls,
        [
            (false, false, Some(g(PLAYER)), Some(g(NPC))),
            (true, true, Some(g(PLAYER)), None)
        ]
    );
}

#[test]
fn nested_dialog_links_speakers_and_returns() {
    let inner_flag = g(100);
    let inner = dialog(2)
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .text(21)
                .set("Object", &[(inner_flag, true, Some(0))]),
        )
        .roots(&[1])
        .build();
    let outer = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(node(1, "TagGreeting").speaker(0).text(1).children(&[2]))
        .node(
            node(2, "Nested Dialog")
                .attr("NestedDialogNodeUUID", Value::Uuid(g(2)))
                .children(&[3]),
        )
        .node(node(3, "TagAnswer").speaker(0).text(3))
        .roots(&[1])
        .build();
    // Link the nested dialog's slot 0 to our slot 1 (the player).
    let mut outer = outer;
    if let crate::NodeKind::NestedDialog {
        speaker_linking, ..
    } = &mut outer.nodes[1].kind
    {
        speaker_linking.push((0, 1));
    }
    let provider: HashMap<_, _> = [(inner.uuid, Arc::new(inner))].into();
    let mut w = MemoryWorld::new();
    w.define_flag(inner_flag, FlagType::Character);
    let mut r = DialogRunner::new(
        Arc::new(outer),
        Arc::new(provider),
        StartParams {
            instance: 3,
            speakers: vec![Some(g(NPC)), Some(g(PLAYER))],
        },
    );
    r.start(&mut w);
    r.advance(&mut w).unwrap();
    assert_eq!(line_handle(r.step()), "h21");
    assert_eq!(r.current_dialog().uuid, g(2));
    assert_eq!(flag_events(&mut r), [(inner_flag, Some(g(PLAYER)), true)]);
    r.advance(&mut w).unwrap();
    assert_eq!(line_handle(r.step()), "h3");
    assert_eq!(r.current_dialog().uuid, g(1));
}

#[test]
fn optional_and_exclusive() {
    let f = g(100);
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .text(1)
                .children(&[2, 3, 4]),
        )
        .node(node(2, "TagQuestion").speaker(1).text(2))
        .node(node(3, "TagQuestion").speaker(1).text(3).flag("exclusive"))
        .node(node(4, "TagQuestion").speaker(1).text(4).flag("exclusive"))
        // Root 5 fails but is optional: passed through to its children.
        .node(
            node(5, "TagGreeting")
                .speaker(0)
                .text(5)
                .flag("optional")
                .check("Global", &[(f, true, None)])
                .children(&[6]),
        )
        .node(node(6, "TagAnswer").speaker(0).text(6))
        .roots(&[5])
        .build();
    let mut w = MemoryWorld::new();
    w.define_flag(f, FlagType::Global);
    let r = start(d.clone(), &mut w);
    assert_eq!(line_handle(r.step()), "h6");

    let mut d = d;
    d.roots = vec![0];
    let mut r = start(d, &mut w);
    r.advance(&mut w).unwrap();
    assert_eq!(choice_handles(r.step()), ["h3", "h4"]);
}

#[test]
fn tag_rules_choose_the_text_and_gate_the_node() {
    let (elf, dwarf) = (g(200), g(201));
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .tagged_text(&[1], Some((0, vec![(0, 1, vec![elf])])))
                .tagged_text(&[2], Some((0, vec![(2, 1, vec![elf, dwarf])])))
                .tagged_text(&[3], Some((0, vec![(1, 1, vec![dwarf])]))),
        )
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    w.add_tag(g(PLAYER), elf);
    assert_eq!(line_handle(start(d.clone(), &mut w).step()), "h1");
    let mut w = MemoryWorld::new();
    assert_eq!(line_handle(start(d.clone(), &mut w).step()), "h2");
    let mut w = MemoryWorld::new();
    w.add_tag(g(PLAYER), elf);
    w.add_tag(g(PLAYER), dwarf);
    // Rule 1 matches first.
    assert_eq!(line_handle(start(d.clone(), &mut w).step()), "h1");

    // A node whose texts all fail is invalid.
    let only_dwarf = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .tagged_text(&[3], Some((0, vec![(0, 1, vec![dwarf])]))),
        )
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    assert!(start(only_dwarf, &mut w).is_ended());
}

#[test]
fn validated_flags_and_fallible_results() {
    let gold = g(300);
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(node(1, "TagGreeting").speaker(0).text(1).children(&[2, 3]))
        // Disabled unless the flag is set.
        .node(node(2, "TagQuestion").speaker(1).text(2).validated(
            true,
            "Global",
            &[(gold, true, None)],
        ))
        // Branches on the validation outcome: always selectable.
        .node(
            node(3, "TagQuestion")
                .speaker(1)
                .text(3)
                .validated(true, "Global", &[(gold, true, None)])
                .children(&[4, 5]),
        )
        .node(
            node(4, "FallibleQuestionResult")
                .attr("Result", Value::Bool(true))
                .children(&[6]),
        )
        .node(
            node(5, "FallibleQuestionResult")
                .attr("Result", Value::Bool(false))
                .children(&[7]),
        )
        .node(node(6, "TagAnswer").speaker(0).text(6))
        .node(node(7, "TagAnswer").speaker(0).text(7))
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    w.define_flag(gold, FlagType::Global);
    let mut r = start(d, &mut w);
    r.advance(&mut w).unwrap();
    match r.step() {
        Step::Choices(c) => assert_eq!(
            c.iter().map(|c| c.enabled).collect::<Vec<_>>(),
            [false, true]
        ),
        other => panic!("{other:?}"),
    }
    assert!(r.choose(&mut w, 0).is_err());
    r.choose(&mut w, 1).unwrap();
    assert_eq!(line_handle(r.step()), "h7");
}

#[test]
fn flag_storage_types_and_events() {
    let (local, global_as_object, user, unchanged, undefined) =
        (g(400), g(401), g(402), g(403), g(404));
    let d = dialog(1)
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(
            node(1, "TagGreeting")
                .speaker(0)
                .text(1)
                // Undefined flags fail checks whatever the expected value.
                .check("Global", &[(undefined, false, None)]),
        )
        .node(
            node(2, "TagGreeting")
                .speaker(0)
                .text(2)
                .set("Local", &[(local, true, None)])
                // A Global group whose flag resource is per-object: applied to the default
                // speaker (the GROUP_Players slot).
                .set("Global", &[(global_as_object, true, None)])
                .set("User", &[(user, true, Some(1))])
                .set("Global", &[(unchanged, false, None)]),
        )
        .roots(&[1, 2])
        .build();
    let mut w = MemoryWorld::new();
    w.define_flag(local, FlagType::Local);
    w.define_flag(global_as_object, FlagType::Character);
    w.define_flag(user, FlagType::User);
    w.define_flag(unchanged, FlagType::Global);
    let other_char = g(50);
    w.users.insert(g(PLAYER), 1);
    w.users.insert(other_char, 1);
    let mut r = start(d, &mut w);
    assert_eq!(line_handle(r.step()), "h2");
    assert_eq!(
        flag_events(&mut r),
        [
            (global_as_object, Some(g(PLAYER)), true),
            (user, Some(g(PLAYER)), true),
            (user, Some(other_char), true),
        ]
    );
    assert!(w.get_flag(&local, FlagType::Local, None));
}

#[test]
fn automated_dialogs_never_wait_for_input() {
    let d = dialog(1)
        .attr("automated", Value::Bool(true))
        .speaker(&[g(NPC)])
        .speaker(&[players_group()])
        .node(node(1, "TagGreeting").speaker(0).text(1).children(&[2, 3]))
        .node(node(2, "TagQuestion").speaker(1).text(2))
        .node(node(3, "TagQuestion").speaker(1).text(3))
        .roots(&[1])
        .build();
    let mut w = MemoryWorld::new();
    let mut r = start(d, &mut w);
    r.advance(&mut w).unwrap();
    // The first question is started like any node (and passes through).
    assert!(r.is_ended());
    assert!(r.take_events().iter().any(|e| matches!(
        e,
        DialogEvent::Started {
            automated: true,
            ..
        }
    )));
}
