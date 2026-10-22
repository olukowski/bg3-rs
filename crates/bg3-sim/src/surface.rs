//! Surfaces: blood, water, fire, clouds … on the cells of the level's AI grid.
//!
//! The game keeps surfaces on the AI grid (`docs/research/surfaces.md`): every 0.5 m tile has
//! a ground and a cloud layer, and a tile's flags carry the surface type of each layer
//! (`eoc::GetSurfacePaintedState`). Levels ship with surfaces painted into the grid file;
//! the rest are made at runtime by surface actions (`esv::SurfaceAction` subclasses) that add
//! cells over time: puddles that grow from a point (death blood, `CreatePuddle`), sprays along
//! a line (blood from physical hits), discs (`CreateSurface`).
//!
//! The sim has no grid of its own: the caller supplies one through [`SurfaceGrid`]
//! (`bg3_sim_nav::GridSurfaces` over `bg3_nav::NavGrid`). Without a grid nothing is created.
//! Cells are keyed by an opaque [`Cell`] the grid hands out.
//!
//! Exact (read from the binary, see the research note): the type numbering, the layer split,
//! the grid's painted state and the block masks, the puddle growth (random frontier cell,
//! 4-neighbour expansion within 1.5 m of height, up to 50 attempts per step), the hit and
//! death rules' parameters. Approximate: the spray's rectangle rasterisation and splatter, and
//! surface interactions (a new surface replaces the old one on a cell; the game's transform
//! rules, e.g. fire on oil, are not modelled).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{Guid, Sim};

/// `eoc::ESurfaceType`, in order. Types up to [`LAST_GROUND`] live on the ground layer, the
/// rest (`…Cloud`) on the cloud layer.
// bg3: eoc::GetSurfaceName, eoc::GetSurfaceType (4.1.1.7631656) — 75 values (0..=0x4a).
pub const SURFACE_TYPES: [&str; 75] = [
    "SurfaceNone",
    "SurfaceWater",
    "SurfaceWaterElectrified",
    "SurfaceWaterFrozen",
    "SurfaceBlood",
    "SurfaceBloodElectrified",
    "SurfaceBloodFrozen",
    "SurfacePoison",
    "SurfaceOil",
    "SurfaceLava",
    "SurfaceGrease",
    "SurfaceWyvernPoison",
    "SurfaceWeb",
    "SurfaceDeepWater",
    "SurfaceVines",
    "SurfaceFire",
    "SurfaceAcid",
    "SurfaceTrialFire",
    "SurfaceBlackPowder",
    "SurfaceShadowCursedVines",
    "SurfaceAlienOil",
    "SurfaceMud",
    "SurfaceAlcohol",
    "SurfaceInvisibleWeb",
    "SurfaceBloodSilver",
    "SurfaceChasm",
    "SurfaceHellfire",
    "SurfaceCausticBrine",
    "SurfaceBloodExploding",
    "SurfaceAsh",
    "SurfaceSpikeGrowth",
    "SurfaceHolyFire",
    "SurfaceBlackTentacles",
    "SurfaceOvergrowth",
    "SurfacePurpleWormPoison",
    "SurfaceSerpentVenom",
    "SurfaceInvisibleGithAcid",
    "SurfaceBladeBarrier",
    "SurfaceSewer",
    "SurfaceWaterCloud",
    "SurfaceWaterCloudElectrified",
    "SurfacePoisonCloud",
    "SurfaceExplosionCloud",
    "SurfaceShockwaveCloud",
    "SurfaceCloudkillCloud",
    "SurfaceMaliceCloud",
    "SurfaceBloodCloud",
    "SurfaceStinkingCloud",
    "SurfaceDarknessCloud",
    "SurfaceFogCloud",
    "SurfaceGithPheromoneGasCloud",
    "SurfaceSporeWhiteCloud",
    "SurfaceSporeGreenCloud",
    "SurfaceSporeBlackCloud",
    "SurfaceDrowPoisonCloud",
    "SurfaceIceCloud",
    "SurfacePotionHealingCloud",
    "SurfacePotionHealingGreaterCloud",
    "SurfacePotionHealingSuperiorCloud",
    "SurfacePotionHealingSupremeCloud",
    "SurfacePotionInvisibilityCloud",
    "SurfacePotionSpeedCloud",
    "SurfacePotionVitalityCloud",
    "SurfacePotionAntitoxinCloud",
    "SurfacePotionResistanceAcidCloud",
    "SurfacePotionResistanceColdCloud",
    "SurfacePotionResistanceFireCloud",
    "SurfacePotionResistanceForceCloud",
    "SurfacePotionResistanceLightningCloud",
    "SurfacePotionResistancePoisonCloud",
    "SurfaceSporePinkCloud",
    "SurfaceBlackPowderDetonationCloud",
    "SurfaceVoidCloud",
    "SurfaceCrawlerMucusCloud",
    "SurfaceCloudkill6Cloud",
];

/// A surface type (`eoc::ESurfaceType`, an index into [`SURFACE_TYPES`]).
pub type SurfaceType = u8;

pub const NONE: SurfaceType = 0;
pub const BLOOD: SurfaceType = 4;
/// The last ground type (`SurfaceSewer`); cloud types follow.
pub const LAST_GROUND: SurfaceType = 38;

/// The type called `name` (`SurfaceBlood`, …), case-insensitively.
pub fn type_index(name: &str) -> Option<SurfaceType> {
    SURFACE_TYPES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| i as SurfaceType)
}

pub fn type_name(ty: SurfaceType) -> &'static str {
    SURFACE_TYPES.get(usize::from(ty)).copied().unwrap_or("")
}

/// `eoc::ESurfaceLayer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceLayer {
    Ground,
    Cloud,
}

impl SurfaceLayer {
    pub fn of(ty: SurfaceType) -> Self {
        if ty > LAST_GROUND {
            Self::Cloud
        } else {
            Self::Ground
        }
    }

    /// The layer's value in a tile's flags: `type << 24` on the ground, `(type − 38) << 32`
    /// for clouds.
    // bg3: eoc::GetSurfacePaintedState (masks 0xFF00_0000 and 0xFF_0000_0000),
    // esv::CreatePuddleAction::Grow (4.1.1.7631656)
    pub fn painted(self, ty: SurfaceType) -> u64 {
        match self {
            Self::Ground => u64::from(ty) << 24,
            Self::Cloud => u64::from(ty.saturating_sub(LAST_GROUND)) << 32,
        }
    }

    /// The painted type of this layer in tile flags.
    pub fn painted_type(self, flags: u64) -> SurfaceType {
        match self {
            Self::Ground => (flags >> 24 & 0xff) as SurfaceType,
            Self::Cloud => match (flags >> 32 & 0xff) as SurfaceType {
                0 => NONE,
                v => v + LAST_GROUND,
            },
        }
    }

