//! Queries over a level's AI grid: tile lookup by world column, projection onto the walkable
//! surface, and the per-agent standability tests used by the path finder.

use std::collections::HashMap;

use crate::aigrid::{AiGridFile, HEIGHT_SCALE, TILE_SIZE, Tile, flags};

/// A tile of a [`NavGrid`]: subgrid index (into `file.subgrids`) and tile coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileRef {
    pub subgrid: u32,
    pub x: u16,
    pub y: u16,
}

/// Who is walking: footprint, step height and collision mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agent {
    /// Footprint half-extent in tiles: the collision test covers `(2·size + 1)²` tiles.
    pub size: i32,
    /// Largest height difference between neighbouring tiles that can be walked
    /// (character template `MovementStepUpHeight`).
    pub step_height: f32,
    /// Tiles whose flags intersect this mask block the agent.
    pub collision_mask: u64,
}

/// `MovementStepUpHeight` default of character templates.
// bg3: eoc::CharacterTemplate::Visit (4.1.1.7631656) — default 0.6 for MovementStepUpHeight.
pub const DEFAULT_STEP_HEIGHT: f32 = 0.6;

impl Agent {
    /// Footprint from a bound radius in metres: `floor(2·(r − 1e-5) + 0.5)` tiles.
    // bg3: eoc::AiGrid::CreatePath (4.1.1.7631656)
    pub fn size_from_radius(radius: f32) -> i32 {
        ((radius - 1e-5) * 2.0 + 0.5).floor() as i32
    }

    pub fn new(radius: f32, step_height: f32) -> Self {
        Self {
            size: Self::size_from_radius(radius),
            step_height,
            collision_mask: flags::DEFAULT_COLLISION_MASK,
        }
    }

    /// A humanoid: AI bound radius 0.499 (the common value in character templates) and the
    /// default step height.
    pub fn humanoid() -> Self {
        Self::new(0.499, DEFAULT_STEP_HEIGHT)
    }
}

/// Subgrids are bucketed by global tile column in squares of this many tiles.
const BUCKET: i32 = 64;

/// A level's AI grid prepared for queries.
pub struct NavGrid {
    pub file: AiGridFile,
    /// Global tile column of each subgrid's tile (0, 0).
    origins: Vec<(i32, i32)>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
    /// Portal links between tiles ([`crate::portal`]).
    pub(crate) portal_links: HashMap<TileRef, Vec<TileRef>>,
}

/// Global tile column containing a world position (`floor(x * 2), floor(z * 2)`).
pub fn column_of(p: [f32; 3]) -> (i32, i32) {
    (
        (p[0] / TILE_SIZE).floor() as i32,
        (p[2] / TILE_SIZE).floor() as i32,
    )
}

