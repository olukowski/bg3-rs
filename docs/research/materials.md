# Materials

Status: investigated with `bg3x lsx`, `bg3x materials [-v]` (survey of a level's materials) and
`cargo run -p bg3x --example normal_convention`; implemented in `crates/bg3-material` (pure) and
`viewer/src/materials.rs` (Bevy glue). Reference build 4.1.1.7631656.

## Two layers: shader graph + instance

**Shader graph** — `Public/<Module>/Assets/Materials/**/<Name>.lsf`, region `Material`. Holds no
node graph, only the compiled interface:

- Render-state flags on the root node: `ShadingModel` (0 default lit, 1 unlit, 3 subsurface),
  `BlendStateID`, `RasterizerStateID`, `DepthStateID`, `MaterialType`, `MaterialUsage`, `UVCount`,
  `IsDiscardingPixels`, `VertexColorEnabled`, `LateAlpha`, `IsOpaqueCustom`, `DiffusionProfileUUID`, ….
- `ShaderDescriptions/ShaderDesc_<Pass>` for every pass (`StaticForward`, `StaticDeferred`,
  `StaticDepth`, `StaticDepthShadow*`, `StaticVelocity`, `StaticEmissive`, `StaticBake`, and
  `Instanced` variants): constant-buffer sizes and offsets of engine parameters.
- Parameter lists: `ScalarParameters`, `Vector3Parameters`, `Texture2DParameters`,
  `SamplerStateParameters`, `VirtualTextureParameters`. Each has `ParameterName`, the default
  (`Value` or `ID`), `UniformName` (e.g. `Texture2DParameter_basecolor`) and per-pass binding
  children (`StaticForward`, `StaticDepth`, …) with `DxVsIndex`/`DxPsIndex`/Vulkan slots (`-1` =
  not bound in that pass). Unnamed texture inputs exist (`ParameterName=""`, uniform named after
  the default ID).
- Compiled shaders sit next to it as `<Name>_<ST|STI>_<PASS>_<API>.bshd` (`ST` static, `STI`
  instanced; passes `DEF`, `FOR`, `DEP`, `DEPS`, `DEPST`, `VEL`, `EMI`, `BAKE`; APIs DX11, DX12,
  DX12XS, Vulkan, PS5, Metal). On macOS the Metal `.bshd` is a 168-byte stub naming entry points in
  a shared library. A handful of GustavX graphs also ship readable **HLSL** (`_HLSL.shd`), which is
  how the channel layouts below were confirmed.

**Instance** — `MaterialBank` resource in a `Public/**/Content/**/_merged.lsf` bank:
`SourceFile` (the graph), `MaterialType`, `DiffusionProfileUUID`, and a copy of the graph's
parameter list. Each instance parameter carries `Enabled`, `Value` (instance value), `BaseValue`
(graph default) and `ExportAsPreset`/`GroupName` (editor metadata). **`Enabled == true` means
the instance overrides the parameter**; otherwise the graph default applies (instances keep stale
copies of defaults). Texture parameters reference `TextureBank` IDs; virtual-texture parameters
reference `VirtualTextureBank` IDs.

`TextureBank` resources: `SourceFile` (DDS path), `SRGB`, `Type` (1 = 2D), `Width`, `Height`,
`Depth`, `Streaming`, `Template`. `VirtualTextureBank` resources: `GTexFileName` (tile-set file
stem), `VirtualTextureLayerConfig` (3 on the Nautiloid), `ReferencedColorSpaces`, `Prefetch*`; the
`SourceFile` is empty.

### Render states (observed)

| Field | Value | Seen on |
|---|---|---|
| `BlendStateID` | 7 | every opaque and alpha-tested graph |
| `BlendStateID` | 0 | `VFX_*Alphablend*` graphs |
| `RasterizerStateID` | 1 | one-sided graphs |
| `RasterizerStateID` | 0 | every `*_2S`/`*_2s` (two-sided) graph |
| `IsDiscardingPixels` | true | nearly everything — **not** an alpha-test flag (dithered fade, `FadeOpacity`) |

**Alpha testing** shows up structurally: the `StaticDepth` pass binds the base-colour texture (or
the virtual texture, for `*_AlphaTest_VT`) in its pixel shader (`DxPsIndex >= 0`), while opaque
graphs' depth passes bind nothing but the dither noise. `bg3-material` uses that rule.

`Base_Glass*` graphs are alpha-tested on a mostly transparent `_BMA` (alpha 0.2–0.4); in game the
glass is see-through, presumably via dithering against the noise texture. The viewer
approximates them with alpha blending.

## Channel layouts

From the HLSL of `CHAR_BASE_SSS_GM_MSK_Spectre_Skin_VT` (`…_ST_DEF_HLSL.shd`) and per-channel
statistics of every Nautiloid map (`bg3x materials -v`):

- **Normal map `_NM`** (BC3, linear): sampled `.wzyx`, i.e. tangent-space **X = A**, **Y = G**,
  **Z = B**; R is always 0. The shader then maps `x·T + z·N − y·Binormal` (it negates the Y). The
  viewer re-packs to BC5 (R = A, G = G) and lets Bevy reconstruct Z. Integrability of the maps
  shows green points along **+v** (rows down); the GR2 tangent frame mirrored into Bevy (see
  `coordinate-space.md`) has T along +u and the bitangent along +v, so **Bevy needs no
  `flip_normal_map_y`** (`examples/normal_convention.rs` checks all Nautiloid normal-mapped meshes).
- **Physical map `_PM`** (BC1) / **`_PMA`** (BC3, alpha = an extra mask), linear: **R metalness,
  G roughness, B ambient occlusion**. Metal and occlusion are read from `.x`/`.z` of the VT physical
  layer in the HLSL; roughness = G by elimination and matches the statistics (R≈0 on organic
  surfaces, B≈1 with dark crevices, G mid-range). Bevy wants occlusion R, roughness G, metal B: an
  R↔B swap, done exactly on the BC1 endpoints (`bc::swap_red_blue_bc1`).
- **Base colour `_BM`** (BC1) / **`_BMA`** (BC3, alpha = opacity), sRGB (`SRGB=true` in the bank).
- **Glow map `_GM`** (BC1 or BC4; `SRGB` varies): multiplied by `GlowColour`/`GlowColor` and a
  strength (`GlowMultiplier`, `GlowIntensity`, …); many graphs animate it (`MinGlow`/`MaxGlow`,
  `PulseSpeed`). Not yet modelled: animation, the true intensity scale.
- Other inputs (`MicroDetailMap`/`MicroDetailNormalMap` `_MSK`/`_NM`, `SSS_Thickness`, `MSKcloth`,
  noise/gradient `VFX_BC4_*`, the unnamed `REF_Cloud_B` dither noise) are graph-specific and unused.

The virtual texture has three layers (`SampleVirtualTexture(…, layer)`): **0 = base colour,
1 = normal (same `.wzyx` swizzle), 2 = physical** (x metal, z occlusion, w SSS mask/thickness).
Statistics of all 81 Nautiloid VTs (`bg3x materials -v`, coarse mip via `bg3-vt`) show the same
channel layouts as the DDS maps for every graph, including `ENV_DetailMap_VT`: layer 1 has R = 0,
G/A ≈ 0.5, B ≈ 1; layer 2 has R ≈ 0 except on metal, G mid-range, B ≈ 1 with dark crevices, A = 1.
The albedo layer's alpha is opacity (e.g. glass ≈ 0.25).

## DDS files

All 80 textures the Nautiloid's materials reference are legacy-header DDS (no `DX10` header, so no
colour-space info: use the bank's `SRGB`), 2D, power-of-two, full mip chains: 43 BC1, 22 BC3, 8 BC4,
1 BC5. No BC7 or cube maps among them. Defaults such as `Engine_Default_White`,
`Engine_Default_Red`, `Engine_Default_BM` (checker) live in the **engine's** banks
(`Public/Engine/Content/[PAK]_Engine/_merged.lsf`), which `ResourceBanks::load` therefore reads
before the modules.

## Nautiloid survey (`bg3x materials`, `TUT_Avernus_C`)

- 113 materials on LOD0 meshes. **94 take their main maps from virtual textures** (`Base_VT`,
  `Base_SSS_VT`, `ENV_DetailMap_VT`, `Base_GM*_VT`, `Base_Translucency_VT`,
  `Base_AlphaTest_2s_SSS_VT`, …), covering almost all walls, floors and organic architecture.
- 15 have a plain base-colour map: glass (`Base_Glass*`), brain jars / aquarium
  (`VFX_Model_AlphaTest_DefaultLit_*`), `ENV_DetailMap` (non-VT exterior wall), membranes
  (`Base_SSS_AlphaTested`), shadow planes.
- 4 have neither: `Helper_Color*` flat colours, the invisible overlay, and
  `Engine_Default_Mesh_Lit_Opaque` (engine placeholder on 100 meshes; its maps are unnamed inputs).
  It is an ordinary lit opaque graph with shipped compiled shaders, so nothing in the material
  hides those meshes: they are scenery blockers and cloth simulation meshes, which the game
  doesn't draw for other reasons (`level-structure.md`, "What the game doesn't draw").
- 34 shader graphs in total. `Helper_Invisible*` / `*_Invisible_*` graphs are helper geometry the
  game doesn't draw; the viewer skips them.
- The `VFX_Model_*` graphs glow from a vertex-colour mask unless `_Use_GlowMap_InsteadOfVCMask`
  is set (their glow map is then an engine default), so the viewer gives them no glow.

### Which graphs cover the Nautiloid

Mesh uses per graph (`bg3x materials TUT_Avernus_C`, LOD0): `Base_SSS_VT` 844 (flesh walls,
cables, organic doors), `ENV_DetailMap_VT` 720 (deck/ground, pods, tables), `Base_VT` 505,
`Base_AlphaTest_2s_SSS_VT` 429 (the destroyed walls), `Base_Translucency_VT` 322, then the
glow graphs (`Base_GM_VT` 84, `Base_GM_Wind_VT` 74, `Base_GM_Pulse_Bouncing_AlphaTest_VT` 56,
`Base_GM_Pulse_VT` 50, `Base_Glass_Glow` 36, …). Screen coverage was not measured; these
use counts decided what was ported.

## Level shading (from the compiled shaders)

Read from the `DEF` (G-buffer writer) and `EMI` (emissive) passes of each graph in
`Materials.metallib` (`bg3x metallib --extract …`, `llvm-dis`; disassembly kept in `.cache/`)
and the deferred lighting `ClusteredLighting_4` in `Core.metallib`, 4.1.1.7631656.

**Common G-buffer** of every `Base_*`/`ENV_*` graph (targets as in `character-shaders.md`):
albedo = `sat(VT layer 0 .rgb)` (or the `basecolor` map), occlusion = physical **B**, the
normal map decoded as described above, roughness = `sat(physical G)`, reflectance =
`sat(Reflectance)` (graph default 0.5; glass instances use 3.5, i.e. 1), metalness =
`sat(physical R)`, flags = shading model + 8 (receives decals) + light channel. So Bevy's
standard inputs carry everything except the reflectance, which the viewer now passes.

Per graph:

| Graph | Model | Target 3 / extras |
|---|---|---|
| `Base_VT`, `Base_AlphaTest_VT`, `Base_2S_VT` | 0 | — |
| `Base_SSS_VT` | 3 | profile slot, amount 1, thickness `sat(SSS_Thickness.r)`, transmission on |
| `Base_AlphaTest_2s_SSS_VT` | 3 | amount `sat(SSS_Radius · SSS_MSK.r)` (Nautiloid `SSS_Radius` 0: no blur), thickness 0, transmission on |
| `Base_Translucency_VT`, `Base_GM_*_Translucency_VT` | 4 | profile slot, thickness `sat(physical A)` |
| `ENV_DetailMap_VT` | 0 | micro detail layer (below) |

**Model 4 (translucent)** in the lighting: the BSDF takes the profile's scatter colour and
distance, tint, thickness remap and thick-object flag like model 3, but the scatter amount
stays 1 (target 3's green is not read) and transmission is always on; there is no
screen-space blur, no dual-lobe specular, and the diffuse keeps its albedo. Point and spot
lights add the model-3 transmission term (`DoClusteredLight_Translucent`); the sun is lit with
the default BRDF only (no transmission). The Nautiloid's translucent graphs have no profile,
so slot 0 (`Engine_Default_DiffusionProfile`, distance 0) applies: no transmission except
where the thickness is 0.

**`ENV_DetailMap_VT` micro detail.** Detail UV = `uv · MicroDetailTiling · s`, where `s` is the
object's scale (distance of the pixel from the object origin over the length of its local
position). `MicroDetailNormalMap` (decoded like every normal map, normalised) times
`DetailNormalStrength`, with its normal component zeroed, is added to the main tangent-space
normal before normalising; occlusion ×= `clamp(pow(MicroDetailMap.r, DetailAOStrength))`;
roughness = `sat(physical G + DetailRoughnessStrength · (1 − MicroDetailMap.g))`.

**Glow (`EMI` passes).** With `GM` the glow map, `c` the glow colour and
`sel = mix(GM.r · c, GM.rgb, step(UseGMColor))` (`floor` clamped to 0–1):

| Graph | Emissive |
|---|---|
| `Base_GM_VT` | `sel · GlowMultiplier` |
| `Base_GM_Pulse_VT` | `sel · GlowMultiplier · lerp(MinGlow, MaxGlow, ½ + ½cos(t · PulseSpeed)) · exposure` |
| `Base_GM_Pulse_VT_FX` | the same without the exposure |
| `Base_GM_Pulse_AlphaTest_Translucency_VT` | `GM.rgb · c · GlowMultiplier · pulse` |
| `Base_GM_Wind_VT`, `…_Wind_Translucency_VT` | `sel · lerp(GlowMin, GlowMax, ½ + ½sin(t · GlowPulseSpeed))` (no multiplier) |
| `Base_GM_Pulse_Bouncing_AlphaTest_VT` | `GM.rgb · c · GlowMultiplier · lerp(MinGlow, MaxGlow, s)`, `s` = smoothstep of the triangle wave `|2 fract(t · PulseSpeed + |object origin| + local y / MeshPulseSize + vertex colour B + ½) − 1|` |
| `Base_Glass_Glow` | `c · GM.rgb · GlowMap_Color · GlowIntensity · mix(1, smoothstep(GlowGradientMinHeight, …MaxHeight, local y), sat(Height_Mask))` |

`t` is `g_CurrentTime` (s). Only `Base_GM_Pulse_VT` multiplies by the camera exposure (which
is why its multipliers are in the hundreds); the others write display-referred values that
the emissive buffer adds unexposed. A BC4 glow map samples as (r, 0, 0).

### In the viewer (`viewer/src/level_materials.rs`, `shaders/level.wesl`)

Textures (`materials.rs`): virtual-textured materials use their three VT layers (via
`bg3-vt`, BC3 tiles kept compressed, converted like the DDS maps) from mip 1
(`BG3_VT_LEVEL` overrides; 0 = full resolution). Textures are deduplicated by (file or VT,
usage) and loaded with rayon. On an M5 Pro: 298 textures (55 DDS + 243 VT layers) = 276 MiB in
~65 ms at mip 1; 785 MiB in ~200 ms at mip 0 (before the detail, thickness and raw glow maps
were added).

Each level material is still built as a `StandardMaterial` (`materials.rs`, unchanged:
base colour + alpha mode, BC5 normal map, R↔B-swapped physical map for roughness/metalness
and occlusion), and a game-shaded `ExtendedMaterial` over it replaces it on every spawned
mesh (`swap_level_materials`). The extension adds the reflectance, the model and subsurface
inputs, the detail layer and the glow (`bg3_material::level` classifies graphs and packs the
uniform, unit-tested), and lights the surface with the game's loop, shared with characters
(`shaders/game_lighting.wesl`: Burley/GGX/remapped Smith, specular anti-aliasing, sun-disc
specular, dual-lobe specular and profile transmission for 3, transmission without the sun for
4, the game's falloff, the level's probes). Subsurface pixels are marked for the screen-space
scattering pass like skin, with profile kernel slots shared with the characters.
`BG3_STANDARD_LEVEL=1` keeps the old `StandardMaterial`s for A/B.

Exact: the G-buffer channel use, reflectance, model-3/4 inputs and lighting (as exact as the
character port), the detail layer's UV scale, normal, roughness and occlusion, and the glow
formulas and units. Approximated:

- The detail normal is added in world space after Bevy's normal mapping (equal for an
  orthonormal tangent frame); normal maps reconstruct Z from X/Y instead of reading B.
