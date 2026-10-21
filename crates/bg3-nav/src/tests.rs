//! Unit tests on self-authored synthetic grids.

use crate::aigrid::{AiGridFile, HEIGHT_SCALE, Subgrid, TILE_SIZE, Tile, flags};
use crate::grid::{Agent, NavGrid};
use crate::path::{PathError, heuristic};

const GUID: &str = "00000000-0000-0000-0000-000000000001";

fn flat(id: u32, w: u32, h: u32, origin: [f32; 3]) -> Subgrid {
    Subgrid {
        id,
        width: w,
        height: h,
        origin,
        object: GUID.into(),
        parent: "00000000-0000-0000-0000-000000000000".into(),
        tiles: vec![
            Tile {
                flags: 0x8000,
                height: 0,
                height2: 0,
                metadata: u32::MAX,
            };
            (w * h) as usize
        ],
    }
}

fn file(subgrids: Vec<Subgrid>) -> AiGridFile {
    AiGridFile {
        version: 21,
        level: GUID.into(),
        subgrids,
        layers: Vec::new(),
    }
}

fn set(s: &mut Subgrid, x: u32, y: u32, f: impl FnOnce(&mut Tile)) {
    let w = s.width;
    f(&mut s.tiles[(x + y * w) as usize]);
}

/// World position of a tile centre of a subgrid at origin (0, 0, 0).
fn at(x: u32, y: u32) -> [f32; 3] {
    [
        (x as f32 + 0.5) * TILE_SIZE,
        0.0,
        (y as f32 + 0.5) * TILE_SIZE,
    ]
}

#[test]
fn file_round_trips() {
    let mut s = flat(7, 5, 3, [-1.5, 2.0, 10.0]);
    set(&mut s, 1, 2, |t| {
        t.flags = flags::BLOCK_2 | 0x8000;
        t.height = 150;
        t.height2 = 120;
        t.metadata = 3;
    });
    let mut f = file(vec![s, flat(8, 2, 2, [0.0, 0.0, 0.0])]);
    f.layers.push(crate::aigrid::Layer {
        guid: [7; 16],
        tiles: vec![crate::aigrid::LayerTile {
            x: 1,
            y: 0,
            subgrid_id: 8,
            flags: flags::BLOCK_7,
            height: 4.5,
            extra: u32::MAX,
        }],
    });
    let bytes = f.to_bytes();
    let back = AiGridFile::parse(&bytes).unwrap();
    assert_eq!(back, f);
    assert_eq!(back.subgrids[0].tile(1, 2).unwrap().height, 150);
    let c = back.subgrids[0].tile_center(1, 2);
    assert_eq!(c, [-1.5 + 0.75, 2.0 + 150.0 * HEIGHT_SCALE, 10.0 + 1.25]);
}

#[test]
fn rejects_unknown_version_and_truncation() {
    let mut bytes = file(vec![flat(1, 2, 2, [0.0; 3])]).to_bytes();
    assert!(AiGridFile::parse(&bytes[..bytes.len() - 20]).is_err());
    bytes[0] = 20;
    assert!(matches!(
        AiGridFile::parse(&bytes),
        Err(crate::Error::Version(20))
    ));
}

#[test]
fn agent_size_matches_the_game_rounding() {
    assert_eq!(Agent::size_from_radius(0.499), 1);
    assert_eq!(Agent::size_from_radius(0.249), 0);
    assert_eq!(Agent::size_from_radius(0.75), 1);
    assert_eq!(Agent::size_from_radius(0.76), 2);
    assert_eq!(heuristic([0.0; 3], [3.0, 0.0, 4.0]), 240);
}

#[test]
fn walls_block_and_the_path_goes_around() {
    // 24 × 24 floor, wall at x = 12 for y < 18.
    let mut s = flat(1, 24, 24, [0.0; 3]);
    for y in 0..18 {
        set(&mut s, 12, y, |t| t.flags |= flags::BLOCK_2);
    }
    let grid = NavGrid::new(file(vec![s]));
    let agent = Agent::humanoid();
    let blocked = grid.project(at(11, 5), 0.5).unwrap();
    assert!(grid.is_colliding(blocked, &agent), "next to the wall");
    assert!(
        !grid.is_colliding(blocked, &Agent::new(0.2, 0.6)),
        "small agent fits"
    );
    let path = grid.find_path(at(4, 4), at(20, 4), &agent).unwrap();
    // Every tile keeps the footprint off the wall, and the path rounds the wall's end.
    for &t in &path.tiles {
        assert!(grid.is_standable(t, &agent) || t == *path.tiles.last().unwrap());
    }
    assert!(path.tiles.iter().any(|&t| t.y >= 19));
    // Smoothed waypoints are a few corners, not every tile.
    assert!(
        path.waypoints.len() < path.tiles.len() / 4,
        "{:?}",
        path.waypoints
    );
    assert_eq!(path.waypoints.first().copied(), Some(at(4, 4)));
    assert_eq!(path.waypoints.last().copied(), Some(at(20, 4)));
    assert!(path.length() > 16.0 * TILE_SIZE);
}

