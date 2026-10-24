//! `bg3x nav`: summarise a level's AI grid and nav cloud, probe tiles, find paths.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use bg3_install::Install;
use bg3_nav::grid::column_of;
use bg3_nav::{Agent, NavGrid, TileRef};
use bg3_vfs::Vfs;

pub struct Options {
    pub level: String,
    pub path: Vec<[f32; 3]>,
    pub at: Option<[f32; 3]>,
    pub radius: f32,
    pub subgrids: bool,
    pub regions: bool,
}

pub fn parse_vec3(s: &str) -> Result<[f32; 3], String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|x| x.trim().parse::<f32>().map_err(|e| format!("{x}: {e}")))
        .collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "expected x,y,z".to_owned())
}

pub fn run(o: Options) -> Result<()> {
    let install = Install::locate()?;
    let vfs = Vfs::from_install(&install)?;
    let file = bg3_nav::load_aigrid(&vfs, &o.level).context("AI grid")?;
    let mut grid = NavGrid::new(file);
    let portals = bg3_nav::load_portals(&vfs, &o.level);
    let attached = grid.set_portals(&portals);
    for p in &portals {
        println!(
            "portal {}: {:?} -> {:?} (tiles {:?} -> {:?})",
            p.id,
            p.source,
            p.target,
            grid.portal_tile(p.source).map(|t| grid.world_pos(t)),
            grid.portal_tile(p.target).map(|t| grid.world_pos(t)),
        );
    }
    if !portals.is_empty() {
        println!("{attached} of {} portals attached", portals.len());
    }
    let agent = Agent {
        size: Agent::size_from_radius(o.radius),
        ..Agent::humanoid()
    };
    let subgrids = &grid.file.subgrids;
    let tiles: usize = subgrids.iter().map(|s| s.tiles.len()).sum();
    let non_empty = subgrids
        .iter()
        .flat_map(|s| &s.tiles)
        .filter(|t| !t.is_empty())
        .count();
    println!(
        "{}: AI grid v{}, {} subgrids, {tiles} tiles ({non_empty} non-empty), {} layers",
        o.level,
        grid.file.version,
        subgrids.len(),
        grid.file.layers.len()
    );
    match bg3_nav::load_navcloud(&vfs, &o.level) {
        Ok(nc) => println!(
            "nav cloud: {} regions ({} fine)",
            nc.regions.len(),
            nc.regions.iter().filter(|r| r.kind == 1).count()
        ),
        Err(e) => println!("nav cloud: {e}"),
    }
    println!(
        "agent: size {} tiles, step {} m, mask {:#018x}",
        agent.size, agent.step_height, agent.collision_mask
    );
    if o.subgrids {
        for (i, s) in subgrids.iter().enumerate() {
            let standable = (0..s.height)
                .flat_map(|y| (0..s.width).map(move |x| (x, y)))
                .filter(|&(x, y)| grid.is_standable(tile(i, x, y), &agent))
                .count();
            println!(
                "  #{i:<3} id {:08x} {:>3}x{:<3} origin {:>8.2} {:>7.2} {:>8.2}  object {}  standable {standable}",
                s.id, s.width, s.height, s.origin[0], s.origin[1], s.origin[2], s.object
            );
        }
    }
    if o.regions {
        regions(&grid, &agent);
    }
    if let Some(p) = o.at {
        println!("column {:?} at {p:?}:", column_of(p));
        let mut col: Vec<TileRef> = grid.tiles_in_column(column_of(p)).collect();
        col.sort_by(|a, b| grid.height(*b).total_cmp(&grid.height(*a)));
        for t in col {
            println!(
                "  subgrid #{} ({},{}) y {:.2} flags {:#018x} area {:#018x} standable {}",
                t.subgrid,
                t.x,
                t.y,
                grid.height(t),
                grid.tile(t).flags,
                grid.state_in_area(t, agent.size),
                grid.is_standable(t, &agent)
            );
        }
    }
    for pair in o.path.as_chunks::<2>().0 {
        let started = std::time::Instant::now();
        match grid.find_path(pair[0], pair[1], &agent) {
            Ok(path) => {
                println!(
                    "path {:?} -> {:?}: {} tiles, cost {}, {:.1} m, {} expanded, {:.2?}",
                    pair[0],
                    pair[1],
                    path.tiles.len(),
                    path.cost,
                    path.length(),
                    path.expanded,
                    started.elapsed()
                );
                for w in &path.waypoints {
                    println!("  {:.2},{:.2},{:.2}", w[0], w[1], w[2]);
                }
            }
            Err(e) => println!("path {:?} -> {:?}: {e}", pair[0], pair[1]),
        }
    }
    Ok(())
}

fn tile(i: usize, x: u32, y: u32) -> TileRef {
    TileRef {
        subgrid: i as u32,
        x: x as u16,
        y: y as u16,
    }
}

/// Tile count, bounds (min, max) and a sample position of a connected region.
type Region = (usize, [f32; 3], [f32; 3], [f32; 3]);

/// Connected regions of standable tiles (4-neighbourhood within the step height), largest first.
fn regions(grid: &NavGrid, agent: &Agent) {
    let mut seen: HashSet<TileRef> = HashSet::new();
    let mut found: Vec<Region> = Vec::new();
    let mut standable: HashMap<TileRef, bool> = HashMap::new();
    let mut ok = |t: TileRef| {
        *standable
            .entry(t)
            .or_insert_with(|| grid.is_standable(t, agent))
    };
    for (i, s) in grid.file.subgrids.iter().enumerate() {
        for y in 0..s.height {
            for x in 0..s.width {
                let t = tile(i, x, y);
                if seen.contains(&t) || !ok(t) {
                    continue;
                }
                let mut queue = VecDeque::from([t]);
                seen.insert(t);
                let (mut n, mut lo, mut hi) = (0, [f32::MAX; 3], [f32::MIN; 3]);
                let sample = grid.world_pos(t);
                while let Some(c) = queue.pop_front() {
                    n += 1;
                    let p = grid.world_pos(c);
                    for a in 0..3 {
                        lo[a] = lo[a].min(p[a]);
                        hi[a] = hi[a].max(p[a]);
                    }
                    for nb in grid.neighbours(c) {
                        if !seen.contains(&nb)
                            && (grid.height(nb) - grid.height(c)).abs() <= agent.step_height
                            && ok(nb)
                        {
                            seen.insert(nb);
                            queue.push_back(nb);
                        }
                    }
                }
                found.push((n, lo, hi, sample));
            }
        }
    }
    found.sort_by_key(|r| std::cmp::Reverse(r.0));
    println!("{} connected regions; largest:", found.len());
    for (n, lo, hi, s) in found.iter().take(12) {
        println!(
            "  {n:>7} tiles  x {:.1}..{:.1}  y {:.1}..{:.1}  z {:.1}..{:.1}  e.g. {:.2},{:.2},{:.2}",
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2], s[0], s[1], s[2]
        );
    }
}
