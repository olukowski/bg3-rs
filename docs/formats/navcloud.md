# Navigation data: AI grid and nav cloud

BG3 ships two kinds of per-level navigation data. They do different jobs:

| Data | Files | Used for | Crate |
|---|---|---|---|
| **AI grid** | `Mods/<Module>/Levels/<Level>/Ai/aigrid.data` (386 files: every level and sub-level) | Where characters can stand, and the paths they **walk**. Flood fills (AI scoring, spell zones, surfaces) also run on it. | `bg3-nav` (`aigrid`, `grid`, `path`) |
| **Nav cloud** | `Mods/GustavDev/Levels/<Level>/NavCloud/navcloud.{meta,data}` in `Gustav_NavCloud.pak` (only the 9 main levels) | A 3D voxel volume. The `navcloud::PathfindingSystem` serves path requests from spell and throw previews (`ecl::spell_preview::PreviewSystem`), spell casting (`esv::spell_cast::CastSystem`) and following (`esv::following`). | `bg3-nav` (`navcloud`) |

So walking (and click-to-move) runs on the **AI grid**. The nav cloud is not used for it.

Other files in a level's `Ai/` folder: `materialgrid.data` (not decoded yet; likely the ground
material per tile, for footsteps and surfaces), `navigationPortals.lsf` (portal links: jump or
teleport points with `Source`/`Target` positions, cost 100 in the path search) and `chasms.lsf`.
None of them affects whether a tile can be walked on.

Everything below was worked out from the bytes of the reference build (4.1.1.7631656) and by
reading the loader and the path search in the macOS arm64 binary (objdump against the symbol
index). Coordinates are BG3 world space (left-handed, Y up).

## AI grid (`aigrid.data`)

Little-endian throughout.

```
u32      version            21 (current); see "Versions"
char[37] level GUID         36 ASCII characters + NUL
u32      subgrid count
subgrid × count:
  u32      id               eoc::AiSubgridId, unique per file
  u32      width, height    in tiles
  f32[3]   origin           world x, y, z of tile (0,0)'s corner; y is the height base
  char[37] object GUID      what the subgrid was baked from (a terrain, floor mesh, platform)
  char[37] parent GUID      all zero in the reference levels
  block    tiles            width × height × 16 bytes, row-major (x + y·width)
u32      layer count
layer × count:
  u8[16]   layer GUID       binary
  u32      tile count
  tile × count (24 bytes):
    i16 x, i16 y, u32 subgrid id, u64 flags, f32 height (world), u32 unknown
```

**Blocks** (`eoc::LoadCompressedAiData`): `u32 inflated size, u32 stored size`, then *stored*
bytes of zlib data that inflate to an LZ4 block. The LZ4 block decompresses to the payload, whose
size the caller knows (here `width × height × 16`).

**Tile** (16 bytes):

| Offset | Type | Meaning |
|---|---|---|
| 0 | u64 | flags (below) |
| 8 | u16 | height: `origin.y + h · 0.02` m (`eoc::AiGrid::GetHeightAtTile`) |
| 10 | u16 | second height in the same units, usually ≤ the first (meaning unknown) |
| 12 | u32 | metadata index, `0xFFFFFFFF` = none |

**Geometry.** Tiles are 0.5 m squares. The centre of tile (x, y) is
`origin + ((x + 0.5)·0.5, h·0.02, (y + 0.5)·0.5)` (`eoc::AiGrid::ToWorldPos`). The tile `y`
axis runs along world `z`. Subgrid origins lie on multiples of 0.5, so all subgrids share one
global lattice: column = `floor(world·2)`. Big objects are cut into 128 × 128 chunks, 64 m
apart. Subgrids overlap wherever surfaces stack (decks, bridges, terrain under a floor). Each tile
then has its own height, and bit 55 marks the overlap.

**Flags.** These are the bits whose role we know from the code:

| Bit | Role |
|---|---|
| 0 | *Empty*: no tile here. Area scans ignore it (`GetStateInArea`). |
| 2, 7, 54 | In the default path collision mask (static blockers; 2 is set on more than half of all non-empty tiles). |
| 4 | Added to the collision mask in combat. |
| 18 | Added to the mask when the character avoids traps. |
| 22 | Doors: in the mask unless the character's template has `CanWalkThroughDoors`. Not set in the static grid. |
| 49–53 | Climbing/jump connections. The neighbour search turns them into directions. |
| 55 | The tile overlaps a tile of another subgrid. Area scans and neighbour lookups switch to the multi-subgrid path. |

The default collision mask is `0x0040_0000_0040_0084`. It is the first quadword of a constant
that `(anonymous)::CreateUnprocessedSettings` copies into new path settings, and
`esv::aigrid::GetAdjustedCollisionMask` then adjusts it per character as described above.

**Layers.** These are named sets of tile overrides that the game switches on and off at runtime
(`eoc::AiGrid::RequestActivateLayer`, `eoc::AiGridChangeLayersTask`), for example when a bridge
breaks. 220 of the 242 v21 files have none. `bg3-nav` parses them but doesn't apply them.

