# Lighting

Status: placed lights, level-wide lighting, lighting/atmosphere triggers, light probes and fog
volumes are parsed (`crates/bg3-world/src/{lights,atmosphere,probes,fog}.rs`). The viewer renders
them with the game's units, falloff, exposure, colour grading and display transform
(`viewer/src/{lights,probes,fog,post}.rs`, `shaders/{game_light,post}.wesl`). Everything below
was established statically, from the compiled shaders (`Core.metallib`) and the binary (4.1.1.7631656),
not traced at run time; what is exact and what is approximated is listed per section.
Points not established are marked **assumption**.

## Placed lights

Lights are level objects in `Mods/<Module>/Levels/<Level>/Lights/_merged.lsf` (`Type=light`) and
`CombinedLights/_merged.lsf` (`Type=CombinedLight`). They merge across modules by `MapKey`.

- `TUT_Avernus_C`: 356 lights (249 point, 52 spot, 55 directional). Of these, 268 are in Gustav
  and 81 in GustavDev; 5 are combined lights.
- `WLD_Main_A`: 6 own lights. Sub-levels add more (for example `WLD_Crashsite_D` has 109).

### Sparse fields and template inheritance

A light object stores only the fields that **differ from its root template**. That template in
turn stores only what differs from its `ParentTemplateId`. A typical chain:

```
level object   Intensity=3000, Kelvin=15000, Radius=8, Transform
template       Light_Nautiloid_Fill_10000K: Intensity=1000, Kelvin=10000, Radius=7
parent         BASE_Light_Nautiloid_Fill: FlatFalloff=true, Gain=1
root           BASE_Light_Nautiloid: every field (LightType=0, Color=1,1,1, Angle=35,45, …)
```

Resolution takes the first value found along object → template → parent → …. This is the first
confirmed case of "empty field means inherit" (see `level-structure.md`).
`RootTemplate::light` holds the sparse `LightProps` of `light`/`CombinedLight` templates.

### Fields

| Field | Type | Notes |
|-------|------|-------|
| `LightType` | u8 | 0 = point, 1 = spot, 2 = box ("directional"). Confirmed by the bounds the engine builds per type (`ls::LightComponent::UpdateBounds`) |
| `Color` | fvec3 | linear colour, used as stored |
| `Intensity` | float | 150–600000 on the Nautiloid, typically 2000–10000. Luminous power (lumen-like), see "Light units" |
| `Kelvin`, `UseTemperature` | float, bool | colour temperature (default 6445.9 K), multiplied in when `UseTemperature` is set |
| `Radius` | float | range: the light is cut off at `Radius` |
| `Angle` | fvec2 | spot cone, **full** angles in degrees (inner, outer). Combined lights have `GameplaySpotlightAngle = (x+y)/2` |
| `FlatFalloff`, `Gain` | bool, float | the engine's "fill light" flag (constant falloff) and an additive term, see "Light units" |
| `DirectionLightDimensions` | fvec3 | box size (w, h, depth) of type-2 lights |
| `DirectionLightAttenuationStart/End/Side`, `…Function` | float, u8 | box-light fade lengths (m) and curve (0 linear, 1 square, 2 smoothstep, 3 smootherstep) |
| `Shadow`, `CastShadow`, `VolumetricShadow` | bool | |
| `PreExpose` | bool | the light is multiplied by the camera exposure like everything else; without it the light is in display units. Every Nautiloid light has it |
| `IsFlickering`, `Amount`, `Speed`, `IsMoving`, `MovementSpeed/Amount` | | animation |
| `ScatteringScale` | float | volumetric fog contribution |
| `LightChannelFlag` | u8 | light channel mask: a light reaches a pixel only if the bit of the pixel's G-buffer channel (bits 4–6 of target 2's alpha) is set. Mostly 191 (all but channel 6) |
| `LightCookieTexture`, `LightCookieResource` | FixedString | projected texture |
| `Enabled` | bool | |

Combined lights add `GameplayRadius`, `GameplaySpotlightAngle`, `GameplayDirectionalDimensions`,
`GameplayIsActive`. These belong to the gameplay light (light/darkness for stealth and
`eoc::GameplayLightComponent`). Their `Intensity` can be 0, which makes them gameplay-only.

**Type 2 is not a sun.** It is a local box-shaped light (a few to ~20 m) that shines along one
axis.

## Level-wide lighting

`Mods/<Module>/Levels/<Level>/meta.lsf` → region `LevelData`:

- `LevelLighting`: a `LightingBank` resource ID.
- `LevelAtmosphere`: an `AtmosphereBank` resource ID.
- `LevelBounds { Min, Max }`.

Both banks are in `Public/Shared/Content/Assets/Atmospheres/[PAK]_Atmospheres/_merged.lsf`. The
Nautiloid and `WLD_Main_A` both use `LTN_Default` and `ATM_Default`. The sub-level
`WLD_Crashsite_D` points to a lighting ID that doesn't resolve. Presumably the main level's
lighting applies to it.

`LightingBank/Resource/Lighting` contains:

- `Sun`: `Yaw`, `Pitch` (degrees), `SunColor`, `SunIntensity` (100000, so lux scale), `Kelvin`,
  `UseTemperature`, shadow cascade settings, `LightSize`, `CoverageSettings`.
- `Moon` (`Enabled`, `Intensity`, `Yaw/Pitch`, textures).
- `SkyLight`: `Color`, `Intensity` (10000), `Kelvin`, plus scattering, clouds and stars.
- `Fog`/`TimelineFog`: `RenderDistance`, `Phase`, `FogLayer0/1 { Enabled, Albedo,
  Density0/1, Height0/1, Noise* }`.
- `SSAOSettings`, `VolumetricCloudsSettings`.

`AtmosphereBank/Resource/Atmosphere` contains wind and cloth settings, `Camera { NearPlane,
FarPlane }`, and `PostProcessing` (bloom, god rays, DOF, LUT, white balance,
`ExposureSettings/Object { Exposure, ExposureMin, ExposureMax, ExposureCompensation, … }`). There
are two `ExposureSettings` entries: (14, 16, 0) and (0, 16, −1). Which one applies when is not
known. Levels also switch lighting and atmosphere where the camera is, with triggers (next
section).

