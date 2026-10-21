//! The imps' decisions of the reference combat trace (combat-20261021-183321, Balanced)
//! against the scoring AI, given the trace's state at each decision.
//!
//! The fight is reached the live way (as `bg3-sim`'s `live_imp_fight`): the avatar walks
//! into the lower deck, Lae'zel joins, the story teleports the party into the imp room and
//! the imps notice it. Then the trace's party actions are applied with their outcomes (no
//! dice) and the AI is asked what each imp does:
//!
//! 1. round 1, `S_FirstFight_Imp_Melee_02` dead, nobody hurt: the ranged imp used
//!    `Projectile_Generic_Imp` on Lae'zel;
//! 2. Lae'zel at 10/12: `S_FirstFight_Imp_Melee_01` used `Target_MainHandAttack` on the
//!    avatar;
//! 3. round 2, the ranged imp dead, Melee_01 at 3/6 after Sacred Flame:
//!    `Target_MainHandAttack` on Lae'zel.
//!
//! Positions after the party moved are not in the trace (it has no positions after the
//! teleport); the party stands next to the imp it attacked, as melee attackers must. Ties
//! between targets are common (the imps' archetypes neutralise most target multipliers and
//! believe every attack hits), so the check is that the trace's choice is among the
//! best-scored candidates. Needs the game install; skips cleanly without it.

use std::sync::Arc;

use bg3_sim::combat::ai::AiPlan;
use bg3_sim::start::{load_game, new_game};
use bg3_sim::{Game, Guid, Notification};
use bg3_sim_nav::GridPathfinder;

const LEVEL: &str = "TUT_Avernus_C";

#[test]
#[ignore = "needs BG3 install"]
fn imp_decisions_match_the_reference_trace() {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || run(install))
        .expect("spawn")
        .join()
        .expect("game thread");
}

fn find(game: &Game, name: &str) -> Guid {
    game.sim
        .entities
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.id)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// Stand `who` `gap` metres from `target`, turned `angle` degrees from the side facing
/// where `who` stands now.
fn place(game: &mut Game, who: Guid, target: Guid, gap: f32, angle: f32) {
    let t = game.sim.entity(target).unwrap().position;
    let w = game.sim.entity(who).unwrap().position;
    let d = ((w[0] - t[0]).powi(2) + (w[2] - t[2]).powi(2))
        .sqrt()
        .max(1e-3);
    let (dx, dz) = ((w[0] - t[0]) / d, (w[2] - t[2]) / d);
    let (s, c) = angle.to_radians().sin_cos();
    let p = [
        t[0] + (dx * c - dz * s) * gap,
        t[1],
        t[2] + (dx * s + dz * c) * gap,
    ];
    game.sim.set_position(who, p);
}

/// One decision: the trace's (spell, target) against the plan.
struct Outcome {
    /// The chosen candidate has the trace's spell.
    spell: bool,
    /// The trace's target is among the best-scored candidates.
    target_tied: bool,
    /// Our deterministic tie-break picks the trace's target.
    target_chosen: bool,
}

fn compare(game: &Game, plan: &AiPlan, spell: &str, target: Guid) -> Outcome {
    eprintln!(
        "{} ({}, average damage {}):",
        game.sim.render(plan.actor),
        plan.archetype,
        plan.average_damage
    );
    for c in plan.candidates.iter().take(6) {
        eprintln!(
            "  {:8.2} {} -> {} walk {:.1} chance {:.2} believed {:.2}",
            c.score,
            c.spell,
            game.sim.render(c.target),
            c.tile.cost,
            c.chance,
            c.believed
        );
    }
    let chosen = plan.chosen().expect("the imp acts");
    let best = chosen.score;
    Outcome {
        spell: chosen.spell == spell,
        target_tied: plan
            .candidates
            .iter()
            .any(|c| c.target == target && c.spell == spell && c.score + 1e-3 >= best),
        target_chosen: chosen.target == target,
    }
}

