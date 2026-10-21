//! `bg3x combat`: run a fight headless on bg3-sim (story included) and print the log.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bg3_sim::combat::{AiAction, CombatEvent};
use bg3_sim::{Game, Guid, Sim};

pub struct Options {
    pub level: String,
    pub party: Vec<String>,
    pub target: String,
    pub seed: u64,
    pub max_turns: usize,
    pub events: bool,
}

fn find(sim: &Sim, s: &str) -> Option<Guid> {
    Guid::from_osiris(s)
        .filter(|g| sim.exists(*g))
        .or_else(|| sim.entities.iter().find(|e| e.name == s).map(|e| e.id))
}

fn name(sim: &Sim, g: Guid) -> String {
    sim.entity(g)
        .map_or_else(|| g.to_string(), |e| e.name.clone())
}

pub fn run(o: Options) -> Result<()> {
    let data = crate::sim::load_data(std::slice::from_ref(&o.level))?;
    let mut sim = Sim::with_data(Arc::new(data));
    sim.load_level(&o.level);
    sim.dice = bg3_sim::rules::SimDice::new(o.seed);
    let target = find(&sim, &o.target).with_context(|| format!("no object {}", o.target))?;
    let mut party = Vec::new();
    for p in &o.party {
        party.push(find(&sim, p).with_context(|| format!("no object {p}"))?);
    }
    if party.is_empty() {
        bail!("give the party with --party");
    }
    sim.party = party.clone();
    // Stand the party 3 m from the target, 1.5 m apart (positions are not part of the
    // story; the caller decides where the fight happens).
    let at = sim.entity(target).map(|e| e.position).unwrap_or_default();
    for (i, p) in party.iter().enumerate() {
        if let Some(e) = sim.entities.get_mut(*p) {
            e.position = [at[0] - 3.0, at[1], at[2] + 1.5 * i as f32];
        }
    }
    let bytes = crate::read_file("GustavX", "Mods/GustavX/Story/story.div.osi")?;
    let events = o.events;
    let level = o.level.clone();
    let run = move || -> Result<()> {
        // The story runs on a big stack (deep rule nesting), like the replay tools.
        let story = bg3_story::Story::parse(&bytes)?;
        let mut game = Game::new(&story, sim);
        game.init();
        // The new-game events of the opening up to the level start (as in the reference
        // trace), so the level's goals are active (e.g. TUT_LowerDeck's INIT).
        game.start_level(&level);
        game.set_companion_factions(&party);
        let off: Vec<String> = game
            .sim
            .combat
            .off_stage
            .iter()
            .map(|g| name(&game.sim, *g))
            .collect();
        if !off.is_empty() {
            println!("off stage: {}", off.join(", "));
        }
        for p in &party {
            game.sim.ensure_combatant(*p);
            let s = &game.sim;
            let c = s.combatant(*p).cloned();
            let sheet = s.entity(*p).and_then(|e| e.sheet.clone());
            match (c, sheet) {
                (Some(c), Some(sh)) => println!(
                    "{} ({}): HP {}/{} AC {} resources {:?} equipment {:?} spells {:?}",
                    name(s, *p),
                    s.entity(*p)
                        .and_then(|e| e.stats.clone())
                        .unwrap_or_default(),
                    sh.hp,
                    sh.max_hp,
                    sh.armor_class,
                    c.resources
                        .entries
                        .iter()
                        .map(|r| format!("{}:{}", r.name, r.max))
                        .collect::<Vec<_>>(),
                    c.equipment,
                    c.spells
                ),
                _ => println!("{}: no combat stats", name(s, *p)),
            }
        }
        let cid = game
            .enter_combat(party[0], target)
            .context("the fight did not start")?;
        let order: Vec<String> = game
            .sim
            .combat_by_id(cid)
            .map(|c| {
                c.order
                    .iter()
                    .map(|g| {
                        g.members
                            .iter()
                            .map(|m| name(&game.sim, *m))
                            .collect::<Vec<_>>()
                            .join(" + ")
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ev = game.take_combat_events();
        print_events(&game.sim, ev, events);
        println!("turn order: {}", order.join(" | "));
        let turns = game.combat_auto(cid, o.max_turns);
        for (who, a) in &turns {
            if let AiAction::Nothing = a {
                println!("({} does nothing)", name(&game.sim, *who));
            }
        }
        let ev = game.take_combat_events();
        print_events(&game.sim, ev, events);
        let done = game.sim.combat_by_id(cid).is_some_and(|c| c.finished);
        println!(
            "{} after {} turns",
            if done {
                "fight over"
            } else {
                "fight still running"
            },
            turns.len()
        );
        if let Some(flag) = game
            .sim
            .data
            .flags
            .iter()
            .find(|f| f.name == "TUT_LowerDeck_State_ImpsDead")
        {
            let set = game.sim.get_flag(flag.id, Guid::NULL).unwrap_or(false);
            println!("story: TUT_LowerDeck_State_ImpsDead = {set}");
        }
        Ok(())
    };
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(run)?
        .join()
        .map_err(|_| anyhow::anyhow!("combat thread panicked"))?
}

fn print_events(sim: &Sim, events: Vec<CombatEvent>, verbose: bool) {
    let n = |g: &Guid| name(sim, *g);
    for e in events {
        match e {
            CombatEvent::CombatStarted { participants, .. } => println!(
                "combat starts: {}",
                participants.iter().map(n).collect::<Vec<_>>().join(", ")
            ),
            CombatEvent::Initiative {
                entity,
                die,
                faces,
                bonus,
                total,
            } => println!(
                "  initiative {}: d{faces}={die} {bonus:+} = {total}",
                n(&entity)
            ),
            CombatEvent::RoundStarted { round, .. } => println!("-- round {round}"),
            CombatEvent::TurnStarted { entity } => println!("{}'s turn", n(&entity)),
            CombatEvent::Moved {
                entity,
                distance,
                movement_left,
            } => println!(
                "  {} moves {distance:.1} m ({movement_left:.1} m left)",
                n(&entity)
            ),
            CombatEvent::SpellCast {
                caster,
                spell,
                target,
            } => println!("  {} uses {spell} on {}", n(&caster), n(&target)),
            CombatEvent::Roll { text, .. } => println!("    {text}"),
            CombatEvent::Damage {
                target,
                amount,
                damage_type,
                hp,
                max_hp,
                critical,
                ..
            } => println!(
                "    {} takes {amount} {damage_type}{} -> {hp}/{max_hp}",
                n(&target),
                if critical { " (critical)" } else { "" }
            ),
            CombatEvent::Downed { entity } => println!("    {} is downed", n(&entity)),
            CombatEvent::Died { entity } => println!("    {} dies", n(&entity)),
            CombatEvent::DeathSave {
                entity,
                roll,
                successes,
                failures,
            } => println!(
                "  {} death save d20={roll} ({successes} successes, {failures} failures)",
                n(&entity)
            ),
            CombatEvent::CombatEnded { .. } => println!("combat ends"),
            CombatEvent::Log(l) if verbose => println!("    | {l}"),
            CombatEvent::StatusApplied { target, status } if verbose => {
                println!("    {} gains {status}", n(&target))
            }
            _ => {}
        }
    }
}