    /// Tile flags on which this layer gets no new cells: static blockers (bits 2, 3, 23),
    /// the layer's own "no surface" bit (9 ground, 10 cloud) and bit 62/63.
    // bg3: esv::CreatePuddleAction::Enter, the neighbour callback of
    // esv::CreatePuddleAction::Grow (4.1.1.7631656) — 0x80000C | 0x200 | 1 << 62 (ground),
    // 0x80000C | 0x400 | 1 << 63 (cloud); the high bit is left out when the action ignores
    // irreplaceable surfaces.
    pub fn block_mask(self) -> u64 {
        let base = 0x0080_000C;
        match self {
            Self::Ground => base | 1 << 9 | 1 << 62,
            Self::Cloud => base | 1 << 10 | 1 << 63,
        }
    }
}

/// Tile flag bit 54: a static blocker; surface growth drops such tiles.
// bg3: esv::CreatePuddleAction::Grow (4.1.1.7631656)
pub const TILE_BLOCKER: u64 = 1 << 54;

/// An AI grid tile, as the grid identifies it.
pub type Cell = u64;

/// The level's AI grid, as surfaces need it. Columns are the global 0.5 m tile lattice
/// (`floor(x·2), floor(z·2)`); several tiles can share a column at different heights.
pub trait SurfaceGrid: Send + Sync {
    /// Tiles of a column (any order).
    fn cells_in_column(&self, column: (i32, i32)) -> Vec<Cell>;
    fn column(&self, cell: Cell) -> (i32, i32);
    /// Height of the tile's surface (world y).
    fn height(&self, cell: Cell) -> f32;
    /// The tile's flags as stored in the level's grid (painted surfaces included).
    fn flags(&self, cell: Cell) -> u64;
    /// Every tile with a painted surface on either layer.
    fn painted_cells(&self) -> Vec<Cell>;
}

impl std::fmt::Debug for dyn SurfaceGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SurfaceGrid")
    }
}

/// Edge length of a cell.
pub const CELL_SIZE: f32 = 0.5;

pub fn column_of(p: [f32; 3]) -> (i32, i32) {
    (
        (p[0] / CELL_SIZE).floor() as i32,
        (p[2] / CELL_SIZE).floor() as i32,
    )
}

/// The tile of a column nearest to `height`.
// bg3: eoc::AiGrid::ToClosestTilePos (4.1.1.7631656) — approximated as nearest height.
pub fn closest_in_column(grid: &dyn SurfaceGrid, column: (i32, i32), height: f32) -> Option<Cell> {
    grid.cells_in_column(column).into_iter().min_by(|&a, &b| {
        (grid.height(a) - height)
            .abs()
            .total_cmp(&(grid.height(b) - height).abs())
    })
}

/// Centre of a cell on its surface.
pub fn cell_pos(grid: &dyn SurfaceGrid, cell: Cell) -> [f32; 3] {
    let (x, z) = grid.column(cell);
    [
        (x as f32 + 0.5) * CELL_SIZE,
        grid.height(cell),
        (z as f32 + 0.5) * CELL_SIZE,
    ]
}

/// The tile at a column offset from `cell`, nearest in height, if within `max_dh`.
fn offset_cell(grid: &dyn SurfaceGrid, cell: Cell, dx: i32, dz: i32, max_dh: f32) -> Option<Cell> {
    let (x, z) = grid.column(cell);
    let h = grid.height(cell);
    closest_in_column(grid, (x + dx, z + dz), h).filter(|&c| (grid.height(c) - h).abs() < max_dh)
}

/// What covers one layer of a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellSurface {
    pub ty: SurfaceType,
    /// Seconds left; negative: permanent.
    pub remaining: f32,
    pub owner: Guid,
}

