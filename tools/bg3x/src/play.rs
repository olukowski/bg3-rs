//! `bg3x play`: start a new game on the sim headlessly (the viewer's `--play` without
//! graphics) and print what the player would see: dialog lines, choices, rolls, and the
//! characters that change stage or position. Lines advance at once; choices take the first
//! enabled option (or `--choices`). Output includes game text: keep it local.

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::Result;
use bg3_install::Install;
use bg3_nav::NavGrid;
use bg3_sim::combat::Pathfinder;
use bg3_sim::start::{load_game, new_game};
use bg3_sim::{Game, Guid, Notification, Presence};
use bg3_sim_nav::GridPathfinder;

/// The avatar's walking speed between `--move-to` points: the default `MovementSpeedRun`
/// (the viewer's click-to-move uses the template's, which is the default for the avatar).
const RUN_SPEED: f32 = bg3_world::characters::DEFAULT_MOVEMENT_SPEED_RUN;
/// Combat turns played per tick (one per 1/30 s keeps the output in game-time order).
const TURNS_PER_TICK: usize = 1;

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
    pub teleport: bool,
    pub no_combat: bool,
    pub combat_log: bool,
}

pub fn run(a: Args) -> Result<()> {
    let install = Install::locate()?;
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install)?);
    let loca = crate::dialog::load_loca(&vfs, "English")?;
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || -> Result<()> {
            let t = std::time::Instant::now();
            let (mut game, opts) = load_game(vfs.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
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
            // The AI grid before the new game, like the viewer: scripted moves, the fights'
            // movement and line of sight use it.
            let grid = load_grid(&vfs, &game.sim.data.module_levels.startup.clone());
            if let Some(g) = &grid {
                game.sim
                    .set_pathfinder(Some(Arc::new(GridPathfinder::new(g.clone()))));
            }
            let started = new_game(&mut game, &opts);
            if let Some(g) = &grid {
                game.sim
                    .set_surface_grid(Some(Arc::new(bg3_sim_nav::GridSurfaces::new(g.clone()))));
            }
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
            let mut walk: Option<Walk> = None;
            let mut fights = Fights::default();
            let dt = 1.0 / 30.0;
            let mut now = 0.0;
            while now < a.seconds {
                game.tick_live(dt);
                now += dt;
                let fighting = if a.no_combat {
                    false
                } else {
                    fights.tick(&mut game, grid.as_deref(), now, a.combat_log)
                };
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
                let busy = fighting
                    || game
                        .sim
                        .dialogs
                        .instances
                        .iter()
                        .any(|d| d.state != bg3_sim::InstanceState::Ended);
                if busy || now <= 1.0 {
                    continue;
                }
                if let Some(w) = &mut walk {
                    // The story may have moved the avatar (a teleport): walk on from there.
                    let at = game.sim.entity(started.avatar).map(|e| e.position);
                    match at.and_then(|p| w.step(p, RUN_SPEED * dt as f32)) {
                        Some(p) => {
                            game.sim.set_position(started.avatar, p);
                        }
                        None => {
                            println!("[{now:.1}s] avatar arrived at {:?}", w.target);
                            walk = None;
                        }
                    }
                } else if let Some(p) = moves.next() {
                    let from = game.sim.entity(started.avatar).map_or(*p, |e| e.position);
                    match plan_walk(grid.as_ref(), from, *p, a.teleport) {
                        Ok(points) => {
                            let length = path_length(&points);
                            println!(
                                "[{now:.1}s] avatar walks to {p:?} ({length:.1} m, {} waypoints)",
                                points.len()
                            );
                            walk = Some(Walk {
                                target: *p,
                                points: points.into(),
                            });
                        }
                        Err(why) => {
                            if a.teleport {
                                println!("[{now:.1}s] avatar moved to {p:?}");
                            } else {
                                println!(
                                    "[{now:.1}s] no path from {from:?} to {p:?} ({why}); teleporting"
                                );
                            }
                            game.sim.set_position(started.avatar, *p);
                        }
                    }
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
            fights.summary(&game);
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

/// The level's AI grid, or `None` (with a note) when it can't be read: then moves teleport
/// and every hostile in range can see the party.
fn load_grid(vfs: &bg3_vfs::Vfs, level: &str) -> Option<Arc<NavGrid>> {
    match bg3_nav::load_aigrid(vfs, level) {
        Ok(f) => Some(Arc::new(NavGrid::new(f))),
        Err(e) => {
            eprintln!("no AI grid for {level}: {e}");
            None
        }
    }
}

/// The AI grid path for a `--move-to`, or why there is none. The goal is the nearest tile
/// a humanoid can stand on (within 3 m vertically and 8 m sideways): trigger centres sit
/// above the floor, some over ledges or blocked tiles.
fn plan_walk(
    grid: Option<&Arc<NavGrid>>,
    from: [f32; 3],
    to: [f32; 3],
    teleport: bool,
) -> Result<Vec<[f32; 3]>, String> {
    if teleport {
        return Err("--teleport".into());
    }
    let grid = grid.ok_or("no AI grid")?;
    let agent = bg3_nav::Agent::humanoid();
    let goal = grid
        .project_standable(to, 3.0, 16, &agent)
        .map(|t| grid.world_pos(t))
        .ok_or("no standable tile near the goal")?;
    GridPathfinder::new(grid.clone())
        .find_path(from, goal, bg3_sim::combat::path::APPROXIMATE_RADIUS)
        .ok_or_else(|| match grid.find_path(from, goal, &agent) {
            Err(e) => e.to_string(),
            Ok(_) => "the start isn't standable".to_owned(),
        })
}
/// A `--move-to` in progress: the rest of the AI grid path.
struct Walk {
    target: [f32; 3],
    points: VecDeque<[f32; 3]>,
}

impl Walk {
    /// Move `step` metres along the path from `at`; `None` once the path is done.
    fn step(&mut self, mut at: [f32; 3], mut step: f32) -> Option<[f32; 3]> {
        let mut moved = false;
        while let Some(&next) = self.points.front() {
            let d = bg3_sim::combat::distance(at, next);
            if d > step {
                let k = step / d;
                return Some(std::array::from_fn(|i| at[i] + (next[i] - at[i]) * k));
            }
            step -= d;
            at = next;
            moved = true;
            self.points.pop_front();
        }
        // The last waypoint reached in this step: stand on it; arrival shows next time.
        moved.then_some(at)
    }
}

fn path_length(points: &[[f32; 3]]) -> f32 {
    points
        .windows(2)
        .map(|w| bg3_sim::combat::distance(w[0], w[1]))
        .sum()
}

/// Fights in a headless game, like the viewer's `--play` with `--combat-autoplay`: hostiles
/// that notice the party (distance and AI-grid line of sight) start a fight, and every side,
/// the party included, plays on the AI.
#[derive(Default)]
struct Fights {
    current: Option<Guid>,
    started: Vec<(Guid, f64)>,
    turns: usize,
}

impl Fights {
    /// After the game's tick: start a fight if hostiles notice the party, play turns of the
    /// party's fight and print what happened. Returns whether a fight is running.
    fn tick(&mut self, game: &mut Game, grid: Option<&NavGrid>, now: f64, verbose: bool) -> bool {
        if self.current.is_none() {
            let sight =
                |a: [f32; 3], b: [f32; 3]| grid.is_none_or(|g| bg3_sim_nav::line_of_sight(g, a, b));
            if let Some(cid) = game.start_combat_on_sight_with(sight) {
                println!("[{now:.1}s] hostiles noticed the party: fight {cid}");
            }
            self.current = game
                .sim
                .party
                .clone()
                .into_iter()
                .find_map(|p| game.sim.combat_of(p).filter(|c| !c.finished).map(|c| c.id));
            if let Some(cid) = self.current
                && !self.started.iter().any(|(c, _)| *c == cid)
            {
                self.started.push((cid, now));
            }
        }
        if let Some(cid) = self.current {
            self.turns += game.combat_auto(cid, TURNS_PER_TICK).len();
        }
        let events = game.take_combat_events();
        if !events.is_empty() {
            crate::combat::print_events(&game.sim, events, verbose);
        }
        if let Some(cid) = self.current
            && game.sim.combat_by_id(cid).is_none_or(|c| c.finished)
        {
            println!("[{now:.1}s] fight {cid} over");
            self.current = None;
        }
        self.current.is_some()
    }

    fn summary(&self, game: &Game) {
        if self.started.is_empty() {
            return;
        }
        println!(
            "{} fight(s), {} turns; party: {}",
            self.started.len(),
            self.turns,
            game.sim
                .party
                .iter()
                .map(|&p| {
                    let hp = game
                        .sim
                        .entity(p)
                        .and_then(|e| e.sheet.as_ref())
                        .map_or_else(String::new, |s| format!(" {}/{}", s.hp, s.max_hp));
                    format!(
                        "{}{hp}",
                        game.sim
                            .display_name(p)
                            .unwrap_or_else(|| game.sim.render(p))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(points: &[[f32; 3]]) -> Walk {
        Walk {
            target: *points.last().unwrap(),
            points: points.iter().copied().collect(),
        }
    }

    #[test]
    fn walk_steps_along_the_path_and_turns_corners() {
        let mut w = walk(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 0.0, 2.0]]);
        let at = w.step([0.0, 0.0, 0.0], 1.0).unwrap();
        assert_eq!(at, [1.0, 0.0, 0.0]);
        // 1 m to the corner, 1 m beyond it.
        let at = w.step(at, 2.0).unwrap();
        assert_eq!(at, [2.0, 0.0, 1.0]);
        // The last metre lands on the end point, the next step reports arrival.
        let at = w.step(at, 5.0).unwrap();
        assert_eq!(at, [2.0, 0.0, 2.0]);
        assert_eq!(w.step(at, 1.0), None);
    }

    #[test]
    fn path_length_sums_segments() {
        let p = [[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 4.0, 0.0]];
        assert_eq!(path_length(&p), 7.0);
        assert_eq!(path_length(&p[..1]), 0.0);
    }
}
