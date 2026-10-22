# Surfaces

Status: engine behaviour read in the macOS arm64 binary (4.1.1.7631656, targeted
`objdump` of the functions named below plus a small xref scanner; disassembly kept in
`.cache/`), data traced with `bg3x lsx`, shading from the surface graph's compiled shader in
`Materials.metallib` (`bg3x metallib --extract …`, `llvm-dis`). Implemented in
`crates/bg3-sim/src/surface.rs` (model, growth, hit and death blood, story functions),
`crates/bg3-sim-nav` (`GridSurfaces`: the AI grid as the sim's surface grid),
`crates/bg3-world/src/surfaces.rs` (surface templates) and `viewer/src/surfaces.rs` +
`viewer/src/shaders/surfaces.wesl` (rendering).

## Where the Nautiloid's blood comes from

- **Painted into the AI grid.** A level's surfaces ship inside `Ai/aigrid.data`: each tile's
  flags carry the ground surface type in bits 24–31 and the cloud type in bits 32–39
  (`eoc::GetSurfacePaintedState`, masks `0xFF00_0000` / `0xFF_0000_0000`). On the Nautiloid
  (GustavDev grid) that is blood on 46 tiles and silver (mind flayer) blood on 21, all around
  the corpses at the **helm** (y ≈ 17), plus fire (1,558 tiles), ash, alien oil and chasm
  (163,765 tiles: the void under the ship). The lower deck where the imp fight happens
  (y ≈ 1–3) has no painted blood. There is no other surface file in the level directory, no
  `surface` level object, and the tutorial story calls no surface function.
- **Not from the corpses.** Characters that start the level dead (`DefaultState` 7/11/12,
  `eoc::death::DeadByDefaultComponent`) get a died event, but the query that queues death
  actions (`esv::CharacterManager::Update`: `esv::Character`, `DeathComponent`,
  `death::StateComponent`, `DiedEventOneFrameComponent`) excludes `repose::StateComponent` and
  `DeadByDefaultComponent`, so pre-placed corpses never run their `OnDeathActions`.
- **The fight.** The blood pools in the imp-deck reference captures are combat blood: each
  physical hit on a bleeding character leaves blood, and each death runs the template's
  death puddle (below). So at the start of the imp fight the deck is clean; it fills as imps
  die. *Inferred* from the data and code above; no capture of the deck before the fight.

## Model (`esv::SurfaceManager`)

- Surfaces live on the AI grid: one ground and one cloud layer per 0.5 m tile.
  `eoc::ESurfaceType` has 75 values (`bg3_sim::surface::SURFACE_TYPES`, from
  `eoc::GetSurfaceName`); types ≤ 38 (`SurfaceSewer`) are ground, the `…Cloud` types are
  cloud. A tile stores its surface as `type << 24` (ground) or `(type − 38) << 32` (cloud) in
  its flags (`esv::CreatePuddleAction::Grow` builds the same values).
- `eoc::AiGrid::GetSurface`: tile bytes 14–15 hold the runtime surface index (`0xFFFF` none;
  bit 0 set: a single surface whose index is `>> 1`, valid if the layer's flag bits are set;
  else an index into per-tile metadata holding a (ground, cloud) pair). The level files have
  `0xFFFF` everywhere: the surfaces are created at load (`esv::SurfaceManager::Update` with
  `esv::surface::LevelLoadedComponent`) from the painted state.
- Blocking: growth drops tiles with flag bit 54, and a layer takes no cell on tiles with bits
  2, 3, 23, the layer's own bit (9 ground, 10 cloud) or 62/63 (`0x80000C | 0x200 | 1 << 62`
  for ground, `| 0x400 | 1 << 63` for clouds; the high bit is left out when the action
  ignores irreplaceable surfaces).
- Durations: story and template lifetimes are in turns; the engine multiplies by 6 (s);
  negative means permanent.