/// A small deterministic generator for the growth choices (the game uses the C library's
/// `rand`; its sequence is not reproduced). Separate from the dice so that surfaces never
/// change the rules' rolls.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Default for Rng {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl Rng {
    fn next(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

/// `esv::CreatePuddleAction`: a puddle growing from a point.
#[derive(Clone, Debug)]
struct Puddle {
    ty: SurfaceType,
    layer: SurfaceLayer,
    /// Cells still to add (`TotalCells`, counted down).
    remaining: i32,
    cells_at_grow: i32,
    grow_timer: f32,
    timer: f32,
    duration: f32,
    owner: Guid,
    frontier: Vec<Cell>,
    seen: BTreeSet<Cell>,
    finished: bool,
}

/// `esv::RectangleSurfaceAction`: cells of a rectangle (with a random splatter), added in
/// order of distance from the start.
#[derive(Clone, Debug)]
struct Spray {
    ty: SurfaceType,
    cells: Vec<Cell>,
    next: usize,
    grow_step: usize,
    grow_timer: f32,
    timer: f32,
    duration: f32,
    owner: Guid,
}

#[derive(Clone, Debug)]
enum Action {
    Puddle(Puddle),
    Spray(Spray),
}

/// The surfaces of the loaded level.
#[derive(Clone, Debug, Default)]
pub struct SurfaceState {
    grid: Option<Arc<dyn SurfaceGrid>>,
    pub ground: BTreeMap<Cell, CellSurface>,
    pub cloud: BTreeMap<Cell, CellSurface>,
    actions: Vec<Action>,
    rng: Rng,
    /// Death type of a character's killing hit, until its death actions run.
    pub(crate) death_types: BTreeMap<Guid, u8>,
    /// Bumped whenever a cell changes (for presentation caches).
    pub revision: u64,
}

impl SurfaceState {
    pub fn grid(&self) -> Option<&Arc<dyn SurfaceGrid>> {
        self.grid.as_ref()
    }

    pub fn layer(&self, layer: SurfaceLayer) -> &BTreeMap<Cell, CellSurface> {
        match layer {
            SurfaceLayer::Ground => &self.ground,
            SurfaceLayer::Cloud => &self.cloud,
        }
    }

    fn layer_mut(&mut self, layer: SurfaceLayer) -> &mut BTreeMap<Cell, CellSurface> {
        match layer {
            SurfaceLayer::Ground => &mut self.ground,
            SurfaceLayer::Cloud => &mut self.cloud,
        }
    }

    /// Is any surface still being made?
    pub fn is_growing(&self) -> bool {
        !self.actions.is_empty()
    }

    /// Put a surface on a cell. `false` if the tile takes no surface of this layer.
    // bg3: esv::CreateSurfaceActionBase::AddCell (4.1.1.7631656) — *approximate*: the game
    // also runs the type's interactions with what is there; here the new type replaces it.
    fn add_cell(&mut self, cell: Cell, ty: SurfaceType, duration: f32, owner: Guid) -> bool {
        let Some(grid) = self.grid.clone() else {
            return false;
        };
        let layer = SurfaceLayer::of(ty);
        if grid.flags(cell) & layer.block_mask() != 0 {
            return false;
        }
        self.layer_mut(layer).insert(
            cell,
            CellSurface {
                ty,
                remaining: if duration > 0.0 { duration } else { -1.0 },
                owner,
            },
        );
        self.revision += 1;
        true
    }

    /// One growth step of a puddle: up to `count` new cells.
    // bg3: esv::CreatePuddleAction::Grow (4.1.1.7631656) — up to 50 attempts; each takes a
    // random frontier tile out of the frontier, skips blockers (bit 54), adds the tile unless
    // it already has the type (failing to add stops the expansion there), and puts the tile's
    // 4 neighbours within 1.5 m of height that are not blocked for the layer and not seen
    // before on the frontier. Returns whether the frontier still has tiles.
    fn grow_puddle(&mut self, p: &mut Puddle, count: i32) -> bool {
        let Some(grid) = self.grid.clone() else {
            return false;
        };
        let mut added = 0;
        for _ in 0..50 {
            if added >= count || p.frontier.is_empty() {
                break;
            }
            let i = self.rng.below(p.frontier.len());
            let cell = p.frontier.remove(i);
            let flags = grid.flags(cell);
            if flags & TILE_BLOCKER != 0 {
                continue;
            }
            let already = self.layer(p.layer).get(&cell).is_some_and(|s| s.ty == p.ty);
            if !already && !self.add_cell(cell, p.ty, p.duration, p.owner) {
                continue;
            }
            for (dx, dz) in [(0, 1), (-1, 0), (1, 0), (0, -1)] {
                let Some(n) = offset_cell(&*grid, cell, dx, dz, 1.5) else {
                    continue;
                };
                if grid.flags(n) & p.layer.block_mask() != 0 || !p.seen.insert(n) {
                    continue;
                }
                p.frontier.push(n);
            }
            if !already {
                p.remaining -= 1;
                added += 1;
            }
        }
        !p.frontier.is_empty()
    }

    fn update_actions(&mut self, dt: f32) {
        let mut actions = std::mem::take(&mut self.actions);
        actions.retain_mut(|a| match a {
            // bg3: esv::CreatePuddleAction::Update (4.1.1.7631656)
            Action::Puddle(p) => {
                if p.finished {
                    return false;
                }
                if p.timer > 0.0 || p.grow_timer == 0.0 {
                    p.timer -= dt;
                    if p.timer <= 0.0 {
                        let more = self.grow_puddle(p, p.cells_at_grow);
                        p.finished = !more || p.remaining < 1;
                        p.timer = p.grow_timer;
                    }
                }
                !p.finished
            }
            // bg3: esv::RectangleSurfaceAction::Update (4.1.1.7631656)
            Action::Spray(s) => {
                s.timer -= dt;
                while s.timer <= 0.0 && s.next < s.cells.len() {
                    let end = (s.next + s.grow_step).min(s.cells.len());
                    for i in s.next..end {
                        self.add_cell(s.cells[i], s.ty, s.duration, s.owner);
                    }
                    s.next = end;
                    s.timer += s.grow_timer;
                }
                s.next < s.cells.len()
            }
        });
        // Actions started while updating (none today) would have been pushed to self.actions.
        actions.append(&mut self.actions);
        self.actions = actions;
    }

    fn update_lifetimes(&mut self, dt: f32) {
        for layer in [SurfaceLayer::Ground, SurfaceLayer::Cloud] {
            let map = self.layer_mut(layer);
            let before = map.len();
            map.retain(|_, s| {
                if s.remaining < 0.0 {
                    return true;
                }
                s.remaining -= dt;
                s.remaining > 0.0
            });
            if map.len() != before {
                self.revision += 1;
            }
        }
    }
}

/// A puddle's parameters (`eoc::CreatePuddleActionData`, `CreatePuddle` in the story).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PuddleParams {
    pub ty: SurfaceType,
    pub total_cells: i32,
    pub cells_at_grow: i32,
    pub grow_timer: f32,
    /// Seconds; ≤ 0: permanent.
    pub duration: f32,
    /// Delay before the first step after the immediate one (`Timeout`).
    pub timeout: f32,
}

/// Search radius for a puddle's first tile.
// bg3: esv::CreatePuddleAction::Enter (4.1.1.7631656) — eoc::AiGrid::GetNearestSpiral, 5 m.
const PUDDLE_SEARCH_RADIUS: f32 = 5.0;

impl Sim {
    /// Supply the level's AI grid. Its painted surfaces become the level's surfaces (and
    /// whatever was there before is dropped). `None` removes surfaces altogether.
    // bg3: esv::SurfaceManager::Update with esv::surface::LevelLoadedComponent — the painted
    // state of the loaded grid becomes surfaces (4.1.1.7631656). *Assumption*: painted
    // surfaces are permanent.
    pub fn set_surface_grid(&mut self, grid: Option<Arc<dyn SurfaceGrid>>) {
        let s = &mut self.surfaces;
        s.ground.clear();
        s.cloud.clear();
        s.actions.clear();
        s.revision += 1;
        s.grid = grid.clone();
        let Some(grid) = grid else {
            return;
        };
        for cell in grid.painted_cells() {
            let flags = grid.flags(cell);
            for layer in [SurfaceLayer::Ground, SurfaceLayer::Cloud] {
                let ty = layer.painted_type(flags);
                if ty != NONE && usize::from(ty) < SURFACE_TYPES.len() {
                    s.layer_mut(layer).insert(
                        cell,
                        CellSurface {
                            ty,
                            remaining: -1.0,
                            owner: Guid::NULL,
                        },
                    );
                }
            }
        }
    }

    /// The surface type on a layer at a position (the cell of its column nearest in height,
    /// within 1 m).
    pub fn surface_at(&self, pos: [f32; 3], layer: SurfaceLayer) -> SurfaceType {
        let Some(grid) = self.surfaces.grid.as_deref() else {
            return NONE;
        };
        closest_in_column(grid, column_of(pos), pos[1])
            .filter(|&c| (grid.height(c) - pos[1]).abs() <= 1.0)
            .and_then(|c| self.surfaces.layer(layer).get(&c))
            .map_or(NONE, |s| s.ty)
    }

    /// Start a puddle at `pos`.
    // bg3: esv::CreatePuddleAction::Enter (4.1.1.7631656) — the nearest usable tile within
    // 5 m starts the frontier, then a first step of 2 cells; the next step comes after
    // `Timeout`, then one every `GrowTimer` seconds.
    pub fn create_puddle(&mut self, pos: [f32; 3], p: PuddleParams, owner: Guid) -> bool {
        let Some(grid) = self.surfaces.grid.clone() else {
            return false;
        };
        if p.ty == NONE || usize::from(p.ty) >= SURFACE_TYPES.len() {
            return false;
        }
        let layer = SurfaceLayer::of(p.ty);
        let Some(start) = nearest_open_cell(&*grid, pos, layer) else {
            return false;
        };
        let mut puddle = Puddle {
            ty: p.ty,
            layer,
            remaining: p.total_cells,
            cells_at_grow: p.cells_at_grow,
            grow_timer: p.grow_timer,
            timer: p.timeout + 1e-7,
            duration: p.duration,
            owner,
            frontier: vec![start],
            seen: BTreeSet::from([start]),
            finished: false,
        };
        let more = self.surfaces.grow_puddle(&mut puddle, 2);
        puddle.finished = !more || puddle.remaining < 1;
        if !puddle.finished {
            self.surfaces.actions.push(Action::Puddle(puddle));
        }
        true
    }