impl NavGrid {
    pub fn new(file: AiGridFile) -> Self {
        let origins: Vec<_> = file.subgrids.iter().map(|s| s.global_origin()).collect();
        let mut buckets: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, (s, &(gx, gz))) in file.subgrids.iter().zip(&origins).enumerate() {
            if s.width == 0 || s.height == 0 {
                continue;
            }
            let (bx0, bz0) = (gx.div_euclid(BUCKET), gz.div_euclid(BUCKET));
            let bx1 = (gx + s.width as i32 - 1).div_euclid(BUCKET);
            let bz1 = (gz + s.height as i32 - 1).div_euclid(BUCKET);
            for bx in bx0..=bx1 {
                for bz in bz0..=bz1 {
                    buckets.entry((bx, bz)).or_default().push(i as u32);
                }
            }
        }
        Self {
            file,
            origins,
            buckets,
            portal_links: HashMap::new(),
        }
    }

    pub fn tile(&self, t: TileRef) -> &Tile {
        let s = &self.file.subgrids[t.subgrid as usize];
        &s.tiles[t.x as usize + t.y as usize * s.width as usize]
    }

    /// Height of a tile's surface.
    pub fn height(&self, t: TileRef) -> f32 {
        let s = &self.file.subgrids[t.subgrid as usize];
        s.origin[1] + f32::from(self.tile(t).height) * HEIGHT_SCALE
    }

    /// World position of the tile centre on its surface.
    pub fn world_pos(&self, t: TileRef) -> [f32; 3] {
        self.file.subgrids[t.subgrid as usize].tile_center(u32::from(t.x), u32::from(t.y))
    }

    pub fn column(&self, t: TileRef) -> (i32, i32) {
        let (gx, gz) = self.origins[t.subgrid as usize];
        (gx + i32::from(t.x), gz + i32::from(t.y))
    }

    /// Non-empty tiles of every subgrid covering a global column.
    pub fn tiles_in_column(&self, (cx, cz): (i32, i32)) -> impl Iterator<Item = TileRef> + '_ {
        let bucket = self
            .buckets
            .get(&(cx.div_euclid(BUCKET), cz.div_euclid(BUCKET)));
        bucket.into_iter().flatten().filter_map(move |&i| {
            let s = &self.file.subgrids[i as usize];
            let (gx, gz) = self.origins[i as usize];
            let (x, y) = (cx - gx, cz - gz);
            if x < 0 || y < 0 || x >= s.width as i32 || y >= s.height as i32 {
                return None;
            }
            let t = TileRef {
                subgrid: i,
                x: x as u16,
                y: y as u16,
            };
            (!self.tile(t).is_empty()).then_some(t)
        })
    }

    /// The tile of a column whose height is closest to `height`.
    // bg3: eoc::AiGrid::ToClosestTilePos (4.1.1.7631656) — approximated as nearest height.
    pub fn closest_in_column(&self, col: (i32, i32), height: f32) -> Option<TileRef> {
        self.tiles_in_column(col).min_by(|&a, &b| {
            let da = (self.height(a) - height).abs();
            let db = (self.height(b) - height).abs();
            da.total_cmp(&db)
        })
    }

    /// Project a world point onto the grid: the tile in its column closest in height, if it is
    /// within `max_dist` metres vertically.
    pub fn project(&self, p: [f32; 3], max_dist: f32) -> Option<TileRef> {
        self.closest_in_column(column_of(p), p[1])
            .filter(|&t| (self.height(t) - p[1]).abs() <= max_dist)
    }

    /// Like [`project`](Self::project), but searches neighbouring columns (up to `radius` tiles
    /// away, nearest first) for a tile the agent can stand on.
    pub fn project_standable(
        &self,
        p: [f32; 3],
        max_dist: f32,
        radius: i32,
        agent: &Agent,
    ) -> Option<TileRef> {
        let (cx, cz) = column_of(p);
        let mut best: Option<(f32, TileRef)> = None;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let Some(t) = self.closest_in_column((cx + dx, cz + dz), p[1]) else {
                    continue;
                };
                let w = self.world_pos(t);
                if (w[1] - p[1]).abs() > max_dist || !self.is_standable(t, agent) {
                    continue;
                }
                let d = (w[0] - p[0]).powi(2) + (w[2] - p[2]).powi(2);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, t));
                }
            }
        }
        best.map(|(_, t)| t)
    }

    /// First tile surface hit by a ray (`dir` need not be normalised), up to `max_dist` metres.
    /// The ray is marched in quarter-tile steps; a tile is hit when the ray passes within
    /// `tolerance` metres of its surface while over its column.
    pub fn raycast(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_dist: f32,
    ) -> Option<(TileRef, [f32; 3])> {
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len == 0.0 {
            return None;
        }
        let d = dir.map(|v| v / len);
        let step = TILE_SIZE * 0.25;
        let mut prev_y = origin[1];
        let mut t = 0.0;
        while t <= max_dist {
            let p = [
                origin[0] + d[0] * t,
                origin[1] + d[1] * t,
                origin[2] + d[2] * t,
            ];
            let (lo, hi) = (p[1].min(prev_y) - 0.05, p[1].max(prev_y) + 0.05);
            let hit = self
                .tiles_in_column(column_of(p))
                .filter(|&tile| (lo..=hi).contains(&self.height(tile)))
                .max_by(|&a, &b| self.height(a).total_cmp(&self.height(b)));
            if let Some(tile) = hit {
                return Some((tile, [p[0], self.height(tile), p[2]]));
            }
            prev_y = p[1];
            t += step;
        }
        None
    }

    /// OR of the flags of the `(2·size + 1)²` tiles around `t` (empty tiles contribute nothing).
    /// Within one subgrid the scan stays in that subgrid; where it crosses subgrids (or the tile
    /// is flagged [`flags::MULTIPLE_SUBGRIDS`]) each column contributes its tile nearest in
    /// height to `t` (approximation of the game's multi-subgrid scan).
    // bg3: eoc::AiGrid::GetStateInArea (4.1.1.7631656)
    pub fn state_in_area(&self, t: TileRef, size: i32) -> u64 {
        let s = &self.file.subgrids[t.subgrid as usize];
        let center = *self.tile(t);
        if size <= 0 {
            return center.flags;
        }
        let (x, y) = (i32::from(t.x), i32::from(t.y));
        let inside = x - size >= 0
            && y - size >= 0
            && x + size < s.width as i32
            && y + size < s.height as i32;
        if inside && center.flags & flags::MULTIPLE_SUBGRIDS == 0 {
            let mut state = 0;
            for yy in y - size..=y + size {
                for xx in x - size..=x + size {
                    let f = s.tiles[(xx + yy * s.width as i32) as usize].flags;
                    if f & flags::EMPTY == 0 {
                        state |= f;
                    }
                }
            }
            if state & flags::MULTIPLE_SUBGRIDS == 0 {
                return state;
            }
        }
        let h = self.height(t);
        let (cx, cz) = self.column(t);
        let mut state = 0;
        for dz in -size..=size {
            for dx in -size..=size {
                if let Some(n) = self.closest_in_column((cx + dx, cz + dz), h)
                    && (self.height(n) - h).abs() <= AREA_HEIGHT_TOLERANCE
                {
                    state |= self.tile(n).flags;
                }
            }
        }
        state
    }

    /// Whether the agent's footprint at `t` touches a blocking tile.
    // bg3: eoc::AiGrid::IsColliding (4.1.1.7631656)
    pub fn is_colliding(&self, t: TileRef, agent: &Agent) -> bool {
        self.state_in_area(t, agent.size) & agent.collision_mask != 0
    }

    /// Whether the ground under the agent's footprint has a step larger than its step height
    /// between adjacent columns (a missing column counts as a drop).
    // bg3: eoc::AiGrid::IsAreaUnstable (4.1.1.7631656) — compares each column's height with its
    // left and upper neighbour in the footprint; the exact tile choice per column is approximated.
    pub fn is_area_unstable(&self, t: TileRef, agent: &Agent) -> bool {
        if agent.size < 1 {
            return false;
        }
        let r = agent.size;
        let h = self.height(t);
        let (cx, cz) = self.column(t);
        let n = (2 * r + 1) as usize;
        let mut heights = vec![f32::NAN; n * n];
        for (j, dz) in (-r..=r).enumerate() {
            for (i, dx) in (-r..=r).enumerate() {
                let hh = self
                    .closest_in_column((cx + dx, cz + dz), h)
                    .map_or(f32::NAN, |c| self.height(c));
                let unstable =
                    |o: f32| hh.is_nan() || o.is_nan() || (hh - o).abs() > agent.step_height;
                if (i > 0 && unstable(heights[j * n + i - 1]))
                    || (j > 0 && unstable(heights[(j - 1) * n + i]))
                {
                    return true;
                }
                heights[j * n + i] = hh;
            }
        }
        heights.iter().any(|h| h.is_nan())
    }

    /// The agent can stand on `t`: a real tile, no blocking flags under its footprint and level
    /// enough ground.
    // bg3: eoc::AiGrid::IsStandable (4.1.1.7631656)
    pub fn is_standable(&self, t: TileRef, agent: &Agent) -> bool {
        !self.tile(t).is_empty() && !self.is_colliding(t, agent) && !self.is_area_unstable(t, agent)
    }

    /// The four neighbours of a tile, in the order the game expands them. Within a subgrid
    /// (and for tiles not flagged [`flags::MULTIPLE_SUBGRIDS`]) the neighbour is the adjacent
    /// tile of the same subgrid; otherwise the tile of the adjacent column closest in height.
    // bg3: eoc::AiGrid::ForEachConnection (4.1.1.7631656); the path search visits directions
    // 0x40, 0x08, 0x10, 0x02 (eoc::aigrid::EGridDirection) in that order.
    pub fn neighbours(&self, t: TileRef) -> impl Iterator<Item = TileRef> + '_ {
        const DIRS: [(i32, i32); 4] = [(0, 1), (-1, 0), (1, 0), (0, -1)];
        let s = &self.file.subgrids[t.subgrid as usize];
        let multi = self.tile(t).flags & flags::MULTIPLE_SUBGRIDS != 0;
        let h = self.height(t);
        let (cx, cz) = self.column(t);
        DIRS.into_iter().filter_map(move |(dx, dz)| {
            let (x, y) = (i32::from(t.x) + dx, i32::from(t.y) + dz);
            let inside = x >= 0 && y >= 0 && x < s.width as i32 && y < s.height as i32;
            if inside && !multi {
                let n = TileRef {
                    subgrid: t.subgrid,
                    x: x as u16,
                    y: y as u16,
                };
                return (!self.tile(n).is_empty()).then_some(n);
            }
            self.closest_in_column((cx + dx, cz + dz), h)
        })
    }
}

/// Vertical window (metres) within which tiles of other subgrids count for an area scan.
/// Not taken from the game (its multi-subgrid scan isn't fully reversed).
pub const AREA_HEIGHT_TOLERANCE: f32 = 1.0;
