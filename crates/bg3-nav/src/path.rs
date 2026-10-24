//! Path finding on the AI grid.
//!
//! Faithful to the reference build (see `docs/formats/navcloud.md` § Path finding):
//! 4-connected tile graph, 10 per step, A* with heuristic `round(48 · distance)` (inadmissible,
//! so the search is greedy-weighted), collision/stability tests per expanded tile.
//! Approximated: the game searches from both ends at once and post-processes the tile path with
//! several smoothing passes; here the search runs from the start only and the smoothing is a
//! line-of-walk string pull. Height costs use an assumed coefficient.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::aigrid::TILE_SIZE;
use crate::grid::{Agent, NavGrid, TileRef, column_of};

/// Cost of moving to a 4-connected neighbour tile.
// bg3: eoc::AiGrid::FindPath (inlined in eoc::AiGrid::UpdatePathfinding), $_15 lambda passes 10
// to eoc::AiGrid::CheckTile (4.1.1.7631656); portals cost 100.
pub const STEP_COST: i32 = 10;

/// Heuristic: `round(2 · distance · 24)`.
// bg3: eoc::AiGrid::CheckTile (4.1.1.7631656)
pub fn heuristic(a: [f32; 3], b: [f32; 3]) -> i32 {
    let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    (d * 2.0 * 24.0).round() as i32
}

/// Extra cost of a height change between neighbours. The game scales `|Δh|` by one of two
/// global tunables (below / above the step height) that aren't in the data files; this uses
/// the horizontal rate (20 per metre) for both. **Assumption.**
// bg3: (anonymous)::HeightCost (4.1.1.7631656)
pub fn height_cost(from: f32, to: f32) -> i32 {
    ((to - from).abs() * STEP_COST as f32 / TILE_SIZE).round() as i32
}

/// Upper bound on expanded tiles per search.
pub const MAX_EXPANSIONS: usize = 400_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Tiles of the raw grid path, start to goal.
    pub tiles: Vec<TileRef>,
    /// Smoothed waypoints in world space, start to goal (the first is the start position).
    pub waypoints: Vec<[f32; 3]>,
    /// Total cost of the grid path.
    pub cost: i32,
    /// Tiles expanded by the search.
    pub expanded: usize,
}

impl Path {
    /// Horizontal length of the smoothed path in metres.
    pub fn length(&self) -> f32 {
        self.waypoints
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt())
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// No standable tile near the start position.
    NoStart,
    /// No standable tile near the goal position.
    NoGoal,
    /// The goal can't be reached from the start.
    Unreachable,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoStart => "no standable tile at the start",
            Self::NoGoal => "no standable tile at the goal",
            Self::Unreachable => "goal unreachable",
        })
    }
}

impl std::error::Error for PathError {}

/// How far (vertically, metres) a start/goal point may be from the grid surface.
pub const SNAP_HEIGHT: f32 = 2.0;
/// How far (tiles) to look sideways for a standable start/goal tile.
pub const SNAP_RADIUS: i32 = 3;

impl NavGrid {
    /// Find a path for `agent` between two world positions.
    pub fn find_path(
        &self,
        from: [f32; 3],
        to: [f32; 3],
        agent: &Agent,
    ) -> Result<Path, PathError> {
        let start = self
            .project_standable(from, SNAP_HEIGHT, SNAP_RADIUS, agent)
            .ok_or(PathError::NoStart)?;
        let goal = self
            .project_standable(to, SNAP_HEIGHT, SNAP_RADIUS, agent)
            .ok_or(PathError::NoGoal)?;
        let (tiles, cost, expanded) = self.search(start, goal, agent)?;
        let mut goal_pos = to;
        goal_pos[1] = self.height(goal);
        if column_of(to) != self.column(goal) {
            goal_pos = self.world_pos(goal);
        }
        let mut start_pos = from;
        start_pos[1] = self.height(start);
        if column_of(from) != self.column(start) {
            start_pos = self.world_pos(start);
        }
        let waypoints = self.smooth(&tiles, start_pos, goal_pos, agent);
        Ok(Path {
            tiles,
            waypoints,
            cost,
            expanded,
        })
    }

