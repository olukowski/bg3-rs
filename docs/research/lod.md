# Mesh level of detail (LOD)

Status: data side read from the install with `bg3x lsx` / `bg3x gr2 --tree` (4.1.1.7631656,
native Linux build). Runtime selection is **not yet confirmed** in the binary: the Linux `bin/bg3`
has almost no renderer symbols (`rf::*`, `ls::Visual*` are stripped down to a handful), so the
selection rule below is inferred from the data and marked as an assumption. Confirming it needs
the macOS binary (symbol index / REA), see "Open questions".

## What the data contains

### Visual bank (`VisualBank` → `Resource`)

- Each `Objects` child has `ObjectID` (`<model>.<mesh>.<n>`), `MaterialID` and `LOD` (`uint8`).
  `LOD` is the only LOD attribute in the bank; there is **no** distance, screen size or bias on
  the visual resource. LOD *n* objects name the GR2 mesh `<mesh>_LOD<n>`.
- Typical: `Buildings/[PAK]_Mindflayers` (Nautiloid props) has 70 `LOD 1` and 40 `LOD 2` objects
  next to the LOD 0 ones; `Buildings/[PAK]_Generic` has 1,429 `LOD` attributes. Many visuals have
  only LOD 0.
- `ExcludeFromHLOD` (`bool`) on a few visual resources: keeps the object out of the generated
  HLOD clusters (see below).
- Root templates and level objects carry no per-object LOD distance either (searched
  `Public/Shared/RootTemplates/_merged.lsf` and `TUT_Avernus_C/Scenery`: only fade attributes
  such as `Fadeable`, `FadeGroup`, `HierarchyOnlyFade`, which are camera-occlusion fading, not
  LOD).

### GR2 meshes (`ExtendedData.UserMeshProperties`)

The switch distances live in the GR2, per mesh: `UserMeshProperties.LodDistance` (`float`, one
element) and `IsImpostor` (always 0 in what we looked at). `UserMeshProperties.Lod` is the LOD
index, with **`-1` for the base mesh** (the bank calls it LOD 0). `bg3-gr2` already parses both
into `MeshProperties::{lod, lod_distance}`.

Reading of the values (consistent across every file surveyed): `LodDistance` of LOD *n* is the
**far end of the range where LOD *n* is drawn**, in metres; the last LOD has `FLT_MAX`
(3.4028235e38). Distances are per mesh and authored, not derived from bounds. Examples:

| GR2 | LOD0 | LOD1 | LOD2 | LOD3 | LOD4 |
|---|---|---|---|---|---|
| `BLD_MF_Nautiloid_Mind_Draining_Device_Bed` | 25.5 | 55.5 | ∞ | | |
| `BLD_MF_Hanging_Pod_Base` | 10 | 20 | 80 | ∞ | |
| `BLD_MFL_Nautiloid_Tentacle_Curved_A` | 65 | 95 | 120 | ∞ | |
| `BLD_MF_Nautiloid_Mind_Draining_Device_Bottom` | 40.1 | ∞ | | | |
| `HUM_M_NKD_Body_A` (character body) | 7 | 14 | 25 | 44 | ∞ |
| `HUM_F_NKD_Head_I` (head) | … | 22 | 31 | 31 | ∞ |
| `HUM_M_ARM_Laezel_…_HalfPlate_A_Body` | 34.5 | 34.5 | ∞ | ∞ | |

Notes:
- Character meshes switch much closer than props (body LOD0 only to 7 m, LOD1 to 14 m).
- Several files have the same distance on two LODs or `∞` before the last LOD (Lae'zel's armour:
  LOD2 and LOD3 both `∞`), so LOD3+ is never reached by distance alone there. A selection rule
  "first LOD whose `LodDistance` ≥ distance" handles all of these.
- Meshes inside one visual share the same per-LOD distances in the files surveyed, but not
  always (`BLD_MF_Pod_Receiver_A`: Base and Tentacles LOD0 both 55.8 but LOD1 `∞`). Treat it per
  mesh.
- Cloth proxy meshes have their own LODs (`…_Cloth_Mesh_LOD1`); the bank's `ClothProxyMapping`
  lists them, and they stay undrawn at every LOD.
- Granny's own `LODError` (per bone) and `LODType` (per skeleton) are present too; these are
  Granny animation LOD, not mesh selection (strings `AnimationLODDetail`, `ShouldLOD`,
  `GetLODNode` belong to animation LOD).

### HLOD (hierarchical LOD, level-wide)

Separate from per-mesh LODs, levels have generated cluster meshes:

- `Mods/GustavDev/Levels/<Level>/HLOD.lsf`, region `HLODs`: `Min`/`Max` bounds, then `HLOD` nodes
  with `UUID`, `Parent` (tree), `VisualID`, `LowVisualID`, `Translate`, `Level` (0–2; on
  `TUT_Avernus_C` 50/13/6 clusters), `Position` (grid cell, `ivec3`), `VersionHash`, and an
  `object` list of member level-object GUIDs.
- Level objects reference their cluster with an `HLOD` guid (702 of 866 scenery objects on
  `TUT_Avernus_C`).
- Cluster visuals live in `Public/GustavDev/Content/Generated/HLOD/[PAK]_<Level>/_merged.lsf`
  (`HLOD_<x>_<y>_<z>.GR2` and `…_LOW.GR2`, with virtual textures `HLOD_*_VT`). Their meshes have
  `LodDistance = ∞`.
