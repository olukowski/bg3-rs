//! A live game on the sim (no trace): the start of a new campaign, trigger volumes and the
//! player's dialog request. Needs the game install; skips cleanly without it.

use std::sync::Arc;

use bg3_sim::Notification;
use bg3_sim::presentation::load_triggers;
use bg3_sim::start::{load_game, new_game};

fn vfs() -> Option<Arc<bg3_vfs::Vfs>> {
    match bg3_install::Install::locate() {
        Ok(i) => Some(Arc::new(bg3_vfs::Vfs::from_install(&i).expect("vfs"))),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

/// The trace reports the avatar entering these triggers when it lands on the start point
/// (`Teleported(…, 53.0581, 17.8543, -426.526)`, then settled at y 17.3832).
#[test]
#[ignore = "needs BG3 install"]
fn start_point_is_in_the_traced_triggers() {
    let Some(vfs) = vfs() else {
        return;
    };
    let triggers = load_triggers(&vfs, "TUT_Avernus_C");
    let start = triggers
        .iter()
        .find(|t| t.trigger_type == "TriggerStart" && t.name == "S_TUT_StartPoint_001")
        .expect("start trigger");
    assert_eq!(start.position, [53.058094, 17.85434, -426.5262]);
    for name in [
        "S_TUT_Misc_RubbleRoom_000",
        "S_TUT_MovementTutorialArea_001",
        "S_TUT_SaveTrigger_Start",
        "RoomTrigger_StartRoom_F0",
    ] {
        let t = triggers
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        for p in [[53.0581, 17.8543, -426.526], [53.0581, 17.3832, -426.526]] {
            assert!(t.contains(p), "{name} ({:?}) should contain {p:?}", t.shape);
        }
    }
}

/// New game → the intro dialog; walking into the rubble room → the devourer's ambient
/// dialog; clicking the devourer victim → the story starts its dialog.
#[test]
#[ignore = "needs BG3 install"]
fn new_game_plays_the_opening() {
    let Some(vfs) = vfs() else {
        return;
    };
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let (mut game, opts) = load_game(vfs).expect("load");
            let started = new_game(&mut game, &opts);
            assert_eq!(started.level, "TUT_Avernus_C");
            assert!(started.story_requested_level);
            let mut dialogs = Vec::new();
            let mut run = |game: &mut bg3_sim::Game, seconds: f64| {
                for _ in 0..(seconds * 30.0) as usize {
                    game.tick_live(1.0 / 30.0);
                    for n in game.take_notifications() {
                        match n {
                            Notification::DialogStarted { dialog, .. } => dialogs.push(dialog),
                            Notification::DialogLine { instance, .. } => {
                                game.dialog_advance(instance).unwrap()
                            }
                            Notification::DialogChoices { instance, choices } => {
                                let i = choices.iter().position(|c| c.enabled).unwrap_or(0);
                                game.dialog_choose(instance, i).unwrap();
                            }
                            _ => {}
                        }
                    }
                }
            };
            run(&mut game, 2.0);
            game.sim.set_position(started.avatar, [-3.4, 17.36, -422.8]);
            run(&mut game, 2.0);
            let victim = game
                .sim
                .entities
                .iter()
                .find(|e| e.name == "S_TUT_Lab_DevourerVictim")
                .expect("victim")
                .id;
            assert!(game.sim.request_dialog_with(victim, started.avatar));
            run(&mut game, 2.0);
            let has = |p: &str| dialogs.iter().any(|d| d.starts_with(p));
            assert!(has("TUT_Start_OriginIntroduction_Default_"), "{dialogs:?}");
            assert!(has("TUT_Lab_AD_DevourerVictim_"), "{dialogs:?}");
            assert!(has("TUT_Lab_TrappedDevourer_"), "{dialogs:?}");
        })
        .expect("spawn")
        .join()
        .expect("game thread");
}