#[test]
fn cliffs_need_the_ramp() {
    // Left half at 0 m, right half at 1.5 m; a ramp along y ≥ 16 climbs 0.25 m per tile.
    let mut s = flat(1, 24, 24, [0.0; 3]);
    for y in 0..24 {
        for x in 12..24 {
            set(&mut s, x, y, |t| t.height = (1.5 / HEIGHT_SCALE) as u16);
        }
    }
    for y in 16..24 {
        for (i, x) in (6..12).enumerate() {
            let h = 0.25 * (i + 1) as f32;
            set(&mut s, x, y, |t| {
                t.height = (h / HEIGHT_SCALE).round() as u16
            });
        }
    }
    let grid = NavGrid::new(file(vec![s]));
    let agent = Agent::humanoid();
    let mut goal = at(18, 4);
    goal[1] = 1.5;
    let path = grid.find_path(at(3, 4), goal, &agent).unwrap();
    assert!(path.tiles.iter().any(|&t| t.y >= 17), "uses the ramp");
    for w in path.tiles.windows(2) {
        assert!((grid.height(w[0]) - grid.height(w[1])).abs() <= agent.step_height);
    }
}

#[test]
fn stacked_subgrids_project_and_connect_by_height() {
    // Ground at 0 m, a deck at 3 m over x 4..12 that continues sideways into a second subgrid.
    let ground = flat(1, 16, 8, [0.0; 3]);
    let deck = flat(2, 8, 8, [2.0, 3.0, 0.0]);
    let deck2 = flat(3, 6, 8, [6.0, 3.0, 0.0]);
    let mut f = file(vec![ground, deck, deck2]);
    // Mark the overlap like the game does.
    for s in &mut f.subgrids {
        for t in &mut s.tiles {
            t.flags |= flags::MULTIPLE_SUBGRIDS;
        }
    }
    let grid = NavGrid::new(f);
    let low = grid.project([5.1, 0.2, 1.1], 1.0).unwrap();
    let high = grid.project([5.1, 2.9, 1.1], 1.0).unwrap();
    assert_eq!(low.subgrid, 0);
    assert_eq!(high.subgrid, 1);
    // A walk on the deck crosses into the third subgrid, staying at 3 m.
    let agent = Agent::new(0.2, 0.6);
    let path = grid
        .find_path([3.1, 3.0, 2.1], [8.6, 3.0, 2.1], &agent)
        .unwrap();
    assert!(path.tiles.iter().all(|&t| grid.height(t) == 3.0));
    assert!(path.tiles.iter().any(|&t| t.subgrid == 2));
    // The ground can't reach the deck (no ramp).
    assert_eq!(
        grid.find_path([1.1, 0.0, 1.1], [5.1, 3.0, 1.1], &agent),
        Err(PathError::Unreachable)
    );
    // Raycast from above hits the deck, not the ground below it.
    let (hit, p) = grid
        .raycast([5.1, 10.0, 1.1], [0.0, -1.0, 0.0], 20.0)
        .unwrap();
    assert_eq!(hit.subgrid, 1);
    assert!((p[1] - 3.0).abs() < 1e-4);
}

#[test]
fn empty_tiles_are_holes() {
    let mut s = flat(1, 10, 3, [0.0; 3]);
    for y in 0..3 {
        set(&mut s, 5, y, |t| t.flags = flags::EMPTY);
    }
    let grid = NavGrid::new(file(vec![s]));
    let agent = Agent::new(0.2, 0.6);
    assert!(grid.project(at(5, 1), 1.0).is_none());
    assert_eq!(
        grid.find_path(at(1, 1), at(8, 1), &agent),
        Err(PathError::Unreachable)
    );
}
