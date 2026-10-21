# Lighting

Status: placed lights and level-wide lighting are parsed (`crates/bg3-world/src/lights.rs`) and
mapped to Bevy lights in the viewer (`viewer/src/lights.rs`). The unit conversion is approximate
and tuned by eye. It is not verified against the original.

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
| `LightType` | u8 | 0 = point, 1 = spot, 2 = directional. Inferred: spots carry `Angle` and have names like `*_Spot*`/`*Spotlight*`; type 2 carries `DirectionLightDimensions` and has names like `*_Directional_*`. Enum names are not found in the binary (`ls::ELightType`) |
| `Color` | fvec3 | linear filter colour |
| `Intensity` | float | 0–~10⁴. Unit unclear, see below |
| `Kelvin`, `UseTemperature` | float, bool | colour temperature, applied when `UseTemperature` is set |
| `Radius` | float | range / attenuation radius |
| `Angle` | fvec2 | spot cone, degrees, (inner, outer). Combined lights have `GameplaySpotlightAngle = (x+y)/2` |
| `FlatFalloff`, `Gain` | bool, float | falloff shape. Unknown details |
| `DirectionLightDimensions` | fvec3 | box size (w, h, depth) of type-2 lights |
| `DirectionLightAttenuationStart/End/Side`, `…Function` | float, u8 | box-light falloff |
| `Shadow`, `CastShadow`, `VolumetricShadow` | bool | |
| `PreExpose` | bool | intensity is relative to the current exposure (presumably) |
| `IsFlickering`, `Amount`, `Speed`, `IsMoving`, `MovementSpeed/Amount` | | animation |
| `ScatteringScale` | float | volumetric fog contribution |
| `LightChannelFlag` | u8 | light channel mask |
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
known. Levels may also switch lighting at runtime (atmosphere/lighting triggers); we don't handle
that yet.

## Viewer mapping (assumptions)

- **Exposure:** a camera `Exposure { ev100 = ExposureMin − ExposureCompensation }` (14). The game
  uses auto-exposure in [14, 16].
- **Sun:** `DirectionalLight { illuminance = SunIntensity }`. Direction from yaw/pitch with
  `(cos p · sin y, sin p, cos p · cos y)` pointing at the sun in BG3 space, then mirrored. The yaw
  zero and handedness are **unverified**.
- **Ambient:** `GlobalAmbientLight { color = SkyLight.Color, brightness = SkyLight.Intensity / π }`
  (a uniform sky's luminance).
- **Local lights:** `lumens = Intensity · 4π · light_scale`, with `light_scale = 200` by default
  (`--light-scale`). This was tuned so the Nautiloid reads at EV 14. Most Nautiloid lights are
  `PreExpose`, which suggests the engine scales them by the exposure. The real formula is still
  open. `range = Radius`.
- **Direction:** spot and box lights are assumed to shine along their local **+Z** (BG3 space),
  then go through `space.rs`. Evidence is weak: for strongly tilted lights, +Z and +Y point down
  about equally often. Box depth is the largest dimension, which fits +Z.
- **Spot angles:** `Angle` is taken as (inner, outer) **half-angles**.
- **Box lights (type 2)** are approximated by a spot light at the object origin covering the box
  cross-section, with `range = depth`.
- Colour temperature uses a black-body fit (Tanner Helland), not the engine's table.
- Shadows, cookies, flicker, fog and volumetrics are not implemented.

## Open questions

- The unit and exposure relationship of local-light `Intensity` (`PreExpose`, `Gain`,
  `FlatFalloff`), and the falloff curve.
- `LightType` enum names. The forward axis of spot and box lights, and whether the box origin is
  its centre or its source face.
- Sun yaw/pitch convention. Which of the two `ExposureSettings` is active.
- `CombinedLights`: how the gameplay half is evaluated (`eoc::GameplayLightComponent`:
  `m_Radius`, `m_EdgeSharpening`, `m_SpotLightAngle`, `m_DirectionalDimensions`, `m_IsSunlight`,
  `m_PointLightVerticalLimit`).
- `LightProbes/*_SH.dat`: 64-byte spherical-harmonics probes, not parsed yet.
- `FogVolumes/_merged.lsf` and lighting/atmosphere triggers.