- `Bouncing` glow: our level meshes carry no vertex colours, so that phase term is 0.
- Screen-space scattering blurs the final colour (pre-scatter texturing), as for skin.
- Not ported: the VFX model graphs' vertex-colour glow, `Base_SSS_Fresnel_GM_VT` and
  `ENV_GM_Pulse_Fluid_VT` (one material each, no glow here), wind/vertex animation
  (`Base_GM_Wind_*`, `Base_UV_Deform_VT`), glass dithering (still alpha-blended), light
  channels, SSAO.

A/B, centre luma: imp deck view (`--play --combat-test --no-follow --camera=-42,14.5,-409
--look-at=-46.6,3.5,-402.8`) 21.4 (game shading) against 22.8 (standard); the framed imp shot
22.6 (game shading), 26.5 (game inputs on Bevy's lighting, `BG3_LEVEL_BEVY_LIGHTING=1`), 27.6
(standard). The renormalised Burley diffuse
(`lerp(1, 1/1.51, r)`) loses some energy on rough surfaces; glow maps now show their
detail at the graph's intensity instead of the earlier guessed 150 nits.

## Open

Character graphs (skin, hair, eyes, clothing palettes, vertex-colour cuts) have their own
material: see `character-shaders.md`.

- Vertex-colour masks, world-position offset and wind animation; the non-VT `ENV_DetailMap`,
  `Base_Glass*` forward passes and the VFX graphs are on the common path without their extras.
- The glass dithering and the exact alpha-test threshold; `BlendStateID`s other than 0 and 7.
- Streaming/feedback-driven VT residency (we extract whole textures up front).