    /// A spray along `dir` (horizontal), `length` long and `width` wide, starting at `start`.
    // bg3: esv::RectangleSurfaceAction::Enter / ::Grow (4.1.1.7631656) — the tiles of a
    // rotated rectangle (eoc::SurfaceShapes::CreateSurfaceRotatedRect), each spreading with
    // probability 1/3 to a random diagonal neighbour over three rounds, sorted by distance from
    // the start and added `grow_step` cells every `grow_timer` seconds. *Approximate*: the
    // rasterisation (tile centres inside the rectangle, plus the tiles under its centre line),
    // a single diagonal step per chosen tile and round, and no line-of-sight check from the
    // start.
    #[allow(clippy::too_many_arguments)]
    pub fn create_spray(
        &mut self,
        start: [f32; 3],
        dir: [f32; 3],
        length: f32,
        width: f32,
        height: f32,
        ty: SurfaceType,
        duration: f32,
        owner: Guid,
    ) -> bool {
        let Some(grid) = self.surfaces.grid.clone() else {
            return false;
        };
        if ty == NONE || usize::from(ty) >= SURFACE_TYPES.len() {
            return false;
        }
        let len = (dir[0] * dir[0] + dir[2] * dir[2]).sqrt();
        if len <= 1e-6 {
            return false;
        }
        let (fx, fz) = (dir[0] / len, dir[2] / len);
        let mut cells = rect_cells(&*grid, start, (fx, fz), length, width, height);
        let mut seen: BTreeSet<Cell> = cells.iter().copied().collect();
        let mut generation = cells.clone();
        for _ in 0..3 {
            let mut next = Vec::new();
            for &c in &generation {
                if self.surfaces.rng.below(3) != 0 {
                    continue;
                }
                let dx = if self.surfaces.rng.next() & 1 == 1 {
                    1
                } else {
                    -1
                };
                let dz = if self.surfaces.rng.next() & 1 == 1 {
                    1
                } else {
                    -1
                };
                if let Some(n) = offset_cell(&*grid, c, dx, dz, height.max(0.5))
                    && seen.insert(n)
                {
                    next.push(n);
                }
            }
            cells.extend(&next);
            generation = next;
        }
        let origin = column_of(start);
        cells.sort_by_key(|&c| {
            let (x, z) = grid.column(c);
            let (dx, dz) = (i64::from(x - origin.0), i64::from(z - origin.1));
            (dx * dx + dz * dz, c)
        });
        self.surfaces.actions.push(Action::Spray(Spray {
            ty,
            cells,
            next: 0,
            grow_step: 10,
            grow_timer: 0.01,
            timer: 0.0,
            duration,
            owner,
        }));
        self.update_surfaces(0.0);
        true
    }

    /// Cover the connected tiles within `radius` of `pos` at once (`CreateSurface` in the
    /// story).
    // bg3: esv::CreateSurfaceAction (4.1.1.7631656) — *approximate*: the game grows the disc
    // outwards over a short time; here it appears at once.
    pub fn create_surface(
        &mut self,
        pos: [f32; 3],
        ty: SurfaceType,
        radius: f32,
        duration: f32,
        owner: Guid,
    ) -> bool {
        let Some(grid) = self.surfaces.grid.clone() else {
            return false;
        };
        if usize::from(ty) >= SURFACE_TYPES.len() {
            return false;
        }
        let cells = disc_cells(&*grid, pos, radius.max(CELL_SIZE * 0.5));
        if ty == NONE {
            // `SurfaceNone` clears what is there (both layers).
            for c in cells {
                for layer in [SurfaceLayer::Ground, SurfaceLayer::Cloud] {
                    if self.surfaces.layer_mut(layer).remove(&c).is_some() {
                        self.surfaces.revision += 1;
                    }
                }
            }
            return true;
        }
        for c in cells {
            self.surfaces.add_cell(c, ty, duration, owner);
        }
        true
    }

    /// Remove a layer within `radius` of `pos` (`RemoveSurfaceLayer`).
    pub fn remove_surface_layer(&mut self, pos: [f32; 3], layer: SurfaceLayer, radius: f32) {
        let Some(grid) = self.surfaces.grid.clone() else {
            return;
        };
        for c in disc_cells(&*grid, pos, radius) {
            if self.surfaces.layer_mut(layer).remove(&c).is_some() {
                self.surfaces.revision += 1;
            }
        }
    }

    /// Advance surface actions and lifetimes.
    pub fn update_surfaces(&mut self, dt: f64) {
        let dt = dt.max(0.0) as f32;
        self.surfaces.update_actions(dt);
        if dt > 0.0 {
            self.surfaces.update_lifetimes(dt);
        }
    }

    /// Cells with a surface on a layer: world position (cell centre on the surface) and type.
    pub fn surface_cells(&self, layer: SurfaceLayer) -> Vec<([f32; 3], SurfaceType)> {
        let Some(grid) = self.surfaces.grid.as_deref() else {
            return Vec::new();
        };
        self.surfaces
            .layer(layer)
            .iter()
            .map(|(&c, s)| (cell_pos(grid, c), s.ty))
            .collect()
    }
}

/// The nearest tile to `pos` that takes a surface on `layer` (spiral search within 5 m, by
/// horizontal distance; tiles within 2 m of height).
fn nearest_open_cell(grid: &dyn SurfaceGrid, pos: [f32; 3], layer: SurfaceLayer) -> Option<Cell> {
    let (cx, cz) = column_of(pos);
    let r = (PUDDLE_SEARCH_RADIUS / CELL_SIZE).ceil() as i32;
    let mut best: Option<(f32, Cell)> = None;
    for dz in -r..=r {
        for dx in -r..=r {
            let Some(c) = closest_in_column(grid, (cx + dx, cz + dz), pos[1]) else {
                continue;
            };
            if (grid.height(c) - pos[1]).abs() > 2.0
                || grid.flags(c) & (layer.block_mask() | TILE_BLOCKER) != 0
            {
                continue;
            }
            let p = cell_pos(grid, c);
            let d = (p[0] - pos[0]).powi(2) + (p[2] - pos[2]).powi(2);
            if d <= PUDDLE_SEARCH_RADIUS * PUDDLE_SEARCH_RADIUS && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, c));
            }
        }
    }
    best.map(|(_, c)| c)
}

