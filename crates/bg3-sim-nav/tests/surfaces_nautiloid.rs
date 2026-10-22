//! Surfaces on the Nautiloid's AI grid (`docs/research/surfaces.md`): the grid's painted blood
//! becomes the level's surfaces, a killed imp bleeds a 10-cell puddle (its template's death
//! action), and a slashing hit sprays blood beyond the target, away from the attacker.
//! Needs the game install; skips cleanly without it.

use std::sync::Arc;

use bg3_sim::start::{load_game, new_game};
use bg3_sim::surface::{BLOOD, SurfaceLayer};
use bg3_sim::{Game, Guid};
use bg3_sim_nav::GridSurfaces;

const LEVEL: &str = "TUT_Avernus_C";

#[test]
#[ignore = "needs BG3 install"]
fn nautiloid_blood() {
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

fn blood_near(game: &Game, p: [f32; 3], r: f32) -> usize {
    game.sim
        .surface_cells(SurfaceLayer::Ground)
        .into_iter()
        .filter(|(c, ty)| {
            *ty == BLOOD && (c[0] - p[0]).hypot(c[2] - p[2]) <= r && (c[1] - p[1]).abs() < 1.5
        })
        .count()
}

fn run(install: bg3_install::Install) {
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install).expect("vfs"));
    let grid = Arc::new(bg3_nav::NavGrid::new(
        bg3_nav::load_aigrid(&vfs, LEVEL).expect("aigrid"),
    ));
    let (mut game, opts) = load_game(vfs).expect("load");
    new_game(&mut game, &opts);
    game.sim
        .set_surface_grid(Some(Arc::new(GridSurfaces::new(grid))));

    // Painted blood: around the helm's corpses, not on the lower deck.
    let painted = game
        .sim
        .surface_cells(SurfaceLayer::Ground)
        .into_iter()
        .filter(|(_, ty)| *ty == BLOOD)
        .count();
    assert!(
        (40..=60).contains(&painted),
        "{painted} painted blood cells"
    );
    let helm_imp = find(&game, "Helm_Imp_Dead_03");
    let at = game.sim.entity(helm_imp).unwrap().position;
    assert!(blood_near(&game, at, 6.0) > 0);

    // A killed imp bleeds its death puddle (BASE_Bipedal: SurfaceBlood, 10 cells).
    let imp = find(&game, "S_FirstFight_Imp_Melee_01");
    let at = game.sim.entity(imp).unwrap().position;
    let before = blood_near(&game, at, 3.0);
    game.sim.kill(imp);
    for _ in 0..30 {
        game.sim.update_surfaces(0.1);
    }
    let after = blood_near(&game, at, 3.0);
    assert_eq!(after - before, 10, "death puddle at {at:?}");

    // A slashing hit sprays away from the attacker.
    let target = find(&game, "S_FirstFight_Imp_Melee_02");
    let attacker = find(&game, "S_Player_Laezel");
    let t = game.sim.entity(target).unwrap().position;
    let from = [t[0] - 1.5, t[1], t[2]];
    game.sim.set_position(attacker, from);
    let cells_before: Vec<_> = game.sim.surface_cells(SurfaceLayer::Ground);
    game.sim.hit_blood(attacker, target, "Slashing", 5);
    game.sim.update_surfaces(0.5);
    let new: Vec<[f32; 3]> = game
        .sim
        .surface_cells(SurfaceLayer::Ground)
        .into_iter()
        .filter(|c| !cells_before.contains(c))
        .map(|(p, _)| p)
        .collect();
    assert!(new.len() >= 3, "{} spray cells", new.len());
    let mean_x = new.iter().map(|p| p[0]).sum::<f32>() / new.len() as f32;
    assert!(
        mean_x > t[0],
        "spray centre {mean_x} beyond the target at {}",
        t[0]
    );
}
