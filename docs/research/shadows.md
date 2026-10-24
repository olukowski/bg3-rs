# Shadows: sun cascades and local light shadow tiles

Status: read statically from the 4.1.1.7631656 arm64 slice of the macOS binary (symbol index,
`objdump` on single functions, a small adrp/ldr cross-reference scan of the disassembly, and
reads of constant tables). REA's Ghidra provider did not finish importing the 244 MB slice in its
startup window, so no pseudocode was used. Everything here is our own description; nothing was
observed at runtime. **Observed** means read directly from instructions or constant data;
**inferred** means our interpretation of observed code.

The purpose is to guide the viewer's shadow performance work (`docs/performance.md`), and to
record where we would need to change things to match the game.

## Stages involved

| Symbol (4.1.1.7631656) | Role |
|---|---|
| `ls::CascadedShadowBufferStage` | Sun cascades: split distances, cascade cameras, schedules the depth jobs |
| `ls::ShadowStageJob` | One depth-render job (a cascade, or part of one) |
| `ls::ShadowCullStage` | Builds the caster set for one cascade camera (`ls::Scene::Cull`) |
| `ls::VistaShadowCullStage`, `ls::VistaShadows::SectorRequest` | Distant "vista" shadow sectors (see Q3) |
| `ls::CascadedShadowMaskStage` | Screen-space sun shadow mask; packs the light-size/fade parameters |
| `ls::SetupForwardLightingConstants` | Uploads cascade matrices and parameters, local shadow fade |
| `ls::LocalLightShadowStage`, `ls::LocalLightShadowCullStage` | Local light (point/spot) shadow rendering into the atlas |
| `ls::ShadowTileAssignmentStage`, `ls::ShadowTileMap` | Per-frame assignment of atlas tiles to shadowed lights |

The Lighting resource's sun fields are serialised by `ls::Lighting::Visit`. That function and the
`CascadedShadowBufferStage` reads give the layout of the sun block: `ShadowEnabled` (bool), then
`ShadowFarPlane`, `ShadowNearPlane`, `ShadowFade`, `ShadowBias`, `LightSize`, `LightDistance`,
`CascadeSpread` (floats), and `CascadeCount` (byte), in that order (observed).

## Q1: cascade split distances, fitting and fading

### Cascade count

`CascadedShadowBufferStage::OnStateChanged` takes `CascadeCount` from the active sun. It only
accepts values from 1 to 4; any other value keeps the previous count (observed). There are at most
4 cascades.

### Split distances (observed)

`CascadedShadowBufferStage::StartRecording` recomputes the splits every frame:

- `n = max(ShadowNearPlane, 0.01)` and `f = ShadowFarPlane`. `n` is stored as split 0.
- For `i = 1 … CascadeCount`, with `t = i / CascadeCount`:
  - `uniform_i = n + (f − n)·t`
  - `log_i = n·(f/n)^t`
  - `split_i = uniform_i + CascadeSpread·(log_i − uniform_i)`

This is the standard "practical split scheme": `CascadeSpread = 0` gives uniform splits,
`CascadeSpread = 1` gives logarithmic ones. The last split is always `ShadowFarPlane`. Cascade
`i` covers view depth `[split_{i−1}, split_i]`. The loop is vectorised four at a time, but the
maths is the same.

### Fitting (observed)

Each cascade is fitted with a **bounding sphere of its view-frustum slice, stabilised by texel
snapping**:

1. From the camera's vertical FOV (`tan(fov/2)`) and aspect ratio, take the minimal sphere that
   contains the slice between `split_{i−1}` and `split_i`. Its centre lies on the view axis at a
   depth fixed by the slice's near and far corner distances, and is clamped to be no farther than
   the slice's far plane. The radius is the distance from the centre to a far corner. The radius
   depends only on the splits and the FOV, so it does not change when the camera rotates.
2. The cascade camera's orientation comes from the sun direction (a quaternion built from it,
   with special cases when the sun is near vertical). The camera is placed at the sphere centre,
   moved back along the light direction by `LightDistance`.
3. Snapping: texel size = `2·radius / resolution`. The centre is rotated into light space, each
   coordinate is rounded down to a multiple of the texel size (`fmodf` subtraction), and the
   result is rotated back. The shadow map therefore moves in whole texels, so it doesn't shimmer
   as the camera moves.
4. The projection is orthographic: `[−r, r]` on both axes, with near 0 and far
   `LightDistance + r`. So `LightDistance` is how far towards the sun casters are captured.
5. A cascade camera is only marked dirty when its projection (near/far, extents) changes by more
   than about 1e‑7.

### Fading and blending