/// Tiles of a horizontal rectangle from `start` along `(fx, fz)`.
fn rect_cells(
    grid: &dyn SurfaceGrid,
    start: [f32; 3],
    (fx, fz): (f32, f32),
    length: f32,
    width: f32,
    height: f32,
) -> Vec<Cell> {
    let (sx, sz) = (-fz, fx);
    let half = width * 0.5;
    let reach = length + width;
    let (c0x, c0z) = column_of([start[0] - reach, 0.0, start[2] - reach]);
    let (c1x, c1z) = column_of([start[0] + reach, 0.0, start[2] + reach]);
    let mut out = BTreeSet::new();
    for z in c0z..=c1z {
        for x in c0x..=c1x {
            let p = [(x as f32 + 0.5) * CELL_SIZE, (z as f32 + 0.5) * CELL_SIZE];
            let (rx, rz) = (p[0] - start[0], p[1] - start[2]);
            let along = rx * fx + rz * fz;
            let side = rx * sx + rz * sz;
            if (0.0..=length).contains(&along) && side.abs() <= half {
                out.extend(column_cell(grid, (x, z), start[1], height));
            }
        }
    }
    let steps = (length / (CELL_SIZE * 0.5)).ceil() as i32;
    for i in 0..=steps {
        let t = length * i as f32 / steps.max(1) as f32;
        let p = [start[0] + fx * t, start[1], start[2] + fz * t];
        out.extend(column_cell(grid, column_of(p), start[1], height));
    }
    out.into_iter().collect()
}

fn column_cell(grid: &dyn SurfaceGrid, col: (i32, i32), y: f32, height: f32) -> Option<Cell> {
    closest_in_column(grid, col, y).filter(|&c| (grid.height(c) - y).abs() <= height.max(0.5))
}

/// Tiles within `radius` of `pos` connected to its tile (4-neighbours within 1.5 m of
/// height).
fn disc_cells(grid: &dyn SurfaceGrid, pos: [f32; 3], radius: f32) -> Vec<Cell> {
    let Some(start) = closest_in_column(grid, column_of(pos), pos[1])
        .filter(|&c| (grid.height(c) - pos[1]).abs() <= 2.0)
    else {
        return Vec::new();
    };
    let mut seen = BTreeSet::from([start]);
    let mut queue = vec![start];
    let mut out = Vec::new();
    while let Some(c) = queue.pop() {
        let p = cell_pos(grid, c);
        if (p[0] - pos[0]).powi(2) + (p[2] - pos[2]).powi(2) > radius * radius {
            continue;
        }
        out.push(c);
        for (dx, dz) in [(0, 1), (-1, 0), (1, 0), (0, -1)] {
            if let Some(n) = offset_cell(grid, c, dx, dz, 1.5)
                && seen.insert(n)
            {
                queue.push(n);
            }
        }
    }
    out.sort_unstable();
    out
}

// ---- blood and death ---------------------------------------------------------------------

/// A `CreatePuddle` death action (`OnDeathActions`, `ActionType` 10 →
/// `eoc::CreatePuddleActionData`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeathPuddle {
    pub surface: String,
    pub total_cells: i32,
    pub cells_at_grow: i32,
    pub grow_timer: f32,
    /// `LifeTime` in turns; negative: permanent.
    pub lifetime: f32,
    pub timeout: f32,
    /// `ApplyDeathTypeBloodCheck` (default true): skip a blood puddle when the death type
    /// leaves no blood.
    pub blood_check: bool,
}

/// `IActionData` type of `CreatePuddleActionData` in `OnDeathActions`.
const ACTION_CREATE_PUDDLE: i64 = 10;

/// Blood and death data of a character template or placed character; `None`: not set here
/// (inherited).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BloodInfo {
    /// `BloodSurfaceType`.
    pub blood_surface: Option<String>,
    /// `OnDeathActions` (the node present, even empty, replaces the inherited list:
    /// **assumption**, the template field rule).
    pub death_puddles: Option<Vec<DeathPuddle>>,
    /// `Bounds`: radius and height of the `Type` 2 bound (`eoc::EBoundType`).
    pub bound: Option<(f32, f32)>,
}

impl BloodInfo {
    // bg3: eoc::CharacterTemplate::Visit (BloodSurfaceType, OnDeathActions),
    // eoc::CreatePuddleActionData::Visit (4.1.1.7631656) — fields SurfaceType, LifeTime,
    // Timeout, TotalCells, CellAtGrow, GrowTimer, ExternalCauseAsSurfaceOwner (default true),
    // ApplyDeathTypeBloodCheck (default true).
    pub fn read(n: &bg3_lsf::Node) -> Option<Self> {
        let f = |n: &bg3_lsf::Node, k: &str| n.attr(k).and_then(|v| v.as_f64());
        let blood_surface = n
            .attr("BloodSurfaceType")
            .map(ToString::to_string)
            .filter(|s| !s.is_empty());
        let death_puddles = n.child("OnDeathActions").map(|d| {
            d.children_named("Action")
                .filter(|a| {
                    a.attr("ActionType").and_then(|v| v.as_i64()) == Some(ACTION_CREATE_PUDDLE)
                })
                .filter_map(|a| {
                    let at = a.child("Attributes")?;
                    Some(DeathPuddle {
                        surface: at.attr("SurfaceType")?.to_string(),
                        total_cells: f(at, "TotalCells").unwrap_or(0.0) as i32,
                        cells_at_grow: f(at, "CellAtGrow").unwrap_or(0.0) as i32,
                        grow_timer: f(at, "GrowTimer").unwrap_or(0.0) as f32,
                        lifetime: f(at, "LifeTime").unwrap_or(0.0) as f32,
                        timeout: f(at, "Timeout").unwrap_or(0.0) as f32,
                        blood_check: at
                            .attr("ApplyDeathTypeBloodCheck")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                    })
                })
                .collect()
        });
        let bound = n.child("Bounds").and_then(|b| {
            b.children_named("Bound")
                .find(|b| b.attr("Type").and_then(|v| v.as_i64()) == Some(2))
                .map(|b| {
                    (
                        f(b, "Radius").unwrap_or(0.5) as f32,
                        f(b, "Height").unwrap_or(2.0) as f32,
                    )
                })
        });
        let out = Self {
            blood_surface,
            death_puddles,
            bound,
        };
        (out != Self::default()).then_some(out)
    }
}

/// `EDeathType` values the death rules look at.
// bg3: StringToDeathType (4.1.1.7631656) — None 0, Acid 1, Chasm 2, DoT 3, Electrocution 4,
// Explode 5, Falling 6, Incinerate 7, KnockedDown 8, Lifetime 9, Necrotic 10, Physical 11,
// Psychic 12, Radiant 13, CinematicDeath 14, Cold 15, Disintegrate 16.
pub mod death_type {
    pub const ACID: u8 = 1;
    pub const CHASM: u8 = 2;
    pub const DOT: u8 = 3;
    pub const ELECTROCUTION: u8 = 4;
    pub const INCINERATE: u8 = 7;
    pub const NECROTIC: u8 = 10;
    pub const PHYSICAL: u8 = 11;
    pub const PSYCHIC: u8 = 12;
    pub const RADIANT: u8 = 13;
    pub const COLD: u8 = 15;

