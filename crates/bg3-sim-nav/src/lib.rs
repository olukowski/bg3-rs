//! Optional glue between the simulation's path hook and the game's AI grid.
//! Neither core crate depends on the other (or on Bevy).

use std::sync::Arc;

use bg3_nav::{Agent, NavGrid};
use bg3_sim::combat::Pathfinder;

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
