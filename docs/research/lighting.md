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

- **Colour** of every local light: `Color · blackbody(Kelvin) (if UseTemperature) · Intensity`.
  No π or lumen/candela factor on the CPU. `blackbody` is a chromaticity on the Planckian locus
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
  centre of the source face) along local +Z. Inside it the light has a constant direction (along
  +Z), illuminance `C` (no π, no distance falloff), fades in over `AttenuationStart` from the
  source face, out over `AttenuationEnd` before the far face and over `AttenuationSide` at the
  sides, with the curve `…Function`. Light cookies can modulate it.
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
renders no local shadow maps). Approximations:

- **Box lights** are fill-light spot lights from the source face: falloff `4/R²` with `R =
  depth`, lumens `π · Intensity · depth²` (so the illuminance on the axis is `Intensity`), a cone
  covering the cross-section at mid-depth (inner = 0.7 · outer). The range window stands in for
  the far fade; the attenuation lengths and curve are not used; the direction is radial from the
  source point instead of constant.
- The sun's atmospheric transmittance is taken as 1 (the `Atmos_Transmittance` precompute and its
  units are not ported). The sun casts Bevy cascaded shadows up to `ShadowFarPlane`; local lights
  cast none. Bevy's volumetric fog uses Bevy's falloff, not the patched one.
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
fixed exposure.

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

Shading (`ClusteredLighting_*`): start from a global "distant probe" (**assumption**: a runtime
sky capture), then for each probe of the pixel's cluster in list order lay
`probe · smoothstep(fade) · coverage` over the result (later probes win). The fade is
`sat((R − |p|)/TD)` for spheres and the same measured from `half − TD` for boxes; lookups are
parallax-corrected towards the volume's far side, less so for rough surfaces. Specular uses mip
`8 · r` and a split-sum LUT, diffuse the diffuse cube times the diffuse colour. Both are multiplied
by `Exposure · 100`: **captures are absolute radiance / 100** (cd/m² ÷ 100), not pre-exposed.
Then SSAO on the diffuse and Lagarde's specular occlusion. The SH is not used for surfaces
(only for fog). `SkyLight.Intensity` is not applied at run time; it only matters for baking.

In the viewer (`viewer/src/probes.rs`) each probe with a capture for the active lighting is a
Bevy reflection probe (`LightProbe` + `EnvironmentMapLight`, intensity `100 · Intensity`; no face
swap needed, since Bevy's `(x, y, −z)` cube lookup undoes the viewer's z mirror), and the
character shader evaluates the same probes (hair with N and −N, blended 0.3/0.7). Approximated:
Bevy blends overlapping probes as a weighted sum instead of ordered lay-over, has no coverage
alpha, uses a box for spheres (2R), Bevy's own falloff and parallax, mip `7 r`, and only the 8
probes nearest the camera. There is no distant probe; Bevy's global ambient is off.

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

In the viewer (`viewer/src/fog.rs`) each enabled volume is a Bevy `FogVolume` (box of `Size` or
2R) with the fade baked into a 16³ density texture, `density_factor = Density`, scattering =
the largest albedo channel (colour = albedo / that), absorption = the rest; every light is a Bevy
`VolumetricLight`. Not reproduced: groups and `Replaces` (Bevy adds volumes), noise, the SH
ambient in-scattering (Bevy's fog ambient is unexposed and fades with depth, so it is off), the
phase for local lights, per-light `ScatteringScale`. `BG3_NO_FOG=1` turns fog off.

## Background

There is no clear colour: the sky goes into the emissive buffer (`PPEmissiveSkydome_Procedural_PS`:
the flat sky colour, or a skydome texture when `SkydomeEnabled`, plus Bruneton scattering, clouds
and a sun disc, exposed) and then through the fog. The flat colour and the scattering set-up are
CPU-side and not traced. The viewer uses what the probes recorded: the mean radiance of all
specular texels (mip 3) whose coverage marks no geometry, over every probe of the active lighting,
as a uniform `Skybox` (**assumption**: a uniform sky; the Nautiloid gives ≈ 43 / 36 / 28 cd/m²).
The atmosphere's `EnvironmentEffect` (storm-cloud VFX) is not rendered.

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

## Open questions

- `CombinedLights`: how the gameplay half is evaluated (`eoc::GameplayLightComponent`:
  `m_Radius`, `m_EdgeSharpening`, `m_SpotLightAngle`, `m_DirectionalDimensions`, `m_IsSunlight`,
  `m_PointLightVerticalLimit`).
- The original trigger blending curve and entering-versus-leaving `FadeTime` semantics; probe list order
  (`ls::LightProbeHierarchySystem`); the distant probe.
- The sun's transmittance precompute; the flat sky colour; `TimelineFog`.
- Which camera uses the second `ExposureSettings` entry; the bloom targets' format; whether
  step creation order is the execution order.
- God rays, lens flare, CAS and anti-aliasing (see "Post-process steps").
