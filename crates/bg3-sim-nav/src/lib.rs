//! Optional glue between the simulation's path and surface hooks and the game's AI grid.
//! Neither core crate depends on the other (or on Bevy).

use std::sync::Arc;

use bg3_nav::{Agent, NavGrid, TileRef};
use bg3_sim::combat::Pathfinder;
use bg3_sim::surface::{Cell, SurfaceGrid, SurfaceLayer};

/// Shares the same read-only level grid with the presentation layer and diagnostics.
pub struct GridPathfinder {
    pub grid: Arc<NavGrid>,
}

impl GridPathfinder {
    pub fn new(grid: Arc<NavGrid>) -> Self {
        Self { grid }
    }

    /// Check the actual endpoint's column, without snapping sideways to a walkable tile.
    pub fn is_standable(&self, position: [f32; 3], radius: f32) -> bool {
        let agent = Agent {
            size: Agent::size_from_radius(radius),
            ..Agent::humanoid()
        };
        self.grid
            .project(position, 0.6)
            .is_some_and(|tile| self.grid.is_standable(tile, &agent))
    }
}

impl Pathfinder for GridPathfinder {
    fn find_path(&self, from: [f32; 3], to: [f32; 3], radius: f32) -> Option<Vec<[f32; 3]>> {
        let agent = Agent {
            size: Agent::size_from_radius(radius),
            ..Agent::humanoid()
        };
        // A sideways snap from an unwalkable spawn could cross a wall before the
        // grid path even starts. Only accept a standable tile in the actual start column.
        let start = self.grid.project(from, bg3_nav::path::SNAP_HEIGHT)?;
        if !self.grid.is_standable(start, &agent) {
            return None;
        }
        let path = self.grid.find_path(from, to, &agent).ok()?;
        let mut points = path.waypoints;
        // NavGrid projects to the floor. Include the caller's starting feet position so
        // the sim accounts for that short approach as well (spawn positions can be offset).
        if points.first() != Some(&from) {
            points.insert(0, from);
        }
        Some(points)
    }
}

/// The level's AI grid as the sim's surface grid: a cell is a [`TileRef`] packed into a `u64`.
pub struct GridSurfaces {
    pub grid: Arc<NavGrid>,
}

impl GridSurfaces {
    pub fn new(grid: Arc<NavGrid>) -> Self {
        Self { grid }
    }

    pub fn cell(t: TileRef) -> Cell {
        u64::from(t.subgrid) << 32 | u64::from(t.x) << 16 | u64::from(t.y)
    }

    pub fn tile(c: Cell) -> TileRef {
        TileRef {
            subgrid: (c >> 32) as u32,
            x: (c >> 16) as u16,
            y: c as u16,
        }
    }
}

impl SurfaceGrid for GridSurfaces {
    fn cells_in_column(&self, column: (i32, i32)) -> Vec<Cell> {
        self.grid.tiles_in_column(column).map(Self::cell).collect()
    }

    fn column(&self, cell: Cell) -> (i32, i32) {
        self.grid.column(Self::tile(cell))
    }

    fn height(&self, cell: Cell) -> f32 {
        self.grid.height(Self::tile(cell))
    }

    fn flags(&self, cell: Cell) -> u64 {
        self.grid.tile(Self::tile(cell)).flags
    }

    fn painted_cells(&self) -> Vec<Cell> {
        let mask = SurfaceLayer::Ground.painted(0xff) | 0xff << 32;
        let mut out = Vec::new();
        for (i, s) in self.grid.file.subgrids.iter().enumerate() {
            for (j, t) in s.tiles.iter().enumerate() {
                if !t.is_empty() && t.flags & mask != 0 {
                    out.push(Self::cell(TileRef {
                        subgrid: i as u32,
                        x: (j % s.width as usize) as u16,
                        y: (j / s.width as usize) as u16,
                    }));
                }
            }
        }
        out
    }
}