    /// Death types after which a blood-typed death puddle is still made.
    // bg3: esv::ActionHelpers::ExecuteAction (4.1.1.7631656) — bit mask 0xDB69 over the
    // death type (types above 16 always bleed).
    pub fn bleeds(death_type: u8) -> bool {
        death_type > 16 || 0xDB69u32 >> death_type & 1 == 1
    }

    /// The death type of a kill by a damage type. **Assumption**: the hit's death type
    /// follows its damage type like this (physical damage: `Physical`); the sim does not
    /// read the spells' `DeathType` fields.
    pub fn from_damage(damage: &str) -> u8 {
        match damage {
            "Acid" => ACID,
            "Fire" => INCINERATE,
            "Lightning" | "Thunder" => ELECTROCUTION,
            "Necrotic" => NECROTIC,
            "Radiant" => RADIANT,
            "Cold" => COLD,
            "Psychic" => PSYCHIC,
            "Poison" => DOT,
            _ => PHYSICAL,
        }
    }
}

impl Sim {
    /// A character's blood data: its own, then its template chain's (first set wins).
    pub fn blood_info(&self, id: Guid) -> BloodInfo {
        let mut out = BloodInfo::default();
        let e = self.entity(id);
        let chain = self.data.template_chain(e.and_then(|e| e.template));
        let sources = std::iter::once(id).chain(chain.iter().map(|t| t.id));
        for src in sources {
            let Some(b) = self.data.blood.get(&src) else {
                continue;
            };
            if out.blood_surface.is_none() {
                out.blood_surface.clone_from(&b.blood_surface);
            }
            if out.death_puddles.is_none() {
                out.death_puddles.clone_from(&b.death_puddles);
            }
            if out.bound.is_none() {
                out.bound = b.bound;
            }
        }
        out
    }

    fn blood_type(&self, id: Guid) -> SurfaceType {
        self.blood_info(id)
            .blood_surface
            .as_deref()
            .and_then(type_index)
            .unwrap_or(NONE)
    }

    /// Blood from a hit: physical damage on a character with a blood type sprays blood away
    /// from the attacker (slashing, bludgeoning) or leaves a small puddle (piercing).
    // bg3: the eoc::ObjectHitEvent handler of esv::CharacterManager (4.1.1.7631656): needs a
    // main damage type of Slashing, Piercing or Bludgeoning, a death type other than Chasm and
    // a blood type other than None. Slashing > 0 or Bludgeoning ≥ 1: a
    // RectangleSurfaceAction from the target 0.25·L behind it along the attacker's facing,
    // L = min(r, 1.5) + 2 long and min(r, 1.5)·0.5 wide (r: the target's type-2 bound radius),
    // 10 cells per 0.01 s; else Piercing ≥ 1: a puddle of n² cells, 2n per step, a step every
    // 0.1/n s, n = max(1, round(2r)). *Assumption*: the attacker faces the target when the hit
    // lands, so its facing is the attacker→target direction.
    pub fn hit_blood(&mut self, attacker: Guid, target: Guid, damage: &str, amount: i32) {
        if amount <= 0 || self.surfaces.grid.is_none() {
            return;
        }
        let blood = self.blood_type(target);
        if blood == NONE || blood == 0xFF {
            return;
        }
        let Some(pos) = self.entity(target).map(|e| e.position) else {
            return;
        };
        let (radius, height) = self.blood_info(target).bound.unwrap_or((0.5, 2.0));
        match damage {
            "Slashing" | "Bludgeoning" => {
                let from = self.entity(attacker).map_or(pos, |e| e.position);
                let mut dir = [pos[0] - from[0], 0.0, pos[2] - from[2]];
                let len = (dir[0] * dir[0] + dir[2] * dir[2]).sqrt();
                if len < 1e-4 {
                    // No attacker position to go by: straight along +z (**assumption**; the
                    // game uses the target's own facing, which the sim doesn't keep).
                    dir = [0.0, 0.0, 1.0];
                } else {
                    dir = [dir[0] / len, 0.0, dir[2] / len];
                }
                let r = radius.min(1.5);
                let length = r + 2.0;
                let start = [
                    pos[0] - dir[0] * length * 0.25,
                    pos[1],
                    pos[2] - dir[2] * length * 0.25,
                ];
                self.create_spray(start, dir, length, r * 0.5, height, blood, -1.0, target);
            }
            "Piercing" => {
                let n = ((radius * 2.0).round() as i32).max(1);
                self.create_puddle(
                    pos,
                    PuddleParams {
                        ty: blood,
                        total_cells: n * n,
                        cells_at_grow: 2 * n,
                        grow_timer: 0.1 / n as f32,
                        duration: -1.0,
                        timeout: 0.0,
                    },
                    target,
                );
            }
            _ => {}
        }
    }

    /// A character's death actions: its `CreatePuddle` death puddles (blood under the body).
    /// Not for characters that start the level dead (the game's death-action query excludes
    /// `eoc::death::DeadByDefaultComponent` and characters lying on something).
    // bg3: esv::CharacterManager::Update (DiedEvent query without DeadByDefault/repose
    // components, esv::GetDeathActions, esv::ActionHelpers::ExecuteAction) (4.1.1.7631656):
    // duration = LifeTime·6 s (negative: permanent); with ApplyDeathTypeBloodCheck, a puddle
    // of the character's own blood type is skipped when the death type doesn't bleed.
    pub fn death_blood(&mut self, victim: Guid, death_type: u8) {
        if self.surfaces.grid.is_none() {
            return;
        }
        let Some(pos) = self.entity(victim).map(|e| e.position) else {
            return;
        };
        let info = self.blood_info(victim);
        let blood = info
            .blood_surface
            .as_deref()
            .and_then(type_index)
            .unwrap_or(NONE);
        for d in info.death_puddles.unwrap_or_default() {
            let Some(ty) = type_index(&d.surface) else {
                continue;
            };
            if d.blood_check && blood != NONE && ty == blood && !death_type::bleeds(death_type) {
                continue;
            }
            self.create_puddle(
                pos,
                PuddleParams {
                    ty,
                    total_cells: d.total_cells,
                    cells_at_grow: d.cells_at_grow,
                    grow_timer: d.grow_timer,
                    duration: if d.lifetime < 0.0 {
                        -1.0
                    } else {
                        d.lifetime * 6.0
                    },
                    timeout: d.timeout,
                },
                victim,
            );
        }
    }
}

// ---- story functions ---------------------------------------------------------------------

