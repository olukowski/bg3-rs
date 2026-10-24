//! Why two points of a level aren't connected: flood the humanoid-standable tiles reachable
//! from each (portals not followed) and print where the two regions come closest, with the
//! height step between them (a ledge, a door frame, …).
//!
//! `cargo run --release -p bg3x --example nav_gap -- LEVEL x,y,z x,y,z`
use bg3_nav::{Agent, NavGrid, TileRef};
use std::collections::{HashSet, VecDeque};
fn flood(grid: &NavGrid, start: TileRef, agent: &Agent) -> HashSet<TileRef> {
    let mut seen = HashSet::from([start]);
    let mut q = VecDeque::from([start]);
    while let Some(t) = q.pop_front() {
        for n in grid.neighbours(t) {
            if seen.contains(&n)
                || (grid.height(n) - grid.height(t)).abs() > agent.step_height
                || !grid.is_standable(n, agent)
            {
                continue;
            }
            seen.insert(n);
            q.push_back(n);
        }
    }
    seen
}
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = |s: &str| -> [f32; 3] {
        let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
        [v[0], v[1], v[2]]
    };
    let install = bg3_install::Install::locate()?;
    let vfs = bg3_vfs::Vfs::from_install(&install)?;
    let grid: NavGrid = bg3_nav::load_navgrid(&vfs, &a[1])?;
    let agent = Agent::humanoid();
    let ta = grid.project_standable(p(&a[2]), 2.0, 3, &agent).unwrap();
    let tb = grid.project_standable(p(&a[3]), 2.0, 3, &agent).unwrap();
    let ra = flood(&grid, ta, &agent);
    let rb = flood(&grid, tb, &agent);
    println!(
        "region A {} tiles, region B {} tiles, shared {}",
        ra.len(),
        rb.len(),
        ra.intersection(&rb).count()
    );
    let mut best = Vec::new();
    for &x in &rb {
        let wx = grid.world_pos(x);
        for &y in &ra {
            let wy = grid.world_pos(y);
            let d = ((wx[0] - wy[0]).powi(2) + (wx[2] - wy[2]).powi(2)).sqrt();
            if d < 8.0 {
                best.push((d, (wx[1] - wy[1]).abs(), wy, wx));
            }
        }
    }
    best.sort_by(|a, b| a.0.total_cmp(&b.0));
    for b in best.iter().take(12) {
        println!(
            "flat {:.2} m, height step {:.2} m: A {:?}  B {:?}",
            b.0, b.1, b.2, b.3
        );
    }
    Ok(())
}