    /// A* over tiles. Returns the tile path, its cost and the number of expansions.
    // bg3: eoc::AiGrid::UpdatePathfinding / eoc::AiGrid::CheckTile (4.1.1.7631656)
    pub fn search(
        &self,
        start: TileRef,
        goal: TileRef,
        agent: &Agent,
    ) -> Result<(Vec<TileRef>, i32, usize), PathError> {
        let goal_pos = self.world_pos(goal);
        let mut g: HashMap<TileRef, (i32, Option<TileRef>)> = HashMap::new();
        let mut open = BinaryHeap::new();
        // Insertion counter keeps ties FIFO (the game's heap order on ties is unknown).
        let mut seq = 0u32;
        g.insert(start, (0, None));
        open.push(Reverse((
            heuristic(self.world_pos(start), goal_pos),
            seq,
            start,
        )));
        let mut closed = std::collections::HashSet::new();
        let mut expanded = 0;
        while let Some(Reverse((_, _, cur))) = open.pop() {
            if !closed.insert(cur) {
                continue;
            }
            if cur == goal {
                let cost = g[&cur].0;
                let mut tiles = vec![cur];
                let mut at = cur;
                while let Some(prev) = g[&at].1 {
                    tiles.push(prev);
                    at = prev;
                }
                tiles.reverse();
                return Ok((tiles, cost, expanded));
            }
            expanded += 1;
            if expanded > MAX_EXPANSIONS {
                break;
            }
            let (gc, _) = g[&cur];
            let hc = self.height(cur);
            // Portal steps: no step-height or standability test on the exit tile (the engine
            // attaches portals to tiles free of its own mask; see crate::portal).
            for &n in self.portal_exits(cur) {
                if closed.contains(&n) {
                    continue;
                }
                let cost = gc + crate::portal::PORTAL_COST;
                if g.get(&n).is_some_and(|&(old, _)| old <= cost) {
                    continue;
                }
                g.insert(n, (cost, Some(cur)));
                seq += 1;
                open.push(Reverse((
                    cost + heuristic(self.world_pos(n), goal_pos),
                    seq,
                    n,
                )));
            }
            for n in self.neighbours(cur) {
                if closed.contains(&n) {
                    continue;
                }
                let hn = self.height(n);
                if (hn - hc).abs() > agent.step_height {
                    continue;
                }
                let cost = gc + STEP_COST + height_cost(hc, hn);
                if g.get(&n).is_some_and(|&(old, _)| old <= cost) {
                    continue;
                }
                if n != goal && !self.is_standable(n, agent) {
                    continue;
                }
                g.insert(n, (cost, Some(cur)));
                seq += 1;
                open.push(Reverse((
                    cost + heuristic(self.world_pos(n), goal_pos),
                    seq,
                    n,
                )));
            }
        }
        Err(PathError::Unreachable)
    }

    /// Whether the agent can walk straight from `a` to `b` (both on the grid near `from_tile`):
    /// every quarter-tile sample along the segment lies over a standable tile reachable from
    /// the previous one within the step height.
    pub fn can_walk_straight(
        &self,
        a: [f32; 3],
        b: [f32; 3],
        from_tile: TileRef,
        agent: &Agent,
    ) -> bool {
        let dx = b[0] - a[0];
        let dz = b[2] - a[2];
        let len = (dx * dx + dz * dz).sqrt();
        let steps = (len / (TILE_SIZE * 0.25)).ceil().max(1.0) as usize;
        let mut cur = from_tile;
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            let col = column_of([a[0] + dx * f, 0.0, a[2] + dz * f]);
            if col == self.column(cur) {
                continue;
            }
            let h = self.height(cur);
            let Some(next) = self.closest_in_column(col, h) else {
                return false;
            };
            if (self.height(next) - h).abs() > agent.step_height || !self.is_standable(next, agent)
            {
                return false;
            }
            cur = next;
        }
        true
    }

    /// String-pull the tile path into waypoints. **Approximation** of the game's smoothing passes
    /// (`SmoothPath_vRecursive`, `_vSubdivide`, `_vCorners`, `_vBrute`, `InsertTurningNodes`,
    /// `SplinePath`, `RemoveStraightNodes`).
    pub fn smooth(
        &self,
        tiles: &[TileRef],
        start: [f32; 3],
        goal: [f32; 3],
        agent: &Agent,
    ) -> Vec<[f32; 3]> {
        let point = |i: usize| -> [f32; 3] {
            if i == 0 {
                start
            } else if i + 1 == tiles.len() {
                goal
            } else {
                self.world_pos(tiles[i])
            }
        };
        let mut out = vec![start];
        if tiles.len() <= 1 {
            if goal != start {
                out.push(goal);
            }
            return out;
        }
        let mut anchor = 0;
        while anchor + 1 < tiles.len() {
            // Advance while the next tile is still reachable in a straight line.
            let mut next = anchor + 1;
            // A portal step (climb, jump) is never pulled straight: both its ends stay.
            while next + 1 < tiles.len()
                && !self.portal_exits(tiles[next]).contains(&tiles[next + 1])
                && self.can_walk_straight(point(anchor), point(next + 1), tiles[anchor], agent)
            {
                next += 1;
            }
            out.push(point(next));
            anchor = next;
        }
        out
    }
}
