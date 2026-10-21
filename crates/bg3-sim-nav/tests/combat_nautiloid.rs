//! Read-only install integration: the lower deck fight with AI-grid movement.
use std::sync::Arc;

use bg3_sim::combat::path::APPROXIMATE_RADIUS;
use bg3_sim::{Game, GameData, Guid, Sim};
use bg3_sim_nav::GridPathfinder;

const LEVEL: &str = "TUT_Avernus_C";

fn find(sim: &Sim, name: &str) -> Guid {
    sim.entities
        .iter()
        .find(|e| e.name == name)
        .expect("fight character")
        .id
}

#[test]
#[ignore = "needs BG3 install"]
fn nautiloid_nav_endpoints_are_standable_and_fight_finishes() {
    let install = match bg3_install::Install::locate() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let vfs = bg3_vfs::Vfs::from_install(&install).expect("vfs");
            let grid = bg3_nav::NavGrid::new(bg3_nav::load_aigrid(&vfs, LEVEL).expect("grid"));
            let nav = Arc::new(GridPathfinder::new(Arc::new(grid)));
            let paks = bg3_vfs::open_paks(&install).expect("paks");
            let stats = bg3_stats::install::load(&paks, bg3_stats::install::BASE_LOAD_ORDER)
                .expect("stats");
            let data = GameData::load(&vfs, &stats, &[LEVEL]).expect("data");
            let bytes = vfs.read("Mods/GustavX/Story/story.div.osi").expect("story");
            let story = bg3_story::Story::parse(&bytes).expect("parse story");
            let mut sim = Sim::with_data(Arc::new(data));
            sim.load_level(LEVEL);
            sim.set_pathfinder(Some(nav.clone()));
            sim.dice = bg3_sim::rules::SimDice::new(0);
            let party = vec![
                find(&sim, "S_Player_Laezel"),
                find(&sim, "S_Player_ShadowHeart"),
            ];
            let target = find(&sim, "S_FirstFight_Imp_Melee_01");
            let at = sim.entity(target).unwrap().position;
            sim.party = party.clone();
            for (i, p) in party.iter().enumerate() {
                sim.entities.get_mut(*p).unwrap().position =
                    [at[0] - 3.0, at[1], at[2] + 1.5 * i as f32];
            }
            let mut game = Game::new(&story, sim);
            game.init();
            game.start_level(LEVEL);
            game.set_companion_factions(&party);
            let cid = game.enter_combat(party[0], target).expect("combat");
            let mut moves = 0;
            for _ in 0..200 {
                let turns = game.combat_auto(cid, 1);
                if turns.is_empty() {
                    break;
                }
                for (who, action) in turns {
                    if let Some(&end) = action.path().last() {
                        moves += 1;
                        assert_eq!(end, game.sim.entity(who).unwrap().position);
                        assert!(
                            nav.is_standable(end, APPROXIMATE_RADIUS),
                            "{} ended at {end:?}",
                            game.sim.render(who)
                        );
                    }
                }
            }
            assert!(moves > 0, "exercise navigation");
            assert!(
                game.sim.combat_by_id(cid).unwrap().finished,
                "fight must finish"
            );
            eprintln!("{moves} AI moves, all endpoints standable");
        })
        .expect("spawn")
        .join()
        .expect("fight thread");
}