- Engine side: `ls::HLODSystem`, `ls::HLODComponent`, `ls::HLODChildComponent`,
  `ls::HLODChildRequestOneFrameComponent`, `ls::ClusterDistMinComponent`,
  `ls::ClusterDistMaxComponent`, `ls::ClusterCullDataComponent`, `ls::ClusterVisualLoadedComponent`
  (ECS type names in the Linux binary). The min/max distances are components set at runtime, not
  stored in `HLOD.lsf`; the rule for when a cluster replaces its children is unknown.
  **Assumption:** this matters for open-world levels; on the small Nautiloid interiors the
  per-mesh LODs are the main win. Out of scope for the first step.

## How the engine selects (what we know)

- **Distance-based, per mesh, from the authored `LodDistance`** — inferred from the data: there
  is no screen-size attribute anywhere, the values are round authored metres, and the binary
  keeps the key `LODDistance` as a string. Not yet confirmed which distance (camera position vs.
  camera target, sphere/bounds centre vs. origin) or whether there is hysteresis.
- **Global quality settings**: the binary has the option keys `ModelDetail`, `InstanceDistance`,
  `ShadowQuality` and `AnimationLODDetail`. **Assumption:** `ModelDetail` scales or biases the
  LOD distances (lower quality → coarser LOD sooner) and `InstanceDistance` limits draw distance
  of small instances. Their mapping to numbers is unknown.
- **Shadows**: no evidence either way on Linux. Unknown whether shadow passes use a coarser LOD.

## The viewer today

- `viewer/src/meshes.rs` `VisualMeshes::load` parses each visual's GR2 once and converts only
  `objects` with `lod == 0` (and drawn, not cloth proxy) into `(Mesh, material ID)` pairs per
  visual ID.
- `viewer/src/level.rs` uploads them and spawns per level object one parent entity (transform)
  with a `Mesh3d` + `MeshMaterial3d` child per part.
- Characters (`viewer/src/characters.rs`) use `VisualResource::drawn_lod0_objects()` likewise.

## In the viewer

Level objects switch LODs (`meshes.rs`, `level.rs`) as recommended below, steps 1, 2 and 4:
every drawn LOD is converted, each part gets `VisibilityRange::abrupt(near, far)` from the GR2
`LodDistance`s, measured from the object origin; `BG3_LOD_SCALE` scales the distances (0 = LOD 0
only). Characters (`characters.rs`, `lod_meshes`) do the same per part (step 3): each skinned or
rigid part entity gets the range of its mesh, measured from the part entity's origin (the
character root for skinned parts, the joint for rigid ones). Measurements: `docs/performance.md`.

## Recommended implementation

Smallest change, level objects first:

1. `meshes.rs`: convert every drawn object, not only LOD 0, and keep with each part its
   `lod: u8` and the GR2 mesh's `properties.lod_distance` (`f32`, `FLT_MAX` → `f32::INFINITY`).
   Compute each part's distance range `[near, far)`: `far` = its own `LodDistance`, `near` = the
   largest `LodDistance` of the lower LODs of the same mesh base name (0 for LOD 0). Drop parts
   with `near >= far` (never selected). If a visual has only LOD 0, emit no range.
2. `level.rs`: when spawning the child for a part with a finite range, add
   `VisibilityRange::abrupt(near, far)` (Bevy 0.20, `bevy::camera::visibility::VisibilityRange`).
   Children are positioned at the parent's origin, so Bevy measures camera→object origin; that
   matches "per object distance" (assumption: the game also uses the object position; switch to
   `use_aabb: true` if it turns out to be bounds centre). Keep it abrupt: Bevy's crossfade
   (`start_margin`/`end_margin` widths) needs the visibility-range dither in the fragment shader,
   which the custom level/character shaders would have to include.
3. Characters: the same for skinned parts in `characters.rs` (all parts share the skeleton, so
   sibling entities per LOD work unchanged; skin extraction cost then also drops for hidden
   LODs only if extraction checks `ViewVisibility`).
4. Optional global bias for benchmarking only (e.g. `BG3_LOD_SCALE`, default 1 = the game's
   distances, `0` to force LOD0) until `ModelDetail` is understood.

Shadows: Bevy's `check_visibility_ranges` evaluates ranges against every `Camera` and
`ShadowLodOrigin` entity, and the shadow cascades are checked against the main camera's
position, so shadow passes would draw the **same** LOD as the view. That is a reasonable default
until we know what the game does; a coarser shadow LOD would need a separate set of
`NotShadowCaster`/shadow-only entities.

Costs to expect: extra GPU memory for LOD meshes (each LOD1 roughly half of LOD0, the sum of all
LODs ≈ 2× LOD0), a bit more load time; per frame one distance test per ranged entity
(parallel). Benefit is largest for wide views and shadow cascades covering far geometry.

## Open questions

- Confirm the rule on the macOS binary (symbol index or REA): candidates to look for are
  `ls::Visual` / `ls::VisualSystem` functions reading `LODDistance` (e.g. a per-frame LOD update
  in the visual or render-object culling stage), the use of `ModelDetail` in the graphics options,
  and whether shadow culling stages (`ls::*ShadowStage`) pass the main camera position.
- Distance reference point (object origin vs. bounds centre vs. camera target) and hysteresis.
- When HLOD clusters replace their member objects (`ClusterDistMin/Max`).