The sim keeps a map per layer from grid tile to (type, remaining seconds, owner). The grid
comes from the caller (`Sim::set_surface_grid`); its painted cells become permanent surfaces
(**assumption**: painted surfaces don't expire). `Sim::advance` ticks actions and lifetimes.

## Actions

`esv::SurfaceActionFactory::CreateAction` types (`eoc::ESurfaceActionType`): 1 CREATE,
2 CREATE_PUDDLE, 3 REMOVE_SURFACE, 4 ZONE, 5 TRANSFORM, 6 ON_PATH, 7 RECTANGLE, 8 POLYGON,
9 FORCE_CREATE.

**Puddle** (`esv::CreatePuddleAction`, exact): `Enter` finds the nearest usable tile within
5 m (`eoc::AiGrid::GetNearestSpiral`), puts it on the frontier and grows 2 cells at once; the
next step comes after `Timeout`, then `CellAtGrow` cells every `GrowTimer` seconds until
`TotalCells` were added or the frontier is empty. A step makes up to 50 attempts: take a random
frontier tile out of the frontier; skip blockers (bit 54); add the cell unless the tile already
has the type (a failed add stops the expansion there); then queue its 4 neighbours (within
1.5 m of height, not blocked for the layer, not queued before). The game picks with the C
library's `rand`; the sim uses its own generator (separate from the dice, so surfaces never
change the rules' rolls).

**Spray** (`esv::RectangleSurfaceAction`): the tiles of a rotated rectangle
(`eoc::SurfaceShapes::CreateSurfaceRotatedRect`, box min (−w/2, −w/2, 0), max (w/2, w/2, L)
along the direction), then three rounds in which each tile, with probability 1/3, spreads to a
random diagonal neighbour (the new tiles are the next round's candidates); the cells are sorted
by distance from the start and added `grow_step` cells every `grow_timer` seconds, with a line
check from the start. *Approximate*: our rasterisation (tile centres in the rectangle plus the
tiles under its centre line), one diagonal step per chosen tile and round (the game chains up
to four), no line check.

**Disc** (`CreateSurface` in the story, CREATE): *approximate*: all connected tiles within the
radius at once (the game grows the disc over a short time).

## Blood

**Death** (`esv::CharacterManager::Update` → `esv::GetDeathActions` →
`esv::ActionHelpers::ExecuteAction`): a character's template `OnDeathActions` (inherited down
the `ParentTemplateId` chain; **assumption**: an `OnDeathActions` node, even empty, replaces the
inherited list, the sparse-field rule). Action type 10 is `eoc::CreatePuddleActionData`
(`SurfaceType`, `LifeTime`, `Timeout`, `TotalCells`, `CellAtGrow`, `GrowTimer`,
`ExternalCauseAsSurfaceOwner` default true, `ApplyDeathTypeBloodCheck` default true).
`BASE_Bipedal` (imps, humans …) has `SurfaceBlood`, 10 cells, 2 per 0.4 s, permanent; mind
flayers `SurfaceBloodSilver`; intellect devourers 6 cells, 4 per 0.1 s. With the blood check, a
puddle of the character's own `BloodSurfaceType` is skipped when the death type doesn't bleed:
death type bit mask `0xDB69` (Acid, Chasm, Electrocution, Incinerate, Necrotic, Radiant and
Disintegrate leave no blood; `EDeathType` values from `StringToDeathType`). The sim takes the
death type from the killing damage type (**assumption**: physical damage → Physical, fire →
Incinerate, necrotic → Necrotic, …; spells' own `DeathType` fields are not read), Physical for
story kills.

**Hits** (the `eoc::ObjectHitEvent` handler `esv::CharacterManager` registers): a hit on a
character (or item) whose main damage type is Slashing, Piercing or Bludgeoning, whose death
type isn't Chasm, on a target with a blood type (`BloodSurfaceType`, or the shapeshift's) other
than None:

- Slashing > 0 or Bludgeoning ≥ 1: a RECTANGLE along the attacker's facing (the projectile's
  for projectiles; the target's own when there is no attacker) starting 0.25·L behind the
  target, L = min(r, 1.5) + 2 long, min(r, 1.5)·0.5 wide (r: the target's type-2 bound radius,
  0.49 for imps), 10 cells per 0.01 s. **Assumption**: the attacker faces the target when the
  hit lands (the sim keeps no facing), so the direction is attacker → target.
- else Piercing ≥ 1: a puddle of n² cells, 2n per step, a step every 0.1/n s,
  n = max(1, round(2r)).

`bg3x combat TUT_Avernus_C --party S_Player_Laezel,S_Player_ShadowHeart --target
S_FirstFight_Imp_Melee_01 --nav` prints the surfaces after the fight. Not modelled: surface
interactions (fire on oil, freezing, electrifying: a new type replaces the old one),
statuses from surfaces, owners and teams, the cloud layer's own rules, saving/loading.

## Story functions

`CreateSurface(object, type, radius, duration[, owner])`, `CreateSurfaceAtPosition`,
`CreatePuddle(object, type, cells min, max, grow min, max, grow timer)` (counts drawn from the
ranges), `RemoveSurfaceLayer(object, "Ground"|"Cloud", radius)`,
`RemoveSurfaceLayerAtPosition`, `GetSurfaceGroundAt` / `GetSurfaceCloudAt` (answered only with
a grid). The Nautiloid's story calls none of them; they serve later levels
(`docs/research/osiris-engine-api.md`).

## Rendering (`ecl::SurfaceDeferredRenderer`)

The client keeps, per AI subgrid, a cell texture (`_SurfaceMask`, RGBA8, one texel per cell,
rows from the top; `ecl::SurfaceDeferredRenderer::UpdateCell` queues 6-byte changes and
`UpdateTexture` writes them) and draws each surface type as a deferred decal box
(`ls::DecalObject::PrepareDraw`, unit box, `WorldOffsetMatrix`, `_DecalDeferredProperties`)
with the type's `DecalMaterial` (`eoc::SurfaceTemplate`, `Type=surface` root templates).
`_SubgridTranslate`/`_SubgridTexProperties` (size in cells, the drawn type, cell size 0.5) map
world positions to the texture; `_AiHeightAndWalkableMask` holds the subgrid's AI heights
(16-bit: `h·1310.7 + origin y`).

The blood material (`VFX_Surface_Blood_01`) is on `VFX_Surface_AlphaBlend_Fluid`
(`Shared_Surface_VFX_Surface_AlphaBlend_Fluid_ST_DEF_fragmentMain`), per pixel:

1. Discard where nothing receives decals (zero `_DecalReceivedNormal`).
2. **Warp**: `Deformation_Noise` at `rel / Tilling_WarpNoise` (rel: world xz minus the subgrid
   origin), `warp = noise.rg·Intensity − Intensity/2`, pulled towards ½ by
   `(1 − BaseColor.a)·¼` (base colour sampled at `rel / Tilling`).
3. **Coverage** at the warped position: gather the four cells around it; a cell counts
   `saturate(B·(R == type) + A·(G == type))` (two type/amount pairs per cell); discard when
   none of the four has the type; interpolate bilinearly between cell centres.
4. **Height fade**: `1 − smoothstep01((|AI height − y| − 0.1)·40)` (bilinear AI height):
   the surface stays within 0.1–0.125 m of the walkable height.
5. **Edge**: `a = smoothstep(0.6, 0.9, c)`, `b = smoothstep(0.95, 0.96, c)`,
   `s = c·saturate(BaseColor.a + b)`, `t = saturate(a + s)`, `lo = clamp(1 − clamp(Edge_Width,
   0.1, 1), 0.2, 1)`, `hi = clamp(lo + Edge_Range, 0.3, 1)`, alpha =
   `smoothstep(lo, hi, t)·(MinOpacity + BaseColor.a·s·(1 − MinOpacity))`.
6. **Outputs**: base colour `saturate(mix(BaseColor.rgb, EdgeColor, Color_Intensity·(1 − c)))`;
   target 2 (`Physical.g` roughness, `Reflectance`, `Physical.r` metalness); all blended with
   `alpha · distance fade · opacity` (the decals' 60 m fade). The normal: `Normal` plus
   `Wave_Intensity`× the animated `Normal_Waves`, towards up by `Normal_Intensity`, applied as a
   rotation to a base normal built from the AI heights (Sobel over the 3×3 heights, falling
   back to the receiver's normal on steep steps), mixed with the received normal by alpha and
   blended by the distance fade. Iridescence (`Iridescence_Opacity`, 0 for blood) adds a
   reflection-vector lookup.

### In the viewer

The level material shader runs the step per pixel after the level decals
(`shaders/level.wesl` → `apply_surfaces` in `shaders/surfaces.wesl`), on decal receivers only.
`viewer/src/surfaces.rs` keeps one world-space cell texture over the level's AI grid
(mask RGBA8 and height R32F, ≤ 2048² cells, cut around the start point on big levels) at fixed
handles; updates from the sim (`--play`: the frame carries the cells whenever the sim's
surface revision changes) rewrite the textures in place, which keeps the GPU textures and the
materials' bind groups. Without `--play` it shows the grid's painted surfaces. Only types
whose material is on the fluid graph are drawn (blood, silver blood, electrified blood, mud:
5 types); the other graphs (fire, oil, water, clouds …) are not ported, and their cells are
left out of the texture (chasm cells under the deck would otherwise take the deck's texels).
`BG3_NO_SURFACES=1` turns surfaces off; `BG3_SURFACE_DEBUG=1` paints the inputs (red:
coverage, green: height fade, blue: near a surface cell) and logs the cell counts.

Exact: the cell data (two pairs, amount), the warp, the bilinear coverage, the height fade, the
edge shaping and alpha, the colour, physical and reflectance outputs, the distance fade.
Approximate:

- One texture for the whole level instead of one per subgrid: where subgrids overlap (decks
  over terrain), only one cell per column is kept. Neighbours of covered cells get the height
  of their tile nearest the cell.
- The material's texture coordinates start at the world origin, not the subgrid's origin
  (same tiling, another phase of noise and texture).
- Normals: no animated waves; the receiver's normal is the base (no AI-height Sobel normal).
- Types around a pixel are blended in type order (at most 3); the game's draw order of the
  per-type decal boxes isn't established.
- No growth fade-in (amount is 255 as soon as a cell exists) and no fade-out on removal.

Verification: `cargo test -p bg3-sim surface` (types and layers, painted state, puddle growth,
blocking, spray direction, removal and lifetimes, death types), `viewer` unit tests (cell
texture fill), the install test `nautiloid_blood` (`bg3-sim-nav`: painted blood at the helm,
a killed imp's 10-cell puddle, a slashing spray beyond the target), and viewer shots: the helm
(`--camera=-40,21,-381 --look-at=-36,17,-387`) shows the painted pools around the dead
devourers and thralls; `--play --combat-test --combat-autoplay --no-follow
--camera=-36,6,-405 --look-at=-40,2,-398 --shot-after 40` shows the fight's blood next to the
dead imp on the deck.