pub const CALLS: &[(&str, usize)] = &[
    ("CreateSurface", 4),
    ("CreateSurface", 5),
    ("CreateSurfaceAtPosition", 6),
    ("CreateSurfaceAtPosition", 7),
    ("CreatePuddle", 7),
    ("RemoveSurfaceLayer", 3),
    ("RemoveSurfaceLayerAtPosition", 5),
];

pub const QUERIES: &[(&str, usize)] = &[("GetSurfaceGroundAt", 2), ("GetSurfaceCloudAt", 2)];

fn arg_text(ctx: &bg3_osiris::HostCtx<'_>, v: &bg3_osiris::Value) -> String {
    ctx.text(v).unwrap_or_default().to_owned()
}

fn arg_guid(ctx: &bg3_osiris::HostCtx<'_>, v: &bg3_osiris::Value) -> Guid {
    ctx.text(v)
        .and_then(Guid::from_osiris)
        .unwrap_or(Guid::NULL)
}

fn arg_real(v: &bg3_osiris::Value) -> f32 {
    use bg3_osiris::Data;
    match v.data {
        Data::Int(i) => i as f32,
        Data::Int64(i) => i as f32,
        Data::Real(r) => r,
        _ => 0.0,
    }
}

/// A story duration in turns as seconds (negative: permanent). **Assumption**: like the
/// death actions' `LifeTime`, one turn is 6 s.
fn turns(v: f32) -> f32 {
    if v < 0.0 { -1.0 } else { v * 6.0 }
}

fn layer_named(name: &str) -> Option<SurfaceLayer> {
    match name {
        n if n.eq_ignore_ascii_case("Ground") => Some(SurfaceLayer::Ground),
        n if n.eq_ignore_ascii_case("Cloud") => Some(SurfaceLayer::Cloud),
        _ => None,
    }
}

pub(crate) fn call(
    sim: &mut Sim,
    ctx: &mut bg3_osiris::HostCtx<'_>,
    name: &str,
    args: &[bg3_osiris::Value],
) -> Option<bool> {
    if !CALLS.contains(&(name, args.len())) {
        return None;
    }
    let pos_of = |sim: &Sim, ctx: &bg3_osiris::HostCtx<'_>, v| {
        sim.entity(arg_guid(ctx, v)).map(|e| e.position)
    };
    let xyz = |a: &[bg3_osiris::Value]| [arg_real(&a[0]), arg_real(&a[1]), arg_real(&a[2])];
    Some(match name {
        // CreateSurface(object, type, radius, duration[, owner])
        // bg3: esv::OsirisGameFunctions::CreateSurface (4.1.1.7631656)
        "CreateSurface" => {
            let Some(pos) = pos_of(sim, ctx, &args[0]) else {
                return Some(false);
            };
            let Some(ty) = type_index(&arg_text(ctx, &args[1])) else {
                return Some(false);
            };
            let owner = args.get(4).map_or(Guid::NULL, |v| arg_guid(ctx, v));
            sim.create_surface(
                pos,
                ty,
                arg_real(&args[2]),
                turns(arg_real(&args[3])),
                owner,
            )
        }
        // CreateSurfaceAtPosition(x, y, z, type, radius, duration[, owner])
        // bg3: esv::OsirisGameFunctions::CreateSurfaceAtPosition (4.1.1.7631656)
        "CreateSurfaceAtPosition" => {
            let Some(ty) = type_index(&arg_text(ctx, &args[3])) else {
                return Some(false);
            };
            let owner = args.get(6).map_or(Guid::NULL, |v| arg_guid(ctx, v));
            sim.create_surface(
                xyz(args),
                ty,
                arg_real(&args[4]),
                turns(arg_real(&args[5])),
                owner,
            )
        }
        // CreatePuddle(object, type, cells min, cells max, grow min, grow max, grow timer):
        // cell and step counts drawn from the ranges. *Approximate*: the draws use the
        // surfaces' generator, not the game's.
        "CreatePuddle" => {
            let Some(pos) = pos_of(sim, ctx, &args[0]) else {
                return Some(false);
            };
            let Some(ty) = type_index(&arg_text(ctx, &args[1])) else {
                return Some(false);
            };
            let mut range = |a: &bg3_osiris::Value, b: &bg3_osiris::Value| {
                let (lo, hi) = (arg_real(a) as i32, arg_real(b) as i32);
                let (lo, hi) = (lo.min(hi), lo.max(hi));
                lo + sim.surfaces.rng.below((hi - lo + 1) as usize) as i32
            };
            let total_cells = range(&args[2], &args[3]);
            let cells_at_grow = range(&args[4], &args[5]);
            let p = PuddleParams {
                ty,
                total_cells,
                cells_at_grow,
                grow_timer: arg_real(&args[6]),
                duration: -1.0,
                timeout: 0.0,
            };
            sim.create_puddle(pos, p, Guid::NULL)
        }
        // RemoveSurfaceLayer(object, "Ground" | "Cloud", radius)
        // bg3: esv::OsirisGameFunctions::RemoveSurfaceLayer (4.1.1.7631656)
        "RemoveSurfaceLayer" => {
            let (Some(pos), Some(layer)) = (
                pos_of(sim, ctx, &args[0]),
                layer_named(&arg_text(ctx, &args[1])),
            ) else {
                return Some(false);
            };
            sim.remove_surface_layer(pos, layer, arg_real(&args[2]));
            true
        }
        // bg3: esv::OsirisGameFunctions::RemoveSurfaceLayerAtPosition (4.1.1.7631656)
        "RemoveSurfaceLayerAtPosition" => {
            let Some(layer) = layer_named(&arg_text(ctx, &args[3])) else {
                return Some(false);
            };
            sim.remove_surface_layer(xyz(args), layer, arg_real(&args[4]));
            true
        }
        _ => unreachable!(),
    })
}