- `CascadedShadowMaskStage` stores `LightSize·0.5·π/180` (the sun's angular radius in radians),
  `LightDistance`, `ShadowFade` and `ShadowBias` (observed). `SetupForwardLightingConstants`
  uploads `1 / ShadowFade` together with the cascade count, four per-cascade float vectors and the
  four cascade matrices. With filter type 1 it also uploads `tan(angular radius)` and
  `tan(angular radius)·LightDistance` (observed).
- Inferred: `ShadowFade` is a fade *distance* (the shader multiplies by its reciprocal), most
  likely the fade-out before `ShadowFarPlane`. The angular light size drives a
  penumbra/blocker-search filter (PCSS-like) when the filter type is 1.
- We did **not** find a CPU-side blend band between cascades. Any cross-fade between cascades
  happens in the shadow-mask shader, which we haven't read (open question).

## Q2: resolution, quality settings and atlas

### Graphics option

`ecl::VideoOptionsManager::InitOptions` adds the shadow quality combo (`rf::EShadowQuality`) with
the values 2, 1, 0 (observed). The quality is read from the global graphics settings byte at
offset 0x43 (`BaseApp::LoadGraphicSettingsConfig` references the `ShadowQuality` string). Inferred
from the table sizes: **0 = High, 1 = Medium, 2 = Low**.

### Sun cascades (observed)

Both tables are indexed by quality:

| Quality | `ls::EShadowQualityShadowMapSizes` | Cascade map size | `ls::EShadowQualityFilterTypes` |
|---|---|---|---|
| 0 High | 1 | **2048** | 1 |
| 1 Medium | 0 | **1024** | 1 |
| 2 Low | 0 | **1024** | 0 |

`OnStateChanged` turns the size code into 1024 or 2048. It writes the same value to every
cascade camera, so **all cascades share one resolution**. `CreateRenderTargets` creates one depth
render target of `size × size` whose descriptor has a layer count of 4 (inferred: a
**4-slice texture array, one slice per cascade**), plus a second `size/4 × size/4` target whose
purpose we did not identify. At High that is 4 × 2048² = 16 Mi depth texels.

### Local light shadow atlas (observed)

`ls::s_DefaultsShadowTileMapSettings` is copied into the render settings (`ecl::PlayerManager::CreateRenderView`,
`BaseApp::InitAPI`). It has one 32-byte entry per quality:

| Quality | Atlas | Max tile | Min tile | Base size | Fade start | Fade end / cut-off | (unused?) | Min size |
|---|---|---|---|---|---|---|---|---|
| 0 High | 8192² | 2048 | 128 | 1024 | 40 m | 55 m | 256 | 8 |
| 1 Medium | 4096² | 1024 | 128 | 512 | 20 m | 35 m | 128 | 16 |
| 2 Low | 2048² | 512 | 128 | 256 | 20 m | 25 m | 64 | 24 |

The column names are our interpretation. Here is how each column is used:

- `LocalLightShadowStage::OnStateChanged` sizes the tile-map render target to *Atlas*², and a
  shared depth target to *Max tile*².
- `ShadowTileAssignmentStage::OnStateChanged` calls `ShadowTileMap::Init(atlas, max, min)`. That
  requires a power-of-two atlas, `min < max < atlas` and `max ≥ 16`. It builds a quadtree with
  `log2(max) − log2(min) + 1` tile levels and room for `(atlas/min)²` smallest tiles.
- *Base size* (1024/512/256) scales each light's requested tile size (see Q3).
- `SetupForwardLightingConstants` turns *Fade start*/*Fade end* into a linear shadow fade,
  `saturate((end − d)/(end − start))` (observed as a scale and bias pair; inferred that `d` is the
  camera distance). `LocalLightShadowCullStage` and the assignment's per-light test use *Fade end*
  as the cut-off: a light whose bounding sphere is farther than *Fade end* plus its radius from
  the camera gets no shadow tile.
- *Min size* (8/16/24) is compared against the light's requested size. Lights below it are dropped,
  and lights just above it get a smoothstep weight (see Q3).
- The 256/128/64 field was not seen in use.

## Q3: caching across frames, and local light tile sizes

### Sun cascades

- **Every cascade is re-fitted and re-culled every frame**: `StartRecording` loops over all
  cascades and schedules their `ShadowStageJob`s (observed). We found no static-caster cache,
  no "update far cascades every N frames" schedule and no frame-skip counters in
  `StartRecording`, `ShadowStageJob::ExecuteWTKernel` or `ShadowCullStage::Cull`. This search was
  bounded to those functions.
- The job split is for CPU parallelism, not caching. There are 8 `ShadowStageJob`s in total,
  and `ls::CascadedShadowBufferStage::s_ShadowCascadeJobCounts = {1, 1, 1, 5}` gives the most jobs
  per cascade. For cascade 3 (the farthest of four),
  `ShadowStageJob::ExecuteWTKernel` uses `clamp(round(N3 / max(250, N0+N1+N2)), 1, 5)` jobs, where
  `Nk` is the caster count of cascade k (observed). `ShadowStageJob::GetHeuristic` ranks jobs by
  cascade index for the scheduler.
- **Vista shadows** (`ls::VistaShadows`, `VistaShadowCullStage`): when enabled, `StartRecording`
  either produces new `SectorRequest`s (over a 16-wide sector loop, using the sun direction) or,
  while requests are still pending, schedules the vista job. The vista cull stage renders one
  sector request at a time through `ls::Scene::Cull` (observed). Inferred: this is a
  **progressively rendered, cached far-distance shadow** that is filled in over several frames and
  refreshed through new requests. When the requests are regenerated (sun direction change,
  camera sector change) is not established.

### Local lights

`ShadowTileAssignmentStage::ExecuteWTKernel`, every frame (observed):

1. Gathers shadow-casting lights. Per light, a lambda takes the light's bounding sphere
   (`LightComponent::GetBoundingSphere`) and drops lights beyond the cut-off. It then computes
   the sphere's angular size as seen from the camera and divides it by a camera angle factor
   (inferred: the FOV), times *Base size*. The result is the light's **requested tile size**.
2. Sorts the lights by a priority (`QuickSort` with lambda `$_1`).
3. For each light: round the requested size up to a power of two and clamp it to
   `[Min tile, Max tile]` (128 to 2048 at High). Find a free node of that size in the
   `ShadowTileMap` quadtree (`FindNode`). Add the tile's share of the atlas
   (`100 / 4^level` percent) to a usage counter, or to an overflow counter if no node was free.
   Lights whose requested size is below *Min size* are dropped. Just above that, the priority is
   reduced by a smoothstep, and lights with priority under 0.2 are skipped.
4. When the atlas usage (used + overflow) goes above 75 %, a scale factor
   (`usage·0.01 − 0.75` enters it) shrinks later requests. Inferred: this is an adaptive budget so
   that crowded scenes get smaller tiles rather than missing ones.
5. Stores a per-light float in a `HashMap<LightComponent*, float>` that carries over to the next
   frame (inferred: hysteresis on the priority or size so tiles don't flicker between levels).

So the tile **size per light is dynamic**: proportional to the light's on-screen angular size,
scaled by quality (base 1024/512/256), and limited to 128–2048 / 128–1024 / 128–512. Tiles are
re-assigned every frame. We found no evidence that a light's tile contents are kept across frames
when nothing moved; the tile assignment itself is recomputed each frame (bounded search; see open
questions).

## Q4: what limits the caster set per cascade

- **Culling volume** (observed): each cascade's cull planes are the six planes of its
  orthographic box, plus those planes of the main camera frustum whose normals face away from the
  sun (a dot-product sign test against the light direction). The total is capped at 10 planes.
  Inferred: this is the usual "light box ∩ (camera frustum extruded towards the light)" volume. It
  removes casters that are inside the cascade box but cannot shadow anything visible.
- **No distance or screen-size cut-off was found** for sun casters. `ShadowCullStage::Cull` fills its
  `ls::CullDesc` with two `FLT_MAX` limits (inferred: maximum distance and LOD/size limits left
  unbounded), the plane list, and a flag word.
- **Per-cascade flag bits** (observed): the flag word passed to the scene cull has one bit per
  cascade index (0x2, 0x4, 0x8, 0x10), plus 0x200 or 0x400 depending on a render-view mode.
  Inferred: objects carry per-cascade render masks, so content can opt out of particular cascades.
- **Per-object flags** that exist in the binary: `CastShadow` (`ls::FFFS::strCastShadow` and
  several anonymous `strCastShadow`), `IsShadowProxy`, `ShadowEnabled`, `ShadowPhysicsProxy`,
  `CastVolumetricShadow`, and `ls::FadeShadowEnabled` (called from `ShadowCullStage::Cull` with the
  fading-entities singleton; inferred: objects that are fading out drop out of shadows according
  to a rule). Shadow proxies (simplified meshes that cast instead of the full mesh) are a content
  feature worth honouring.
- Local lights: lights beyond the *Fade end* distance plus their radius get no shadow at all
  (55/35/25 m by quality). `LocalLightShadowCullStage::ProcessSSECull` culls casters per light.

## Implications for the viewer (design notes, not behaviour)

- The game renders up to four 2048² cascades (High) each frame, with no caster caching. The
  viewer's Bevy cascades are in the same cost class. Matching the split formula exactly gives the
  same per-cascade coverage. Bevy's `CascadeShadowConfigBuilder` uses its own split scheme, so we
  would compute the splits ourselves from `CascadeSpread`.
- Sphere fitting with texel snapping is cheaper to stabilise than frustum-fitted cascades, but the
  maps cover more area per texel.
- Local light shadows in the game use one atlas with tiles sized by screen size, and are cut off
  beyond 55 m (High) with a 40–55 m fade. Box lights are not local lights in this sense (see
  `docs/research/lighting.md`).

## Open questions

1. The shadow-mask shader: cascade selection, any blending between cascades, the exact use of
   `ShadowFade`, filter type 0 vs 1.
2. What the `size/4` second cascade render target and `CreateReadbackTexture` are for.
3. Vista shadows: when sector requests are regenerated, their resolution (a 4096 constant appears
   in `VistaShadowCullStage::ExecuteWTKernel`), and how they are combined with the cascades.
4. Whether `LocalLightShadowStage` skips re-rendering a tile whose light and casters did not
   change. Its `ExecuteWTKernel` was not read in detail.
5. The meaning of the unused tile-map field (256/128/64), and the exact camera angle factor in the
   requested-size formula.
6. Confirm the quality enum names (0 = High) from the combo labels or the user's
   `graphicSettings.lsx`.