**Versions.** 242 files are version 21. 144 are older (4, 12–20). These are sub-levels,
platforms (`PLT_*`) and cinematic levels. In them the loader branches on the version: the header
differs below 12, 14 and 20, and the tiles are 24 bytes up to v15, 20 bytes in v16, and different
again in v17/18. The game converts them on load. `bg3-nav` rejects them for now with
`Error::Version`. All main levels are v21.

## Nav cloud (`navcloud.meta` + `navcloud.data`)

```
navcloud.meta:
  u32 version (1), u32 region count
  region × count (28 bytes): i32 x, y, z (region key), u64 offset, u64 size   → into .data
navcloud.data, per region:
  u32 kind, u32 byte count, u8 voxels[byte count]
```

A region is a 6 m cube, keyed by `floor(position / 6)` (`(anonymous)::ToTilePos`). Kind 0 is
coarse, 2³ voxels of 3 m. Kind 1 is fine, 6³ voxels of 1 m. Voxel index is
`(z·n + y)·n + x` (`navcloud::TryGetTileInfo`). Voxel values are 0–3, with 0/1 dominating. The
Nautiloid uses only 0 and 1, and its coarse regions are all 1. The meaning of the values isn't
established; 1 is presumably open space. The search over it uses 6 or 26 neighbour directions
(`navcloud::s_Directions_6`, `s_Directions_26`).

## Path finding on the AI grid

**Faithful (read from the code):**

- **Graph:** 4-connected tiles, expanded in the order of `EGridDirection` 0x40, 0x08, 0x10, 0x02
  (`eoc::AiGrid::FindPath`, inlined in `UpdatePathfinding`). A neighbour inside the same subgrid
  is the adjacent tile. Across subgrids, or from a tile with bit 55, it is the tile of the
  adjacent column at the current height (`eoc::AiGrid::ForEachConnection` →
  `CheckTilesOnAiWorldPos`).
- **Step cost** 10 per tile, 100 through a portal. **Heuristic** `round(2 · |Δ| · 24)`, the 3D
  distance (`eoc::AiGrid::CheckTile`). At 48 per metre against 20 per metre of travel it is
  inadmissible, so the search is greedy-weighted A*. f = g + h. Open lists are binary heaps.
- **Agent footprint:**
  `size = floor(2·(radius − 1e-5) + 0.5)` tiles (`eoc::AiGrid::CreatePath`). A humanoid with
  AI bound radius 0.499 gets size 1. **Collision** ORs the flags of the `(2·size+1)²` tiles around
  a tile (empty tiles excluded) and tests them against the mask (`IsColliding`,
  `GetStateInArea`).
- **Step height:** neighbours may differ by at most the template's `MovementStepUpHeight`
  (default 0.6 m). **Stability** (`IsAreaUnstable`) compares every column in the footprint with
  its left and upper neighbour, and rejects a tile when any difference exceeds the step height.
- **Speeds** (template defaults from `eoc::CharacterTemplate::Visit`): Stroll 1.0, Walk 2.0,
  **Run 3.75**, Sprint 6.0, Dash 6.0 m/s; MaxDashDistance 4.0, StepUpHeight 0.6. Templates
  override them, e.g. Intellect Devourer walk 2 / run 3.75.

**Approximate in `bg3-nav`:**

- The game searches from **both ends** at once: two heaps and per-direction visited bits, with
  the active side switched by a flag on the grid. We search from the start only, so the path can
  differ where several routes cost the same.
- **Height cost** (`(anonymous)::HeightCost`): the game scales `|Δh|` by one of two global
  tunables, depending on whether the step is above or below the step height, and costs drops
  beyond a threshold by interpolating per-path values. The tunables aren't in the data files. We
  use 20 per metre (the horizontal rate) and don't allow drops.
- **Multi-subgrid scans:** for collision and stability we pick, in each column, the tile nearest
  in height within 1 m. The game's per-column choice (`ToClosestTilePos`) isn't fully reversed.
- **Smoothing:** the game runs `SmoothPath_vRecursive`, `_vSubdivide`, `_vCorners`, `_vBrute`
  (selected by global switches), then `InsertApproachNodes`, `InsertTurningNodes`,
  `SplinePath`, `RemoveStraightNodes` and `GenerateCheckpointsBetweenNodes`. We string-pull
  instead: from each anchor we take the furthest tile reachable along a straight line on which
  every quarter-tile sample is standable within the step height.
- **Not modelled:** attack-of-opportunity cost (`AoOCost`), surface/path influences
  (`CalculateInfluenceCost`), dynamic objects and doors, portals and climbing, layers, and the
  "dangerous tile" counter (a second collision test with the extended mask).

**Verified so far:** every v21 grid in the install parses (`every_aigrid_parses`). On the
Nautiloid, 13,683 tiles are standable for a humanoid, and the regions they form match the
visible decks, stairs and holes in the viewer overlay (`bg3-viewer --nav`). A path from the
raised walkway to the deck (`nautiloid_deck_path`) descends the stairs within the step height.
Nothing has yet been compared against paths traced from the running game.

## Tools

- `bg3x nav <level> [--subgrids] [--regions] [--at x,y,z] [--path a --path b …] [--radius r]`
- `bg3-viewer <level> --nav` draws the grid overlay. A left click moves the controlled character
  (`--player NAME`, `--walk-to x,y,z`).
