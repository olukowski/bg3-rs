//! Navigation portals: `Mods/<Module>/Levels/<Level>/Ai/navigationPortals.lsf`, links between two
//! points of the AI grid that a walk can't connect (a climb, a jump). The Nautiloid has one: the
//! climb from below the transformation chamber up to its entrance.
//!
//! In the engine each end is attached to a tile (`eoc::AiGridPortal::AttachToGrid`): the
//! position goes to its closest tile (`eoc::AiGrid::ToClosestTilePos`), and a flood
//! (`eoc::ClosestToTargetFlood`, collision flags `0x80c`, no distance limit) finds the nearest
//! tile free of those flags. `eoc::AiGrid::AddPortal` then registers the portal on that tile,
//! and the path search steps through it at cost 100 (`eoc::AiGrid::CheckTile`).
// bg3: eoc::NavigationPortalManager::Load, eoc::AiGridPortal::AttachToGrid,
// eoc::AiGrid::AddPortal (4.1.1.7631656)
//!
//! **Approximate**: the end tile is the nearest tile free of `0x80c` within [`ATTACH_RADIUS`]
//! columns and [`ATTACH_HEIGHT`] metres (a column search, not the engine's flood), moved to the
//! nearest tile a humanoid can stand on when it can't (how the engine's portal step tests
//! collision was not read).
//! Both directions are linked (the engine adds the portal at both end tiles).
//! `eoc::AiGrid::HeuristicWithPortal` is not modelled.

use crate::aigrid::flags;
use crate::grid::{NavGrid, TileRef, column_of};

/// Collision flags the attach flood avoids (bits 2, 3 and 11).
// bg3: eoc::AiGridPortal::AttachToGrid (4.1.1.7631656) passes 0x80c to ClosestToTargetFlood.
pub const ATTACH_MASK: u64 = 0x80c;
/// Search radius (tiles) and height window (metres) for an end tile. **Approximation**.
pub const ATTACH_RADIUS: i32 = 16;
pub const ATTACH_HEIGHT: f32 = 4.0;
/// Path cost of stepping through a portal.
// bg3: eoc::AiGrid::FindPath passes 100 to eoc::AiGrid::CheckTile for portal steps
// (4.1.1.7631656).
pub const PORTAL_COST: i32 = 100;

/// A portal from the level data.
#[derive(Debug, Clone, PartialEq)]
pub struct NavPortal {
    pub id: i32,
    pub source: [f32; 3],
    pub target: [f32; 3],
}

/// Read `navigationPortals.lsf` (region `PortalData`, nodes `Portal` with `Id`, `Source`,
/// `Target`).
pub fn parse_portals(res: &bg3_lsf::Resource) -> Vec<NavPortal> {
    let vec3 = |n: &bg3_lsf::Node, name: &str| match n.attr(name) {
        Some(bg3_lsf::Value::Vec3(v)) => Some(*v),
        _ => None,
    };
    res.region("PortalData")
        .into_iter()
        .flat_map(|n| n.children_named("Portal"))
        .filter_map(|p| {
            Some(NavPortal {
                id: p.attr("Id").and_then(|v| v.as_i64()).unwrap_or(-1) as i32,
                source: vec3(p, "Source")?,
                target: vec3(p, "Target")?,
            })
        })
        .collect()
}

/// A level's portals (last module that has the file), or none.
pub fn load_portals(vfs: &bg3_vfs::Vfs, level: &str) -> Vec<NavPortal> {
    let path = bg3_vfs::modules::BASE_LOAD_ORDER
        .iter()
        .rev()
        .map(|m| format!("Mods/{m}/Levels/{level}/Ai/navigationPortals.lsf"))
        .find(|p| vfs.exists(p));
    let Some(path) = path else {
        return Vec::new();
    };
    vfs.read(&path)
        .ok()
        .and_then(|d| bg3_lsf::parse(&d).ok())
        .map(|r| parse_portals(&r))
        .unwrap_or_default()
}

impl NavGrid {
    /// The tile a portal end attaches to (see the module docs).
    pub fn portal_tile(&self, p: [f32; 3]) -> Option<TileRef> {
        let (cx, cz) = column_of(p);
        let mut best: Option<(f32, TileRef)> = None;
        for dz in -ATTACH_RADIUS..=ATTACH_RADIUS {
            for dx in -ATTACH_RADIUS..=ATTACH_RADIUS {
                for t in self.tiles_in_column((cx + dx, cz + dz)) {
                    let f = self.tile(t).flags;
                    if f & flags::EMPTY != 0 || f & ATTACH_MASK != 0 {
                        continue;
                    }
                    let w = self.world_pos(t);
                    if (w[1] - p[1]).abs() > ATTACH_HEIGHT {
                        continue;
                    }
                    let d = (0..3).map(|i| (w[i] - p[i]).powi(2)).sum::<f32>();
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, t));
                    }
                }
            }
        }
        // The engine's tile may be one a humanoid footprint can't stand on (its collision
        // test for portal steps was not read); step to the nearest standable tile instead.
        let (_, t) = best?;
        let agent = crate::grid::Agent::humanoid();
        if self.is_standable(t, &agent) {
            return Some(t);
        }
        self.project_standable(self.world_pos(t), 1.0, 3, &agent)
            .or(Some(t))
    }

    /// Attach portals: each links its two end tiles both ways. Returns how many attached.
    pub fn set_portals(&mut self, portals: &[NavPortal]) -> usize {
        self.portal_links.clear();
        let mut n = 0;
        for p in portals {
            let (Some(a), Some(b)) = (self.portal_tile(p.source), self.portal_tile(p.target))
            else {
                continue;
            };
            self.portal_links.entry(a).or_default().push(b);
            self.portal_links.entry(b).or_default().push(a);
            n += 1;
        }
        n
    }

    /// Tiles reachable from `t` through a portal.
    pub fn portal_exits(&self, t: TileRef) -> &[TileRef] {
        self.portal_links.get(&t).map_or(&[], Vec::as_slice)
    }
}
