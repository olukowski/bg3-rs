//! `bg3x play`: start a new game on the sim headlessly (the viewer's `--play` without
//! graphics) and print what the player would see: dialog lines, choices, rolls, and the
//! characters that change stage or position. Lines advance at once; choices take the first
//! enabled option (or `--choices`). Output includes game text: keep it local.

use std::sync::Arc;

use anyhow::Result;
use bg3_install::Install;
use bg3_sim::start::{load_game, new_game};
use bg3_sim::{Guid, Notification, Presence};

fn parse_vec3(s: &str) -> Result<[f32; 3], String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|x| x.trim().parse().map_err(|e| format!("{x}: {e}")))
        .collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "expected x,y,z".to_owned())
}

pub fn vec3_arg(s: &str) -> Result<[f32; 3], String> {
    parse_vec3(s)
}

pub struct Args {
    pub seconds: f64,
    pub choices: Vec<usize>,
    pub talk: Option<String>,
    pub move_to: Vec<[f32; 3]>,
    pub quiet: bool,
    pub triggers: Option<String>,
}

pub fn run(a: Args) -> Result<()> {
    let install = Install::locate()?;
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install)?);
    let loca = crate::dialog::load_loca(&vfs, "English")?;
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || -> Result<()> {
            let t = std::time::Instant::now();
            let (mut game, opts) = load_game(vfs).map_err(|e| anyhow::anyhow!("{e}"))?;
            eprintln!(
                "loaded in {:.1?}; {} triggers in the startup level",
                t.elapsed(),
                opts.startup_triggers.len()
            );
            if let Some(f) = &a.triggers {
                for t in opts
                    .startup_triggers
                    .iter()
                    .filter(|t| t.name.contains(f.as_str()))
                {
                    println!(
                        "trigger {} ({}) at {:?}: {:?}",
                        t.name, t.trigger_type, t.position, t.shape
                    );
                }
            }
            let started = new_game(&mut game, &opts);
            println!(
                "started: {} in {} ({} events; level requested by the story: {})",
                game.sim.render(started.avatar),
                started.level,
                started.events,
                started.story_requested_level
            );
            let off: Vec<String> = game
                .sim
                .presences()
                .into_iter()
                .filter(|p| !p.on_stage)
                .map(|p| p.name)
                .collect();
            println!("off stage: {}", off.join(", "));
            for p in game.sim.presences() {
                if p.name.contains("Player") {
                    println!(
                        "  {} at {:?} (on stage {}, visible {})",
                        p.name, p.position, p.on_stage, p.visible
                    );
                }
            }
            let mut choices = a.choices.iter().copied();
            let mut before = snapshot(&game.sim);
            let mut talked = false;
            let mut moves = a.move_to.iter();
            let dt = 1.0 / 30.0;
            let mut now = 0.0;
            while now < a.seconds {
                game.tick_live(dt);
                now += dt;
                for n in game.take_notifications() {
                    print_notification(&game.sim, &loca, &n);
                    match n {
                        Notification::DialogLine { instance, .. } => {
                            let _ = game.dialog_advance(instance);
                        }
                        Notification::DialogChoices {
                            instance,
                            choices: c,
                        } => {
                            let pick = choices
                                .next()
                                .map(|i| i.saturating_sub(1))
                                .or_else(|| c.iter().position(|c| c.enabled))
                                .unwrap_or(0);
                            println!("  -> choose {}", pick + 1);
                            let _ = game.dialog_choose(instance, pick);
                        }
                        _ => {}
                    }
                }
                let after = snapshot(&game.sim);
                if !a.quiet {
                    diff(&before, &after);
                }
                before = after;
                let busy = game
                    .sim
                    .dialogs
                    .instances
                    .iter()
                    .any(|d| d.state != bg3_sim::InstanceState::Ended);
                if !busy && now > 1.0 {
                    if let Some(p) = moves.next() {
                        println!("[{now:.1}s] avatar moved to {p:?}");
                        game.sim.set_position(started.avatar, *p);
                    } else if let Some(name) = a.talk.as_deref().filter(|_| !talked) {
                        talked = true;
                        let target = game
                            .sim
                            .entities
                            .iter()
                            .find(|e| e.name.contains(name))
                            .map(|e| e.id);
                        match target {
                            Some(t) => {
                                println!("[{now:.1}s] talk to {}", game.sim.render(t));
                                game.sim.request_dialog_with(t, started.avatar);
                            }
                            None => println!("no entity named like {name}"),
                        }
                    }
                }
            }
            let mut missing: Vec<_> = game.sim.stage.missing.iter().collect();
            missing.sort_by(|a, b| b.1.cmp(a.1));
            eprintln!(
                "{} functions the story used are not implemented; most used:",
                missing.len()
            );
            for (name, n) in missing.iter().take(40) {
                eprintln!("  {n:>6}  {name}");
            }
            Ok(())
        })?
        .join()
        .map_err(|_| anyhow::anyhow!("play thread panicked"))?
}

fn snapshot(sim: &bg3_sim::Sim) -> std::collections::HashMap<Guid, Presence> {
    sim.presences().into_iter().map(|p| (p.id, p)).collect()
}

fn diff(
    before: &std::collections::HashMap<Guid, Presence>,
    after: &std::collections::HashMap<Guid, Presence>,
) {
    for (id, p) in after {
        match before.get(id) {
            None => println!("  + {} at {:?}", p.name, p.position),
            Some(b) if b.on_stage != p.on_stage || b.visible != p.visible => println!(
                "  ~ {}: on stage {}, visible {}",
                p.name, p.on_stage, p.visible
            ),
            Some(b) if b.position != p.position && !p.moving => {
                println!("  ~ {} moved to {:?}", p.name, p.position)
            }
            _ => {}
        }
    }
}

fn print_notification(sim: &bg3_sim::Sim, loca: &bg3_loca::Loca, n: &Notification) {
    let text = |t: &Option<bg3_dialog::model::TextLine>| {
        t.as_ref()
            .and_then(|t| loca.text(&t.handle))
            .unwrap_or("")
            .to_owned()
    };
    match n {
        Notification::DialogStarted { instance, dialog } => {
            println!("dialog {instance} started: {dialog}")
        }
        Notification::DialogLine { line, .. } => {
            let who = line
                .actor
                .map(|a| sim.render(bg3_sim::dialog::from_lsf(&a)))
                .unwrap_or_default();
            println!("  {who}: {}", text(&line.text));
        }
        Notification::DialogChoices { choices, .. } => {
            for (i, c) in choices.iter().enumerate() {
                let roll = c
                    .roll
                    .as_ref()
                    .map(|r| format!("[{} {}] ", r.ability, r.skill))
                    .unwrap_or_default();
                println!("  {}. {roll}{}", i + 1, text(&c.text));
            }
        }
        Notification::DialogRoll { roll, .. } => match &roll.check {
            Some(c) => println!("  roll: {c}"),
            None => println!(
                "  roll: {}",
                if roll.success { "success" } else { "failure" }
            ),
        },
        Notification::TradeRequested { instance } => println!("dialog {instance}: trade"),
        Notification::DialogEnded { instance } => println!("dialog {instance} ended"),
    }
}