### Lighting and atmosphere triggers

The level's `meta.lsf` resources are only the default. Polygon triggers in the level's
`Triggers/_merged.lsf` replace them where the camera is: root template `TriggerType`
`TriggerAtmosphere` (template `AtmosphereTrigger`) and `TriggerLighting` (`LightingTrigger`), with
`ResourceIDs/ResourceID` (one or more resource IDs), `FadeTime` (s), `Height` and `Points` (the
prism shape of `PhysicsType` 3, which both templates set, with `Height` 2).
`crates/bg3-world/src/atmosphere.rs` reads them (level and `Globals` triggers, as the sim does);
`LightingSources::zones_at` picks the trigger in effect (`bg3-world` tells lighting from
atmosphere triggers by which bank the first resource is in).

#### Trigger volumes and selection

Read in the binary (4.1.1.7631656); implemented once in `crates/bg3-world/src/trigger.rs` and
used by both this selection and the sim's `EnteredTrigger`/`LeftTrigger` (the server's
`esv::trigger::TrackingSystem` calls the same hit test):

- **Hit test** (`ls::trigger::HitTest`). The point is taken relative to the trigger's
  position and turned by the conjugate of its rotation (`ls::Trigger::InitTriggerPhysics_Legacy`
  stores it); `Scale` is not applied. Sphere: distance from the position ≤ `Radius` (rotation
  ignored). Box: every local coordinate within ±`Extents` (half sizes, faces inside). Polygon:
  fewer than three points never contain anything; otherwise the local point must be inside the
  local bounds — whose y range is spanned by the vertices `(x, 0, z)` and `(x, Height, z)` built
  by `CalculateVertices`, i.e. **local y from 0 to `Height`: up from the position** (down for a
  negative height) — and inside the `Points` polygon by the even-odd rule
  (`IsPointInPoly`; an edge counts when one end is below the point's z and the other at or
  above it). A bounding-sphere pre-check is a superset and changes nothing.
- **Which point** (`ecl::CameraSystem::CheckTriggers`, `ecl::TimelineCameraSystem::UpdateCameras`):
  the camera's eye position (the camera controller's world translation, set from the camera
  entity's transform in `ls::CameraSystem::OnTranslationChanged`), re-checked when the camera
  moves — not the player character's position.
- **Overlaps** (`ecl::GetLightingTrigger`, `ecl::CameraSystem::SwitchAtmosphereTrigger` with the
  sort in `ecl::CreateAtmosphereInstanceAndGetTrigger`): of the triggers of the kind found at the
  point (`eoc::trigger::GetTriggersOfTypeAtPosition`) that pass the hit test, the one with the
  smallest local bounding radius in the horizontal plane (`CalculatePhysicsBounds`, the first
  bounds field): sphere `Radius`, box `√(Extents.x² + Extents.z²)`, polygon the largest distance
  of a point from the mean of the points (height ignored). No priority field. Lighting keeps
  the first candidate on a tie; atmosphere sorts with an unstable quicksort (**assumption**:
  ties keep the earlier trigger in file order; the engine's candidate order comes from its
  trigger tree).

The Nautiloid (`TUT_Avernus_C`; level default `LTN_Default`/`ATM_Default`, EV 14–16, sun
100000) has four:

| Trigger | Resources (first = default) |
|---------|-----------------------------|
| `LTN_TUT_Avernus_C_Exterior` (whole ship) | `LTN_Default_EV10_Avernus_Ext`, `LTN_TUT_Avernus_A_Exterior_B`, `LTN_CRE_AstralPlane_A` (in no bank we load) |
| `ATM_TUT_Avernus_C_Exterior` (whole ship) | `ATM_Default_EV10_Avernus_Ext`, `ATM_CRE_AstralPlane_A_CINE` |
| `ATM_TUT_Avernus_C_Interior` | `ATM_Default_EV08_Avernus_Int`, `ATM_CRE_AstralPlane_A_CINE` |
| `ATM_TUT_Avernus_C_Interior_UnderDeck` | `ATM_Default_EV08_Avernus_Int` |