fn run(install: bg3_install::Install) {
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install).expect("vfs"));
    let grid = bg3_nav::NavGrid::new(bg3_nav::load_aigrid(&vfs, LEVEL).expect("grid"));
    let (mut game, opts) = load_game(vfs).expect("load");
    let started = new_game(&mut game, &opts);
    let avatar = started.avatar;
    let laezel = find(&game, "S_Player_Laezel");
    let trigger = opts
        .startup_triggers
        .iter()
        .find(|t| t.name == "S_TUT_LowerDeck_GuideDialogTrigger")
        .expect("guide trigger")
        .position;
    let mut fight = None;
    for step in 0..400 {
        if step == 40 {
            game.sim.set_position(avatar, trigger);
        }
        game.tick_live(0.1);
        if fight.is_none() {
            fight = game.start_combat_on_sight();
        }
        for n in game.take_notifications() {
            match n {
                Notification::DialogLine { instance, .. } => {
                    game.dialog_advance(instance).ok();
                }
                Notification::DialogChoices { instance, choices } => {
                    let i = choices.iter().rposition(|c| c.enabled).unwrap_or(0);
                    game.dialog_choose(instance, i).ok();
                }
                _ => {}
            }
        }
        if fight.is_some() {
            break;
        }
    }
    fight.expect("the imps start the fight");
    game.sim
        .set_pathfinder(Some(Arc::new(GridPathfinder::new(Arc::new(grid)))));
    let melee1 = find(&game, "S_FirstFight_Imp_Melee_01");
    let melee2 = find(&game, "S_FirstFight_Imp_Melee_02");
    let ranged1 = find(&game, "S_FirstFight_Imp_Ranged_01");
    for g in [avatar, laezel, melee1, melee2, ranged1] {
        eprintln!(
            "{} at {:?} archetype {}",
            game.sim.render(g),
            game.sim.entity(g).unwrap().position,
            game.sim.ai_modifiers(g).archetype
        );
    }

    // Round 1: the avatar hits Melee_02 (3), Lae'zel kills it; both stand next to it.
    place(&mut game, avatar, melee2, 1.3, 0.0);
    place(&mut game, laezel, melee2, 1.3, 70.0);
    game.sim.kill(melee2);
    let mut outcomes = Vec::new();
    let plan = game.sim.ai_plan(ranged1).expect("ranged imp plan");
    outcomes.push(compare(&game, &plan, "Projectile_Generic_Imp", laezel));

    // The ranged imp hits Lae'zel for 2 fire; Melee_01's turn.
    game.sim.set_hitpoints(laezel, 10);
    let plan = game.sim.ai_plan(melee1).expect("melee imp plan");
    outcomes.push(compare(&game, &plan, "Target_MainHandAttack", avatar));

    // Melee_01 walked to the avatar and missed. Round 2 (screenshot 18.40.28): Sacred Flame
    // on Melee_01 (3/6); Lae'zel kills the ranged imp; Melee_01 stands a couple of metres
    // from the avatar, Lae'zel some 6 m off (estimated from the screenshot; the ranged imp's
    // own position at that point is not in the trace).
    place(&mut game, melee1, avatar, 2.5, 0.0);
    game.sim.set_hitpoints(melee1, 3);
    game.sim.kill(ranged1);
    place(&mut game, laezel, melee1, 6.0, 120.0);
    let plan = game.sim.ai_plan(melee1).expect("melee imp plan");
    outcomes.push(compare(&game, &plan, "Target_MainHandAttack", laezel));

    let n = |f: fn(&Outcome) -> bool| outcomes.iter().filter(|o| f(o)).count();
    eprintln!(
        "trace decisions: spell {}/3, target among the best {}/3, target chosen {}/3",
        n(|o| o.spell),
        n(|o| o.target_tied),
        n(|o| o.target_chosen)
    );
    assert_eq!(n(|o| o.spell), 3, "the imps' attack spells");
    assert_eq!(n(|o| o.target_tied), 3, "the trace's targets score best");
}