pub(crate) fn query(
    sim: &mut Sim,
    ctx: &mut bg3_osiris::HostCtx<'_>,
    name: &str,
    args: &mut [bg3_osiris::Value],
) -> Option<bool> {
    if !QUERIES.contains(&(name, args.len())) {
        return None;
    }
    // Without a grid the sim can't answer; leave it to the caller.
    sim.surfaces.grid.as_ref()?;
    let pos = sim.entity(arg_guid(ctx, &args[0]))?.position;
    // bg3: esv::OsirisGameFunctions::GetSurfaceGroundAt / GetSurfaceCloudAt (4.1.1.7631656)
    let layer = if name == "GetSurfaceGroundAt" {
        SurfaceLayer::Ground
    } else {
        SurfaceLayer::Cloud
    };
    args[1] = ctx.string(type_name(sim.surface_at(pos, layer)));
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A flat test grid: one tile per column over `w × h` columns at height 0, with optional
    /// per-column flags.
    struct Flat {
        w: i32,
        h: i32,
        flags: HashMap<(i32, i32), u64>,
    }

    impl Flat {
        fn new(w: i32, h: i32) -> Self {
            Self {
                w,
                h,
                flags: HashMap::new(),
            }
        }
        fn key(c: (i32, i32)) -> Cell {
            ((c.0 as u32 as u64) << 32) | c.1 as u32 as u64
        }
    }

    impl SurfaceGrid for Flat {
        fn cells_in_column(&self, c: (i32, i32)) -> Vec<Cell> {
            if c.0 >= 0 && c.1 >= 0 && c.0 < self.w && c.1 < self.h {
                vec![Self::key(c)]
            } else {
                Vec::new()
            }
        }
        fn column(&self, c: Cell) -> (i32, i32) {
            ((c >> 32) as u32 as i32, c as u32 as i32)
        }
        fn height(&self, _: Cell) -> f32 {
            0.0
        }
        fn flags(&self, c: Cell) -> u64 {
            self.flags.get(&self.column(c)).copied().unwrap_or(0)
        }
        fn painted_cells(&self) -> Vec<Cell> {
            self.flags
                .iter()
                .filter(|(_, f)| *f & 0xFF_FF00_0000 != 0)
                .map(|(&c, _)| Self::key(c))
                .collect()
        }
    }

    fn sim_with(grid: Flat) -> Sim {
        let mut s = Sim::default();
        s.set_surface_grid(Some(Arc::new(grid)));
        s
    }

    #[test]
    fn type_names_and_layers() {
        assert_eq!(type_index("SurfaceBlood"), Some(BLOOD));
        assert_eq!(type_index("surfacebloodsilver"), Some(24));
        assert_eq!(type_name(39), "SurfaceWaterCloud");
        assert_eq!(SurfaceLayer::of(38), SurfaceLayer::Ground);
        assert_eq!(SurfaceLayer::of(39), SurfaceLayer::Cloud);
        assert_eq!(SurfaceLayer::Ground.painted(BLOOD), 0x0400_0000);
        assert_eq!(SurfaceLayer::Cloud.painted(39), 1 << 32);
        assert_eq!(SurfaceLayer::Cloud.painted_type(1 << 32), 39);
        assert_eq!(SurfaceLayer::Ground.painted_type(0x1900_0000), 25);
    }

    #[test]
    fn painted_surfaces_load_with_the_grid() {
        let mut g = Flat::new(8, 8);
        g.flags.insert((2, 3), SurfaceLayer::Ground.painted(BLOOD));
        g.flags.insert((4, 4), SurfaceLayer::Cloud.painted(41));
        let s = sim_with(g);
        assert_eq!(s.surface_at([1.25, 0.0, 1.75], SurfaceLayer::Ground), BLOOD);
        assert_eq!(s.surface_at([2.25, 0.0, 2.25], SurfaceLayer::Cloud), 41);
        assert_eq!(s.surface_at([0.25, 0.0, 0.25], SurfaceLayer::Ground), NONE);
    }

    #[test]
    fn puddle_grows_to_its_cell_count_over_time() {
        let mut s = sim_with(Flat::new(20, 20));
        let p = PuddleParams {
            ty: BLOOD,
            total_cells: 10,
            cells_at_grow: 2,
            grow_timer: 0.4,
            duration: -1.0,
            timeout: 0.0,
        };
        assert!(s.create_puddle([5.0, 0.0, 5.0], p, Guid::NULL));
        // The first step adds 2 cells at once.
        assert_eq!(s.surfaces.ground.len(), 2);
        for _ in 0..20 {
            s.update_surfaces(0.1);
        }
        assert_eq!(s.surfaces.ground.len(), 10);
        assert!(!s.surfaces.is_growing());
        // Connected, around the start.
        for (p, ty) in s.surface_cells(SurfaceLayer::Ground) {
            assert_eq!(ty, BLOOD);
            assert!((p[0] - 5.0).abs() < 3.0 && (p[2] - 5.0).abs() < 3.0);
        }
    }

    #[test]
    fn growth_avoids_blocked_tiles() {
        let mut g = Flat::new(10, 10);
        // A wall of "no ground surface" tiles at x = 5.
        for z in 0..10 {
            g.flags.insert((5, z), 1 << 9);
        }
        let mut s = sim_with(g);
        let p = PuddleParams {
            ty: BLOOD,
            total_cells: 60,
            cells_at_grow: 60,
            grow_timer: 0.0,
            duration: -1.0,
            timeout: 0.0,
        };
        s.create_puddle([1.0, 0.0, 2.5], p, Guid::NULL);
        for _ in 0..10 {
            s.update_surfaces(0.1);
        }
        let cols: Vec<_> = s
            .surfaces
            .ground
            .keys()
            .map(|&c| (c >> 32) as i32)
            .collect();
        assert!(cols.iter().all(|&x| x < 5));
        assert_eq!(cols.len(), 50, "fills the 5 × 10 tiles on its side");
    }

    #[test]
    fn spray_runs_along_the_direction() {
        let mut s = sim_with(Flat::new(40, 40));
        s.create_spray(
            [10.0, 0.0, 10.0],
            [1.0, 0.0, 0.0],
            2.5,
            0.25,
            2.0,
            BLOOD,
            -1.0,
            Guid::NULL,
        );
        for _ in 0..10 {
            s.update_surfaces(0.02);
        }
        let cells = s.surface_cells(SurfaceLayer::Ground);
        assert!(cells.len() >= 5);
        let xs: Vec<f32> = cells.iter().map(|(p, _)| p[0]).collect();
        let max = xs.iter().copied().fold(f32::MIN, f32::max);
        let min = xs.iter().copied().fold(f32::MAX, f32::min);
        assert!(max >= 12.0 && min >= 9.0, "{min}..{max}");
        // Splatter stays within three diagonal steps of the line.
        assert!(cells.iter().all(|(p, _)| (p[2] - 10.0).abs() <= 2.0));
    }

    #[test]
    fn remove_layer_and_lifetimes() {
        let mut s = sim_with(Flat::new(20, 20));
        s.create_surface([5.0, 0.0, 5.0], 15, 1.0, 6.0, Guid::NULL);
        assert!(s.surface_at([5.0, 0.0, 5.0], SurfaceLayer::Ground) == 15);
        let n = s.surfaces.ground.len();
        assert!((9..=16).contains(&n), "{n}");
        s.remove_surface_layer([5.0, 0.0, 5.0], SurfaceLayer::Ground, 0.4);
        assert!(s.surfaces.ground.len() < n);
        s.update_surfaces(6.5);
        assert!(s.surfaces.ground.is_empty());
    }

    #[test]
    fn death_types_that_bleed() {
        use death_type::*;
        assert!(bleeds(PHYSICAL) && bleeds(DOT) && bleeds(COLD) && bleeds(PSYCHIC));
        assert!(!bleeds(ACID) && !bleeds(CHASM) && !bleeds(INCINERATE) && !bleeds(RADIANT));
        assert!(!bleeds(ELECTROCUTION) && !bleeds(NECROTIC) && !bleeds(16));
        assert_eq!(from_damage("Slashing"), PHYSICAL);
    }
}