So in play the Nautiloid is lit by `LTN_Default_EV10_Avernus_Ext` (sun 500 at 3500 K, sky light
5000, two fog layers) and exposed at EV 8.5–12 (exterior) or 8.5–10.5 (interior), about five stops
darker than the level default. Light probes are baked per lighting resource and exist for exactly
the three resources of the lighting trigger, which confirms it is the one in use. The astral-plane
resources are also keyed in the `TUT_Helm_Transponder` timeline (the helm vision).
**Assumption:** the first resource is the default one. On the imp deck (y ≈ 2.8) the camera
decides: a camera below y 14.1 (close-ups, the viewer's `--frame` shots) is only in
`…_Exterior`; a camera more than ~11 m above the deck is also inside `…_Interior` (bound
radius 143 m against the exterior's 317 m), which wins. `…_UnderDeck` (y 3.2–15.8) lies west of
the deck (x −127…−69) and does not cover it. The two atmospheres differ only in `ExposureMax` and
the second entry's compensation. Story calls `TriggerSetAtmosphere`/`TriggerSetLighting`
(not used on the Nautiloid) are not implemented. The
viewer starts with the triggers at its start camera position (`--camera`, the play camera, or
the avatar / `--look-at` until `--frame` places the camera) and then follows the camera for the
atmosphere (`lights::follow_camera_triggers`, blending as described below); switching the lighting
resource at runtime (sun, probes, fog) is not implemented.

Atmosphere transitions blend the gameplay camera's first exposure entry (EV minimum,
maximum, compensation and metering percentiles), plus all stored grading parameters,
white balance and bloom scalars. **Assumptions:** the NEW selected trigger supplies
`FadeTime`, interpolation is linear in the stored parameter space, and discrete values
(auto-exposure enable, metering mode and bloom enable) change at the midpoint. In particular,
range `ExposureShift` blends before conversion to `1.35^shift` for the shader. The fixed
display LUT has no per-atmosphere handle to switch; future per-atmosphere textures/LUTs
should use the same midpoint rule. Rendering limitations for bloom/white balance remain.
Retargeting captures the current blended values, so a second trigger change does not jump
back to an earlier endpoint. Zero duration switches instantly. Leaving all triggers uses
zero duration (**assumption:** the level default has no entering trigger/FadeTime).
The first camera update selects instantly, preserving screenshot startup and the GPU's
existing first-frame exposure jump. Synthetic tests in `lights::atmosphere_tests` cover
endpoints, midpoint/discrete settings, retargeting and zero duration.

Evidence from the references (camera positions unknown, compared by eye):
`laezel-combat.png` looks down at the deck in blue shade, with warm gold light only far off to
one side; `laezel-sheathed.png` looks out of the opening into strong golden haze. Viewer shots
looking at the party (`--play --combat-test --no-follow --look-at=-46.6,3.5,-402.8`) from a
high camera (`--camera=-42,14.5,-409`, interior atmosphere) look most like `laezel-combat`:
cool blue deck and warm light only in the distance. From a low camera (`-43.5,8,-407.5`,
exterior atmosphere) the deck looks warmer. Because the atmospheres differ only in exposure range, the tint
comes from what the camera sees, not from the trigger. Centre-region luma against
`laezel-combat`: high 22.7, low 31.2, reference 37.8. The framings differ too much (UI,
geometry in front of the high camera) to call either one right.

### Light probes

`LightProbes/_merged.lsf`, `Type=lightProbe`, sparse over the root template `GUS_LightProbe`
(`Radius` 5, `Size` 10³, `TransitionDistance` 1, `LightProbeShape` 0, `LightProbeType` 1,
`Intensity` 1, `InfiniteCapture`). `CaptureDescriptions/CaptureDesc` per `Lighting` resource:
`DiffuseIBL`, `DiffuseAlphaIBL`, `SpecularIBL`, `SpecularAlphaIBL` (`.dds` in
`Gustav_Textures.pak`) and `SphericalHarmonicsIBL` (64-byte `_SH.dat`). The Nautiloid has 32
probes (28 Gustav, 4 GustavDev) at y ≈ 17–30 in the rooms and a few at y ≈ 6 (`LightProbe_Room3*`).
`crates/bg3-world/src/probes.rs` reads them.

## Light units and falloff

Sources: the light loop of `ClusteredLighting_*` (Core.metallib) and the CPU code that packs the
light buffers (`ls::ClusteredAssignmentStage::SetLightData` / `SetDirectionLightData`,
`ls::LightTemplate::Visit`, `ls::LightComponent::LightComponent`, `ls::blackbody`,
`ls::Lighting::Visit`, `ls::GameRenderView::UpdateConstantBuffersData`).

- **Colour** of every local light: `Color · blackbody(Kelvin) (if UseTemperature) · (Intensity +
  a second component field) · a per-light factor from the light list` (**assumption**: the
  flicker offset and a fade, 0 and 1 for a static light; the viewer uses `Intensity` alone).
  No π or lumen/candela factor on the CPU (checked for points, spots and boxes in `SetLightData`
  / `SetDirectionLightData` and the inlined point light packing of
  `ls::ClusteredAssignmentStage::ExecuteWTKernel`); the shader applies 1/(4π) to point lights,
  1/π to spots and nothing to boxes. `blackbody` is a chromaticity on the Planckian locus
  (rational fits in CIE 1960 uv) blended into the CIE daylight locus around 13000 K, converted to
  linear Rec.709 and normalised to unit luminance (`bg3_world::lights::kelvin_to_rgb`, exact).
- **Exposure:** the lighting multiplies `PreExpose` lights, the sun and the ambient by the
  exposure `1/(1.2 · 2^EV100)` (the ambient also by 100, see "Light probes"); lights without
  `PreExpose` get 1.
- **Point:** `C · (1/max(d², 1e-4) + Gain) · sat(1 − (d/R)⁴)² · N·L / (4π)`, zero beyond `R`. With
  `FlatFalloff` the inverse square becomes the constant `4/R²`. `Gain` is added, not multiplied.
  Nautiloid lights are mostly `FlatFalloff` with `Gain` 1.
- **Spot:** the same times `sat((cos θ − cos(outer/2)) / (cos(inner/2) − cos(outer/2)))²`, with
  **1/π** instead of 1/(4π). The axis is the local **+Z**.
- **Box (type 2):** the box spans `±w/2`, `±h/2` and `z ∈ [0, depth]` from the object origin (the
  centre of the source face) along local +Z; position and rotation only, the object's scale is
  not applied. Inside it the light has a constant direction (towards the light: local −Z),
  illuminance `C · w · N·L` (no π, no distance falloff), with the fades `w` below. Light
  cookies can modulate it. Packing (`ls::ClusteredAssignmentStage::SetDirectionLightData`): a
  world → box map to coordinates `u = (x/w + ½, y/h + ½, z/depth)` (inside: every coordinate in
  0–1), `zs = clamp(AttenuationStart/depth, 1e-5, 1)`, `ze = clamp(1 − AttenuationEnd/depth, 0,
  0.99999)`, `1/zs`, `1/(1 − ze)`, side scales `(w, h)/max(AttenuationSide, 1e-5)`, the curve
  `AttenuationFunction` in the top byte of the channel word. `Illuminance_Direction`
  (`ClusteredLighting_*`): `w = f(u.z/zs)` below `zs`, `f((1 − u.z)/(1 − ze))` above `ze`, else 1;
  times `f(s)` per side for `s = (½ − |u − ½|) · scale < 1`; `f` is `t`, `t²`, smoothstep(sat t)
  or `t³(6t² − 15t + 10)`, for curves 0–3. Box lights with `Shadow` are shadowed by an
  **orthographic** map: the shadow lookup uses the first three rows of the shadow matrix with no
  perspective divide (point and spot lights divide by the fourth).
- **Sun:** direction towards the sun `(cos p · cos y, sin p, cos p · sin y)` in BG3 space (yaw 0 =
  +X, 90° = +Z, pitch = elevation). If `SkyLight.ScatteringEnabled` (both Nautiloid lighting
  resources and `LTN_Default`), surfaces are lit with `ScatteringSunColor · ScatteringSunIntensity ·
  transmittance` (Bruneton atmosphere LUT); otherwise with `pow(SunColor, 2.2) · SunIntensity ·
  blackbody(Kelvin)`. Lux; the BRDF carries the 1/π. The sun is always exposed and shadowed
  (cascades, `ShadowFarPlane`).

### In the viewer

`viewer/src/lights.rs`. Bevy's exposure is the same `1/(1.2 · 2^EV100)` and Bevy divides a point
or spot light's lumens by 4π, so: point lumens = `Intensity` (exact); spot lumens = `4 ·
Intensity` and half angles (exact cone); colours from the engine's `blackbody` (exact). The
falloff is one function, `bg3_light_falloff` in `shaders/game_light.wesl`, used by the game's
light loop (`game_lighting.wesl`, characters and level surfaces) and patched at run time into Bevy's point light (`bevy_pbr::render::pbr_lighting`, in
place of `getDistanceAttenuation`). It reads `Gain`/`FlatFalloff` packed as the game does
(`FlatFalloff ? −max(Gain, ε) : Gain`) from the light's `shadow_depth_bias` (unused: the viewer
renders no point or spot shadow maps). Notes and approximations:

- **Box lights** are exact, in the game's light loop: `lights::box_light` packs them like the
  engine into a storage buffer (`lights::BOX_LIGHTS`, bound at 120 by every game-shaded level and
  character material) and `game_lighting.wesl` evaluates `Illuminance_Direction` for every box
  containing the pixel. It finds the candidates in a 4 m world grid (`lights::BOX_LIGHT_GRID` at
  121, a performance measure only: the game's clustered assignment does this per froxel; see
  `docs/performance.md`). Their shadows: a Bevy directional light gives the same kind of
  orthographic map, so black directional lights carry them (`lights::spawn_box_shadow_carriers`;
  the shader finds the carrier by direction, boxes along one axis share it). Bevy allows ten
  directional lights, one being the sun, so the nine carriers go each frame to the directions
  of the shadowed boxes in the camera's view nearest the camera (`assign_box_shadow_carriers`);
  farther ones go unshadowed. As in the game, each carrier renders one map fitted to its boxes
  (`fit_box_shadow_cascades`; the Nautiloid has 11 shadowed boxes along 10 directions). Unused
  carriers render an empty map rather than being switched off, so the shadow map array keeps its
  size. Approximations: the map size is Bevy's directional one (2048; the game's box light shadow
  resolution is not known), biases and filtering are Bevy's; box lights don't scatter in Bevy's
  fog. The first port gave each carrier Bevy's four cascades over the camera. That drew the
  whole view 36 times a frame (5 fps on the M5 Pro, a slideshow on Linux; see
  `docs/performance.md`). `BG3_BOX_SHADOW_CASCADES=1` brings that look back, with casters culled to
  the boxes, so it costs 10–25 % instead. The one-map shadows differ from it by −0.7 to +0.2 luma
  on static deck views (softer edges close to the camera). An earlier finding, that two cascades
  instead of four darkened the framed imp shot by 3.5 luma, came from `--play` shots, which change
  with the frame rate (the fight runs while the shot waits). Surfaces on Bevy's
  `StandardMaterial` (`BG3_STANDARD_LEVEL=1`) get no box
  lights. `BG3_NO_BOX_LIGHTS=1`, `BG3_BOX_ONLY=<name substring>` and `BG3_NO_BOX_SHADOWS=1` are
  A/B switches.
- The sun's atmospheric transmittance is taken as 1 (the `Atmos_Transmittance` precompute and its
  units are not ported). The sun casts Bevy cascaded shadows up to `ShadowFarPlane`; point and
  spot lights cast none. Bevy's volumetric fog uses Bevy's falloff, not the patched one.
- Not handled: light channels (surface channels aren't known), lights without `PreExpose`,
  `IntensityOffset`/flicker, cookies, the falloff-exponent modifier (0 at load).
- `--light-scale` (default 1) multiplies local lights for A/B comparisons.

## Exposure

Sources: `PPCalculateEV`, `PPHistogram`, `PPExposure`, `PPRexpose_PS` (Core.metallib) and
`ls::PostProcessStep_Exposure` (`OnSizeChanged`, `ExecuteWTKernel`), `ls::ExposureSettings::Visit`.

The lighting writes **pre-exposed** HDR: it multiplies lights by last frame's exposure
`E = 1/(1.2 · 2^EV100)`. The exposure buffer starts at EV100 6.38 with a histogram over EV100
−8 to 18. Each frame:

1. **EV texture**, 640×384 (1280×768 when the screen is wider than 2560 / taller than 1440):
   four bilinear taps a quarter texel apart, Rec.709 luminance, un-pre-exposed, as EV100:
   `log2(max(L · 1.2 · 2^EV, 1e-5)) + 3`.
2. **Histogram** of 256 bins over [−8, 18], each texel split linearly between its two nearest bins
   in 1/256 fixed point, weighted by the `ExposureMeteringMode`: 0 uniform, 1 centre-weighted
   (`1 − sat(length((p − size/2) · 2/size))`); texels at or below −8 are skipped.
3. **Average** over the mass between `ExposureMinPercentile` and `ExposureMaxPercentile` (5–95 %),
   target = mean − `ExposureCompensation` (so positive compensation brightens).
4. **Adaptation** `EV += (target − EV)(1 − e^{−k·dt})` with k = 2 towards a brighter scene, 1
   towards a darker one, then **clamped** to [`ExposureMin`, `ExposureMax`].

The gameplay camera uses the first `ExposureSettings` entry (**assumption**: the second is for
dialog/cinematic cameras; the index comes from the camera component). `DeltaTime` is taken as the
frame time (**assumption**).

In the viewer (`viewer/src/post.rs`, `shaders/post.wesl`) all of the above is ported: a compute
pass builds the histogram from the HDR target, a one-thread pass averages and adapts, and the
result is read back to become the camera's `Exposure` (pre-exposure, a few frames late; the
tonemap pass corrects the difference). Differences: the first frame jumps straight to the target
(the game's "immediate" adaptation mode, **assumption** about when it applies; screenshots
otherwise depend on the warm-up), and `PPRexpose_PS` (re-exposing history buffers) has no
counterpart because there is no TAA history. `BG3_BEVY_POST=1` restores Bevy's tonemapping with a
fixed exposure. `BG3_POST_DEBUG=1` prints the adapted EV100 and the metered target before
adaptation and clamping; `BG3_METERING=<mode>` overrides the metering mode (calibration only).

### Calibrating against the game's captures

The probes' specular captures (mip 0) are the game's own render of the level, in absolute
radiance (÷ 100), from known points. Metering a capture face like the exposure pass does (5–95 %
trimmed mean of texel EV100 `log2(100 · 8 · L)`, over the window a 45° camera with the viewer's
aspect sees; `cargo run -p bg3x --example light_survey` with `METER_PROBE=<name>`) and the viewer
from the same point and direction (uniform metering, `BG3_POST_DEBUG=1`) compares scene radiance
without involving post-processing or framing: `scripts/probe-calibration.sh PROBE…`.
The game's captures meter **EV100 6–8 from inside the ship** (e.g. `Interior_019` faces 6.0–8.3,
`Room3_001` 6.7–7.9), below the atmospheres' `ExposureMin` 8.5: in game the gameplay camera there
sits at the clamp as well, so the clamp in our shots is expected and image brightness follows
absolute scene radiance. Differences are listed under "Audit" below.

## Colour grading and tonemapping

Sources: `PPComputeLUT`, `PPColorGradingApplyLUTPS`, `PPToneMap_LUT`, `CombineUI`
(Core.metallib), `ls::PostProcessStep_ToneMap::ExecuteGraphics`, `rf::GraphicSettings`,
`ls::PostProcessStep_ComputeLUTs::ExecuteWTKernel`.

On SDR (the default, tonemapper version 20 → shader version 21) the display transform is a
**fixed 64³ LUT from the game data**, `Engine/Textures/ZCAM_SDR_4.dds` (`R10G10B10A2`, output
sRGB-encoded). `ToneMapHighlightsFixAmount` only feeds the ACES LUT of the HDR/legacy path and
has no effect here. Per pixel:

1. **Grading** (`PPComputeLUT`, baked by the game into a 32³ LUT indexed by `PQ(x/222)`, input
   pre-exposed linear Rec.709): `+ Brightness` (0), sRGB → AP1, a gamut expansion of saturated
   colours, the `Shadows` and `Highlights` curves of `NewColorCorrection`, white balance (identity
   at 6500 K / tint 0), hue rotation and `Saturation` in ICtCp (intensity and chroma of the
   saturated colour, hue of the original), the six ranges (`Global`, `Dark`, `Shadow`, `Midtones`,
   `Highlight`, `Specular`: chroma offset `XYOffset`, `Saturation`, `1.35^ExposureShift`) in
   Oklab with smoothstep weights on lightness, then `Contrast` around `log2(0.18 · 2^Pivot)` on
   AP1 luminance, and negatives lifted to 0.
2. **Display** (`PPToneMap_LUT`): an ACES-style reference gamut compression (thresholds 0.815 /
   0.803, limits 1.147 / 1.264), the ACEScc shaper, and the ZCAM SDR LUT sampled at the raw
   coordinate (no half-texel remap, a quirk of the game we keep).
3. `CombineUI`: the in-game gamma setting (default 1, identity).

In the viewer the grading is evaluated per pixel instead of through the 32³ LUT (so the LUT's
quantisation is not reproduced), the LUT is loaded from the install at start, and the encoded
output is decoded to linear for Bevy's sRGB swapchain (which re-encodes it). Not ported: white
balance other than 6500 K / tint 0 (logged), the VFX grading of spell effects, artist grading LUTs
(`GradingLutEnabled`) and screen fades. Bloom and the vignette: next sections.

The Nautiloid atmospheres: `ATM_Default_EV08_Avernus_Int` (EV 8.5–10.5, centre-weighted,
Highlights 10, blue shadows `ShadowRange.XYOffset (0.1, 0.01)`) and `ATM_Default_EV10_Avernus_Ext`
(EV 8.5–12, uniform metering, Saturation 1.1, `ShadowRange.ExposureShift 0.25`); both Contrast
1.12.

## Post-process steps

`ls::PostProcessStage::Load` creates the steps in this order (**assumption**: creation order is
execution order; `InsertStep` takes an index): ClearRenderView, GodRays, DOF, FadeBlending,
**Exposure**, ExposureOld, ReExpose, LensFlare, DarkenEnvironment, RadialBlur, SMAA, TAA,
History, **Bloom**, ChromaticAberration, ComputeLUTs, ColorGrading, InjectTexture,
**ToneMap**, CAS, **Vignette**, Calibration, FSR. So the exposure histogram sees the scene
without bloom, bloom is added to the pre-exposed HDR scene before grading, and the vignette
works on the display-encoded output. What the Nautiloid uses and the viewer does:

| Step | Gate | Nautiloid | Viewer |
|---|---|---|---|
| Exposure, grading, tonemap | — | yes | ported (above) |
| Bloom | atmosphere `Bloom` and the graphics option | exterior atmosphere only | ported (below) |
| Vignette | atmosphere `Vignette` (52 of 344 atmospheres) | off | ported (below) |
| GodRays | atmosphere `GodRays` (97) | off | not ported |
| LensFlare | `LensFlareEnabled` (9) | off | not ported |
| DOF | atmosphere / camera | — | not ported (cinematic) |
| SMAA / TAA / FSR | graphics options | — | none (no anti-aliasing) |
| CAS (sharpening) | graphics: sharpening on and no upscaler (`_GraphicSettings` +0xa1, +0x97) | user setting | not ported |

No film grain or dithering step exists in the SDR path. CAS only redistributes contrast
locally and changes the mean luminance little.

## Bloom

Sources: `PPBloomScaleBiasDownsampleAndBlur_PS`, `PPBloomDownsampleAndBlur_PS`,
`PPBloomUpsampleAndBlur_PS`, `PPBloomUpsampleBlurAndCombine_PS` (Core.metallib),
`ls::PostProcessStep_Bloom::ExecuteWTKernel`, `::LoadRenderTargets`, `::LoadShaders`,
`::UpdateIsRenderingInternal`, `ls::Atmosphere::Visit` (4.1.1.7631656).

- **Settings**: `PostProcessing/Bloom` (on/off), `Threshold` (engine default 0.4) and `Amount`
  (default 1.5) are copied straight into the threshold pass's `Params`. The step runs when the
  atmosphere's `Bloom` is set and the graphics option is on.
- **Targets**: 1/2, 1/4, 1/8, 1/16 of the screen (two each: down and up) and 1/32 (one).
- **Threshold** (scene → 1/2): four bilinear taps a quarter of a destination texel apart (one
  source texel each), each `c · sat(Amount · (dot(c, (0.3, 0.59, 0.11)) − Threshold))` on the
  pre-exposed scene, and the per-channel minimum of the four (suppresses fireflies).
- **Downsample** (1/2 → … → 1/32): a 3×3 tent (centre ¼, edges ⅛, corners 1/16) with taps one
  destination texel × `s` apart, where `s = height / 1080` (times render/output size when
  upscaling).
- **Upsample** (1/32 + 1/16 → 1/16, … → 1/2): this level + `Amount` (a constant 1) × the tent of
  the level below, taps two destination texels × `s` apart.
- **Combine**: scene + **0.2** × the 1/2 result (bilinear).

The Nautiloid: `ATM_Default_EV10_Avernus_Ext` has bloom on, threshold 1, amount 0.25 (so
only pre-exposed luminance above 1 blooms, fully from 5); the interior atmosphere has it off;
`ATM_Default` has 0.9 / 0.2.

In the viewer (`viewer/src/bloom.rs`, `shaders/bloom.wesl`) all of the above is ported,
between our exposure and tonemap passes, following the atmosphere of the camera. Differences:
the targets are `Rgba16Float` (the engine's format code is not decoded, **assumption**), the
frame's exposure lag is corrected as in the tonemap pass, and the graphics option is taken as
on. `BG3_NO_BLOOM=1` turns it off; `BG3_BLOOM=threshold,amount` forces other settings (checks
only). On the framed Lae'zel shot (exterior atmosphere) bloom changes the centre luma by 0.03:
the deck has almost nothing above the threshold.

## Vignette

`PPVignette_PS` with `ls::PostProcessStep_Vignette::ExecuteWTKernel` (4.1.1.7631656): when the
atmosphere's `Vignette` is set, the display-encoded output is mixed towards `VignetteColor`
(as stored) by `w = sat(VignetteIntensity · ‖|2(uv − ½)|^VignettePower‖)` (power per axis,
then the length). Defaults: intensity 0.5, power 7, colour black. Ported in the tonemap pass
(`post.wesl`); off on the Nautiloid. `BG3_VIGNETTE=intensity,power` forces it on (checks only).

## Light probes

Formats (Nautiloid): `_Diffuse_RGB` is a 16² BC6H cubemap holding irradiance/π, `_Specular_RGB`
a 128² BC6H cubemap with 8 GGX-prefiltered mips (mip m ≈ roughness m/8), the `_Alpha` maps BC4
cubemaps of the same sizes holding coverage (≈ 1 where the capture saw geometry, ≈ 0.05 towards
openings), `_SH.dat` 4 × float4 (`Y00, Y1−1, Y10, Y11`, each RGBA, the plain projection of
radiance). Faces are in D3D order and sampled with the BG3 world direction. Probes without
`LightProbeShape` are spheres (template default) and ignore `Size`.

Shading (`ClusteredLighting_*`): start from a global "distant probe", then for each probe of the pixel's cluster in list order lay
`probe · smoothstep(fade) · coverage` over the result (later probes win). The fade is
`sat((R − |p|)/TD)` for spheres and the same measured from `half − TD` for boxes; lookups are
parallax-corrected towards the volume's far side, less so for rough surfaces. Specular uses mip
`8 · r` and a split-sum LUT, diffuse the diffuse cube times the diffuse colour. Both are multiplied
by `Exposure · 100` (the `Exposure` buffer times the constant 100, checked in
`ClusteredLighting_4`): **captures are absolute radiance / 100** (cd/m² ÷ 100), not pre-exposed.
Then SSAO on the diffuse and Lagarde's specular occlusion. The SH is not used for surfaces
(only for fog). `SkyLight.Intensity` is not applied at run time; it only matters for baking.

The distant probe is two diffuse and two specular cubemaps (`DistantProbeDiff`/`Diff2`,
`DistantProbeSpec`/`Spec2`): diffuse `= Diff(N) · g_DistantProbeIntensity`, specular sampled at
mip `8r + g_DistantProbeMipLevels − 8`, and the second pair (with
`g_DistantProbeSecondIntensity`/`SecondMipLevels`) mixed in by `g_DistantProbeFade` when that is
above 0 (`ForwardLightingConstants`). What feeds them on the CPU is not found (the symbol index
has only the binding names `rf::sb::DistantProbe*`; Ghidra can't analyse the binary within its
time limit); the lighting resources have no field naming them (`SkyLight` has `Color`,
`Intensity` 5000, scattering and skydome settings). It only shows where no local probe covers a
pixel fully: on the imp deck `LP_TUT_Avernus_C_Interior_019` (sphere, R 32, TD 8, 23.6 m away)
covers it completely. (`LightProbe_Room3_Main`, a box over the deck, is `Enabled = false` in the
data and skipped.)

In the viewer (`viewer/src/probes.rs`) each probe with a capture for the active lighting is a
Bevy reflection probe (`LightProbe` + `EnvironmentMapLight`, intensity `100 · Intensity`; no face
swap needed, since Bevy's `(x, y, −z)` cube lookup undoes the viewer's z mirror), and the
character shader evaluates the same probes (hair with N and −N, blended 0.3/0.7). Approximated:
Bevy blends overlapping probes as a weighted sum instead of ordered lay-over, has no coverage
alpha, uses a box for spheres (2R), Bevy's own falloff and parallax, mip `7 r`, and only the 8
probes nearest the camera. There is no distant probe; Bevy's global ambient is off. Bevy
divides the summed probe weights out where they don't reach 1 (no camera environment map), so
partial coverage is renormalised rather than filled from a distant probe.

## Fog

All of the game's fog is volumetric (froxels: `VolumetricFogInitialize`, `VolumetricFogLighting`,
`VolumetricFogGather`, applied by `DeferredFog_*_PS` as `scene · T + inscatter`, pre-exposed).

- **Height layers** (`FogLayer0/1`, `ls::Fog::Layer::GetFogParameters`): density
  `D0 · (D1/D0)^((y − H0)/(H1 − H0))` (exponential through both points, extrapolated, capped at
  7.93), scattering `Albedo ·` density, optional noise when `NoiseCoverage` < 1. On the Nautiloid
  they are negligible (optical depth ≈ 0.5 % per 100 m) and the viewer doesn't render them.
- **Fog volumes** (`FogVolumes/_merged.lsf`, template `Shared_FogVolume`: Density 0.01, Albedo 1,
  sphere of Radius 5, Size 10, TD 1): extinction `Density`, scattering `Albedo · Density`, a
  *linear* fade over `TransitionDistance`; volumes of one `FogGroup` combine by max, `Replaces`
  volumes scale down the fog before them. Nautiloid densities range from 0.002 to 0.15 /m (the red
  rooms). Lighting: sun and local lights with a Henyey-Greenstein phase (g = `Fog/Phase`,
  **assumption**) and each light's `ScatteringScale`, plus the probes' SH ambient.

Ambient in the fog (`VolumetricFogLighting`, 4.1.1.7631656): start from the distant probe's
harmonics (`g_DistantProbeSH`), then for each probe of the froxel's cluster lay its harmonics over
the result with weight `sat(SH alpha) · smoothstep(fade)` (the same fades as the surface probe
loop: sphere `sat((R − |p|)/TD)`, box `1 − sat(|max(|p| − half + TD, 0)|/TD)`), each evaluated
as `c₀ · 0.282095 + g · 0.488603 · (c₁ d_y + c₂ d_z + c₃ d_x)` with `g = g_VolumetricPhase` and
`d` the froxel's direction from the camera, clamped at 0, times 100. So the fog sees the probes'
mean radiance plus a phase-weighted directional term, in the probes' units (÷ 100). Around the
imp deck the band-0 radiance is 20–46 cd/m² and the fog over the deck is dense
(`FogVolume_Avernus_Interior_Cockpit_Fog_Red`, a 54 × 4.7 × 48 m slab at y 12–17 with density
0.15, under `…_Interior_Cockpit`, 0.01), so this ambient is a large part of what the captures
show as haze.

In the viewer (`viewer/src/fog.rs`) each enabled volume is a Bevy `FogVolume` (box of `Size` or
2R) with the fade baked into a 16³ density texture, `density_factor = Density`, scattering =
the largest albedo channel (colour = albedo / that), absorption = the rest; every light is a Bevy
`VolumetricLight`. The probes' ambient is patched into Bevy's fog shader (`FogAmbientPlugin`, like
the falloff patch): per volume `albedo · L · (1 − T) · exposure`, with `T` the transmittance
through the volume along the ray and `L` the band-0 lay-over above averaged over the volume
(5³ samples weighted by the fade), carried in the volume's `light_tint`; Bevy's own camera
ambient term (unexposed, fading with depth) is dropped. Not reproduced: groups and `Replaces`
(Bevy adds volumes), noise, the ambient's variation inside a volume and its band-1 term, the
distant probe's harmonics (0 where no probe covers), the phase for local lights, per-light
`ScatteringScale`, box lights in the fog. `BG3_NO_FOG=1` turns fog off.
The ray march takes one step per 0.5 m inside a volume (4–64 steps; Bevy's fixed 64 patched for
performance, `docs/performance.md`); the game's froxel resolution is not read.

## Background

There is no clear colour: the sky goes into the emissive buffer (`PPEmissiveSkydome_Procedural_PS`:
the flat sky colour, or a skydome texture when `SkydomeEnabled`, plus Bruneton scattering, clouds
and a sun disc, exposed) and then through the fog. The flat colour and the scattering set-up are
CPU-side and not traced. The viewer uses what the probes recorded (`probes::apply_sky`): the
specular capture (mip chain as baked, ×100) of the probe whose capture saw the most sky
(coverage < 0.5 at mip 3) becomes the skybox — on the Nautiloid `LP_TUT_Avernus_C_Exterior`
(45 % sky texels), which shows the clouded Avernus sky and haze — falling back to a uniform sky
of the mean sky texel of all probes (≈ 43 / 36 / 28 cd/m²). **Assumption**: one capture stands in
for the background in every direction; geometry it saw near its probe shows wherever the level
has none. The atmosphere's `EnvironmentEffect` (storm-cloud VFX) is not rendered.

## Verification (2026-10-22)

`scripts/compare-references.sh` (coarse central-region sRGB means, 0–255, against local
screenshots of the original; game content, kept in `.cache/`), luma ours / reference:

| Shot | Before (EV 14, light scale 200, Bevy tonemapping) | After |
|------|------|------|
| `laezel` | 114.5 / 41.9 | 27.1 / 41.9 |
| `imp` | 108.6 / 32.4 | 51.5 / 32.4 |
| `avatar-deepgnome-cleric` | 110.3 / 31.6 | 30.3 / 31.6 |

The remaining differences are mostly framing (our framed shots are from further away and see
other geometry) and level objects the game doesn't draw (large flat helper planes catching
light); nothing was tuned to these numbers. Unit tests cover the trigger shapes and selection (`bg3_world::trigger`), probe and fog
template inheritance, the fog edge weights, the post-processing fields and the engine blackbody;
the install test `nautiloid_triggers_choose_lighting_probes_and_fog` checks the Nautiloid's
trigger choice, probe captures and fog volumes.

### Level shading and bloom (2026-10-22)

Same script after the level materials moved to the game's light loop and bloom/vignette were
added (luma ours / reference; captures repeat to ±0.03):

| Shot | Before | After |
|------|------|------|
| `laezel` | 28.7 / 41.9 | 28.6 / 41.9 |
| `imp` | 27.6 / 32.4 | 22.6 / 32.4 |
| `avatar-deepgnome-cleric` | 15.6 / 31.6 | 14.4 / 31.6 |

On the imp shot the game's inputs lit by Bevy's PBR (`BG3_LEVEL_BEVY_LIGHTING=1`) give 26.5:
most of the drop is the game's BRDF (renormalised Burley, remapped Smith), not the inputs.
Bloom changes these shots by < 0.1 (exterior atmosphere, threshold 1). In all three shots the
adapted exposure sits at the atmosphere's `ExposureMin` (EV100 8.5, `BG3_POST_DEBUG=1` prints
it): the metered scene is darker than the brightest exposure the game allows, so the game's
scene radiance must be higher than ours. The remaining gap is in the light sources and
ambient (the distant probe, box lights, sun transmittance, shadowing differences), not in the
post chain or the surface response.

### Light sources, probe fog and background (2026-10-22)

`scripts/compare-references.sh`, luma ours / reference, per step (captures repeat to ±0.05):

| Step | `laezel` | `imp` | `avatar-deepgnome-cleric` |
|------|------|------|------|
| Before (level shading and bloom, above) | 28.6 / 41.9 | 22.6 / 32.4 | 16.2 / 31.6 |
| Exact box lights, unshadowed | 33.4 | 69.1 | 16.5 |
| + box light shadows (carriers) | 24.2 | 24.4 | 10.7 |
| + fog ambient from the probes, captured background | 25.4 | 25.3 | 12.1 |

The unshadowed step shows how much `Light_Dungeon_Directional_Outside` (10000 lx) matters:
shadowed, the imp deck is lit blue by `Light_DIR_FillBLue_NS_001` and the blue fill points, with
the warm shaft only where the hull opens, as in `laezel-combat`. The avatar went darker: the start
pod is inside the shadowed box `Light_Dungeon_Directional_Sunlight_4800K_001`, and the spot-light
stand-in had lit it unshadowed; its reference is taken elsewhere (a fire-lit, hazy spot), so that
pair compares different places. All shots still sit at EV100 8.5 with metered targets of 6.0–6.4.

The calibration against the probes' own captures (`scripts/probe-calibration.sh`, 6 probes ×
6 directions around the deck: `Interior_019`, `Room3_001`, `Interior_039`, `Interior_016`,
`Room6_front`, `Exterior`), ours − game in EV:

| Step | Mean | Mean \|Δ\| | Worst probe (mean) |
|------|------|------|------|
| Box lights with shadows | −0.52 | 0.82 | `Interior_019` −1.00 |
| + fog ambient, captured background | −0.29 | 0.52 | `Interior_019` −0.62 |

The remaining deficit is largest looking −X from the cockpit (−1.3 to −1.4 EV, towards the open
hull): in the captures that view is a bright haze over the structure that our fog (one ambient
value per volume, no band 1, no distant probe, no `Replaces`) and background only partly
reproduce. Views straight down are slightly brighter than the game's (−0.1 to +0.3 EV from
`Interior_019`, `Interior_039`, `Interior_016` and `Room6_front`; +0.8 and +1.2 from `Room3_001`
and `Exterior`). The framed shots' remaining gap to the
references is then mostly framing (low close-up cameras against the references' high views onto
the lit deck) and the haze, not missing light sources.

### Audit: why the Nautiloid was too dark (2026-10-22)

Every framed reference shot sat at `ExposureMin` (EV100 8.5) with a metered target of EV ≈ 6
(`BG3_POST_DEBUG=1`): too little scene radiance. Checked, in order:

1. **Ambient / probes.** Units verified in `ClusteredLighting_4` (capture × `Exposure` × 100;
   diffuse = diffuse cube × diffuse colour × LUT, as Bevy's split sum does it); the deck is fully
   inside `LP_TUT_Avernus_C_Interior_019`, so the unported distant probe doesn't matter there.
   Probes and sun alone meter the framed Lae'zel shot at EV ≈ 4.75 (`--light-scale=0`): local lights
   dominate. Missing and added: the probes' **ambient in-scattering in the fog** (see "Fog"), and
   a **background** from the captured sky instead of a uniform mean (see "Background").
2. **Local lights.** All 332 enabled lights are spawned; Bevy's clustering doesn't drop any (on
   Metal it uses storage buffers, no 204-object cap). Point and spot units and falloff verified
   against the shader and `SetLightData`. Missing: **box lights**. The spot-light stand-in gave
   nothing outside `depth` from the source face, but the imp deck lies 20 m from the source face of
   `Light_DIR_FillBLue_NS_001` (300 lx blue, a 50 × 59 × 11 m box: the blue light of the
   `laezel-combat` reference) and 26–54 m into `Light_Dungeon_Directional_Outside(_001)` (2000 and
   10000 lx warm, shadowed). Ported exactly, with orthographic shadows (see "Light units").
   Unshadowed, the 10000 lx box floods the deck (imp shot luma 69 against 32): its shadow is what
   keeps the deck in blue shade.
3. **Light units.** Verified, no change: colour = `Color · blackbody · Intensity` with no CPU
   factor, 1/(4π) points, 1/π spots, none for boxes; `FlatFalloff`/`Gain` as documented; the sun
   is the scattering sun colour × 3000 lx (transmittance 1) at pitch 21°, shadowed.
4. **Exposure.** The game's own captures meter EV 6–8 inside the ship, so the 8.5 clamp is what
   the game does too, and brightness follows radiance. EV range, percentiles, compensation and
   metering mode of `ATM_Default_EV10_Avernus_Ext` are read as stored; no change.

## Open questions

- `CombinedLights`: how the gameplay half is evaluated (`eoc::GameplayLightComponent`:
  `m_Radius`, `m_EdgeSharpening`, `m_SpotLightAngle`, `m_DirectionalDimensions`, `m_IsSunlight`,
  `m_PointLightVerticalLimit`).
- The original trigger blending curve and entering-versus-leaving `FadeTime` semantics; probe list order
  (`ls::LightProbeHierarchySystem`); what feeds the distant probe (`rf::sb::DistantProbe*`,
  `g_DistantProbeSH`, intensities, fade) — a runtime sky capture is the likely answer, not shown.
- Whether `LightProbe_Room3_Main` (`Enabled = false`) is really off in game.
- The sun's transmittance precompute; the flat sky colour; `TimelineFog`.
- Which camera uses the second `ExposureSettings` entry; the bloom targets' format; whether
  step creation order is the execution order.
- God rays, lens flare, CAS and anti-aliasing (see "Post-process steps").
