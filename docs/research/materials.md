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
  viewer re-packs to RG8 (R = A, G = G) and lets Bevy reconstruct Z. Integrability of the maps
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

## DDS files

All 80 textures the Nautiloid's materials reference are legacy-header DDS (no `DX10` header, so no
colour-space info: use the bank's `SRGB`), 2D, power-of-two, full mip chains: 43 BC1, 22 BC3, 8 BC4,
1 BC5 (6 IDs have no texture resource). No BC7 or cube maps among them.

## Nautiloid survey (`bg3x materials`, `TUT_Avernus_C`)

- 113 materials on LOD0 meshes (1 not in any bank). **94 take their main maps from virtual
  textures** (`Base_VT`, `Base_SSS_VT`, `ENV_DetailMap_VT`, `Base_GM*_VT`, `Base_Translucency_VT`,
  `Base_AlphaTest_2s_SSS_VT`, …), covering almost all walls, floors and organic architecture.
- 15 have a plain base-colour map: glass (`Base_Glass*`), brain jars / aquarium
  (`VFX_Model_AlphaTest_DefaultLit_*`), `ENV_DetailMap` (non-VT exterior wall), membranes
  (`Base_SSS_AlphaTested`), shadow planes.
- 3 have neither (`Helper_Color*` flat colours, the invisible overlay).
- 33 shader graphs in total. `Helper_Invisible*` / `*_Invisible_*` graphs are helper geometry the
  game doesn't draw; the viewer skips them.

## Viewer approximation (`viewer/src/materials.rs`)

`StandardMaterial` per material ID: base colour + alpha mode, RG8 normal map, swizzled physical map
as both `metallic_roughness_texture` and `occlusion_texture`, glow map as emissive. VT materials get
flat grey (roughness 0.8) plus their glow map. Textures are deduplicated by (file, usage) and loaded
with rayon: 51 textures (119 MiB GPU data) in ~40 ms on an M5 Pro.

## Open

- Virtual textures (Granite `.gts`/`.gtp` tile sets referenced by `GTexFileName`) — needed for
  almost every Nautiloid surface.
- Detail maps (`ENV_DetailMap*`: `MicroDetailMap`, tiling, strengths), SSS, translucency, glow
  animation, `Reflectance`, vertex-colour masks — would need a custom Bevy material/shader.
- The glass dithering and the exact alpha-test threshold.
