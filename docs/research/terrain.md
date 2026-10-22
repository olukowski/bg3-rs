# Terrain

Status: format understood well enough to rebuild geometry, holes and layer weights; drawn with
the game's terrain shading (see Drawing). Implemented in
`crates/bg3-world/src/terrain.rs`. The viewer renders terrain from it (`viewer/src/terrain.rs`).
In the reference build, all 3,707 patches referenced by 753 terrains in 223 levels parse. One
patch (`EPI_CIN_VlaakithThroneRoom_A`) is referenced but not shipped.

Sources: hex inspection of patches; disassembly of `ls::TerrainPatchFileReader::ReadHeader`,
`ls::TerrainPatchFileReader::Read` and `ls::TerrainPatch::UpdateBound` (4.1.1.7631656, objdump on
the arm64 slice) to confirm field meanings; checks across every patch in the install.

## Files

Terrain files for a level live in `Mods/<Module>/Levels/<Level>/Terrains/`:

| File | Content |
|------|---------|
| `_merged.lsf` | Terrain objects (`Type=terrain`) in region `Templates`, as `GameObjects` nodes |
| `<MapKey>_<px>_<py>.patch` | Heightfield tile `(px, py)`. Each tile covers up to 64×64 cells |
| `<MapKey>_<n>.bin` | Physics heightfield chunks (magic `SEBD…`). We don't read these |

The `Visual/Data` (`<MapKey>.data`) and `Visual/Physics` (`<MapKey>.bin`) attributes name files
that do not exist under those exact names. Only one stale `.data` file ships. The engine builds the
tile names itself.

Terrain objects merge across modules by `MapKey`, the same way as other level objects. The patches
for a terrain are read from the directory of the module that defines it.

### Terrain object (`_merged.lsf`)

- `MapKey`, `Name`, `TemplateName` (a generic terrain root template), `LevelName`, `LayerList`.
- `Transform { Position, RotationQuat, Scale }`. This is the terrain's local origin, which is the
  corner of cell (0, 0).
- `Visual { Width, Height }`: size in **cells**. There are `Width+1 × Height+1` vertices.
  `Visual/LocalBound` covers roughly `[0, Width] × [minH, maxH] × [0, Height]`, with a small
  margin.
- `TerrainLayers/TerrainLayer[]`: `TerrainBrushResource` (a `TerrainBrushBank` ID) plus
  `ParamOverrides1/2` (fvec4) and `ParamOverridesMask`.
- `TerrainBaseLayers/TerrainBaseLayer`: `TerrainBrushResourceId` plus `Curves` (a height and a
  slope curve; used at runtime to split the base weight between two base layers, see Drawing).
- `Visual/HeightTransition` (blend transition width, default 0.2) and `Visual/CurvesResolution`
  (base layer ramp width, default 256; set by two terrains in the install).

### Patch file (`.patch`)

Little-endian. Versions 7 and 8 occur in referenced files. 77 version-5 files exist, but no level
references them (for example `WLD_Crashsite_D/ef47b058…`).

```
"PVersion"            8 bytes magic
u32 version           ≥ 6 has the next field; reader rejects < 5
u32 header_size       40 (v7) or 72 (v8); must match
i32 vertices_x, vertices_y        this patch's vertex counts (= cells + 1)
i32 cells_x, cells_y              ≤ 64
i32 terrain_vertices_x, _y        whole terrain (Width+1, Height+1)
i32 patch_x, patch_y              tile index; the patch starts at cell (64·px, 64·py)
i32 hole_count
i32 layer_count
[v8] i32[4], i32[4]               unknown; usually all −16384
f32 heights[vertices_y][vertices_x]   row-major, rows along local +Z
u32 holes[hole_count]                 cell indices in the WHOLE terrain, row stride terrain_vertices_x
layer_count × {
    u32 len; char brush_id[len]       TerrainBrushBank ID (GUID string)
    u32 len; u8 weight[len]           len = cells_x·cells_y, one byte per cell of this patch
}
u32 len; u8 blocks[len]               len = ceil(vx/4)·ceil(vy/4)·16; 16 bytes per 4×4 vertex block
[rest]                                present when layer_count > 4 (see below)
```

Notes:

- **Grid spacing is 1 unit.** `TerrainPatch::UpdateBound` builds a patch's bounds as
  `x ∈ [64·px, 64·px + cells_x]`, `z ∈ [64·py, 64·py + cells_y]`, with y from the min/max height.
  Neither the LSF nor the patch stores a scale.
- Neighbouring patches share their edge row/column of vertices: patch (0,0) of a 145-cell-wide
  terrain has 65 vertex columns, and patch (1,0) starts at vertex 64.
- **Holes** use whole-terrain cell coordinates `(i % terrain_vertices_x, i / terrain_vertices_x)`.
  Note that the stride is the vertex width, not the cell width. We confirmed this for all 1.35M
  hole entries in the install: every one falls inside its own patch's cell rectangle. The reader
  sets bits in a terrain-wide hole bitset. One code path re-strides indices when the patch is
  narrower than the file stride.
- **Layer weights** are one byte per cell and per layer. They often sum to about 255, but not
  always (see Drawing).
- **Block data** (v ≥ 7): the reader only accepts it when its length is exactly
  `ceil(vx/4)·ceil(vy/4)·16`. It is copied into a 2D GPU-side buffer with a 16-byte-aligned row
  pitch. It looks like a block-compressed texture (BC-style, 4×4 texels per 16 bytes), most likely
  the packed layer-index/weight map for the first four layers. **Not decoded.**
- **Extra data** (v8, more than 4 layers): a 64-byte block, then per-entry `u32` records that seem
  to fill a second weight texture for layers 5–8. **Not decoded.** We keep it as raw bytes.

### Terrain brushes (`TerrainBrushBank`)

`Public/<Module>/Content/Assets/TerrainTextures/[PAK]_TerrainTextures/_merged.lsf` (Shared and
SharedDev; 238 brushes after merging) has a `TerrainBrushBank` region. Each `Resource` has `ID`,
`Name` (`TT_Ash_A`, `TT_Coastal_Grass_A`, …), `BaseColorMap`/`NormalMap`/`PhysicalMap`
(`TextureBank` IDs), `DetailNormalMap`, `Tiling`, `MaterialType` (surface/footstep material?),
`IsBlackBrush`, height-blend parameters (`HeightScale`, `HeightOffset`, `HeightContrast`) and an
optional height gradient (`UseHeightGradient`, `Gradient*Color`, `GradientHeight*`). The same file
also has `TextureBank` entries for the brush textures.

## Levels and sub-levels

- `TUT_Avernus_C` (Nautiloid): 11 terrains, 37k cells. Mostly sculpted flesh/ash decks with many
  holes, for example `WLD_TUT_Avernus_C_Main_Cover_A` (145×155 cells, 9 patches, heights
  −44…14, 8.8k holes).
- `WLD_Main_A` has only 2 small, flat terrains of its own. Act 1's landscape lives in
  **sub-levels** such as `WLD_Crashsite_D` (5 terrains, main one 190×267 cells, 35 layers),
  `WLD_Forest_G`, `WLD_Plains_D`, … Their terrains are placed in the same world coordinates as
  `WLD_Main_A`. The viewer takes `--terrain-levels A,B,…` to add them.

## Drawing

Sources (4.1.1.7631656): disassembly (objdump on the arm64 slice) of
`ecl::EntityManager::AddTerrain`, `ls::Terrain::UpdateTerrainBrushData`,
`ls::Terrain::InitRenderResources` (base layer ramp), `ls::TerrainBrushResource` (constructor and
`Visit`), `ls::TerrainBrushSystem::RequestDeferred` (brush instance layout),
`ls::TerrainLayerSetting::Visit`, `eoc::TerrainTemplate::Visit` and `ls::FCurve`
(`Visit`, `UpdateDirty`, `GetValueAt`); the terrain shaders `Terrain_VS`, `Terrain_PS4`,
`Terrain_PS8` and `Terrain_PS12` in `Core.metallib` (`bg3x metallib Core.metallib --extract …`,
read with `llvm-dis`; the metadata names every buffer field and texture). The disassembly stays
in `.cache/`.

- **Every terrain is drawn.** The client gives each terrain object a `VisualComponent` and a
  `TerrainObject` with no condition on the template or the object (`Flag`, `WalkOn`,
  `IsDecorative`, … don't matter). On the Nautiloid the terrains follow the sculpted deck
  meshes closely: around the imp fight `WLD_TUT_Avernus_C_Lower_A` lies within about ±0.45 m of
  the deck surface, and from straight above it is the first surface hit for 30 of 169 sample
  rays (found with the viewer's `BG3_PICK`).
- **Geometry** (`Terrain_VS`): vertices at the cell corners plus one at each cell centre, so a
  cell is four triangles around its centre. Heights come from a heightmap texture holding
  heights normalised to the terrain's range (`Terrain_ScaleOffset` = range, minimum); the pixel
  shader receives that normalised height and the world height. The centre vertex reads the hole
  texture at its cell; a hole discards the whole cell. The viewer splits each cell along one
  diagonal instead (same outline, slightly different interior shape).
- **Terrain normal**: `Terrain_LocalNormalMap`, one texel per vertex, sampled at
  `(x + 0.5, z + 0.5) / (cells + 1)` with a clamping trilinear sampler.
- **Layer weights** (`Terrain_PS<n>`, `n` = layer count rounded up to a multiple of 4, up to
  64): one RGBA texel per cell and per 4 layers (an array slice per 4 layers from 8 layers on),
  sampled bilinearly at terrain-local `(x, z)` / cells, so texel centres sit at cell centres.
  Weights don't sum to 255 in the data (e.g. `[255, 0, 145, 0]`, `[0, 0, 18, 0]` on the
  Nautiloid); the blend doesn't need them to.
- **Projection**: from the terrain normal `n`, `b = max(|n| − 0.576, 0)` normalised. If one axis
  has all of it (on ground flatter than about 55°, only Y does) that axis projects; otherwise Y
  is replaced by 0.01 and only the larger of X and Z is kept, so steep slopes use **one** side
  projection, not a triplanar blend. With `s` = `b` signed like `n` and `h` the local height,
  `uv = (z·s.x + x·s.y − x·s.z, −z·b.y − (b.x + b.z)·h)`: `(x, −z)` on level ground,
  `(±z, −h)` / `(∓x, −h)` on the sides. Every layer texture samples `uv · Tiling` with explicit
  gradients from the untiled `uv`.
- **Height blend** (per layer, `w` its weight, `a` its base colour map's alpha, a layer with
  `w ≤ 0.001` is skipped): `v = w · |a|^c` if `w + a − 1 > AlphaThreshold`, else 0;
  `v *= HeightScale`; if `v > 0.01`, `v += HeightOffset`; the layer's height is `v` if
  `v > 0.01`, else −1 (absent). Only the **four highest** layers take part (the shaders for more
  than 4 layers sort the heights). With `h_i = max(height_i, 0)`,
  `base = sat(1 − Σh_i)`, `top = max(h_i…, base)` and `T = HeightTransition`:
  `g_i = w_i · (max(0, h_i − top + T) + 10⁻⁶)`, factor `f_i = g_i / (Σg + base · T)`, and the
  base layers get `rest = sat(1 − Σf_i)`.
- **Base layers** (`TerrainBaseLayers`, 0–2 per terrain, always one of the terrain's layers):
  `Terrain_BaseLayerIndices` are their layer indices (above 63 = none: the rest is drawn as a
  grey 0.1/0.2 checker in 2 m squares, normal up, metalness 0, roughness 1, AO 1). Two base
  layers split the rest by `Terrain_BaseLayerRamp`, `CurvesResolution` (default 256) texels
  wide, R16 unorm, 4 rows: rows 0/1 = slope curve of base layer 0/1 at
  `acos(i/(W−1))` degrees, rows 2/3 = height curve at `min + range · i/(W−1)` divided by the
  largest height-curve key value (at least 1); everything clamped to 0–1. The shader reads rows
  0/1 at `u = sat(n.y)` and rows 2/3 at the normalised height (gather + lerp):
  `A = slope₀ · height₀`, `B = 2 · slope₁ · height₁`, `A = 1` if both are 0; the base layers
  get `rest · A/(A+B)` and `rest · B/(A+B)`. The curves are the first (height) and second
  (slope) `Curve` of the base layer; `ls::FCurve` clamps the input to `CurveLimits` X (if the
  range is wider than 10⁻⁷), returns the first/last key's value outside the keys, interpolates
  per segment by the left key's `CurveKeyInterpolationType` (0 constant, 1 linear, 2 Hermite
  with `tan` of the stored tangents as slopes), and clamps the output to the Y limits.
- **Layer shading** (`SampleLayerCore`, for every layer and base layer with a factor above
  0.001): a black brush (`IsBlackBrush`) only adds its factor to a "black" sum. Otherwise the
  physical map (DXT1 array; R metalness, G roughness, B occlusion), the normal map (DXT5 array;
  `(A, −G, B)·2 − 1` with X and Y times the projection's sign `s.x + s.y − s.z`, X/Y rotated a
  quarter turn on the X side projection) and with `UseDetailNormalMap` the detail normal map at
  `uv · max(DetailNormalTiling, 0.01)`, combined by reoriented normal mapping with its XY scaled
  by `−DetailNormalScale`. With `UseHeightGradient` the base colour is multiplied by
  `mix(mix(Bottom, Mid, a), 1, b)` (`t` = smoothstep of the world height between
  `GradientHeightBottom` and `…Top`, `a = √smooth(sat(t / MidPoint))`,
  `b = √smooth(sat((t − MidPoint)/(1 − MidPoint)))`) and roughness by
  `sat(|GradientRoughnessReach + colour.r|^GradientRoughnessPower)`. Base colour, physical and
  normalised normal are summed with the factors.
- **Output** (G-buffer as in `character-shaders.md`): the summed normal in a tangent frame from
  the terrain normal (`T = normalize(n.y, −n.x, 0)`, `B = normalize(T × N)`), albedo
  `sat(Σ)`, AO `Σ physical.B + black`, roughness `sat(Σ physical.G + black · (1 − 2 sat(black −
  0.5)))`, reflectance 0.5, metalness `sat(Σ physical.R + (black ≥ 0.5 ? black : 0))`, shading
  model 1 (unlit) where black ≥ 0.975, else 0, plus 8 (receives decals).
- **Layer parameters** (`SB_TerrainLayerParams`, 64 × 112 bytes): from the brush, with
  `ParamOverridesMask` bits 1/2/4/8 taking `ParamOverrides1.xyzw` for `HeightScale`,
  `HeightOffset`, `HeightContrast`, `AlphaThreshold` and 16/32/64 taking `ParamOverrides2.x/z/w`
  for `Tiling`, `DetailNormalTiling`, `DetailNormalScale` (128 selects `.y`, which feeds
  nothing; unused in the data). The shader's contrast is `1 / HeightContrast`. Gradient colours
  are clamped to 0–1. A per-terrain bit set can zero a layer's `HeightScale` (turning it off).
  Brush defaults (resource constructor): `HeightScale` 3.5, `HeightOffset` 0,
  `HeightContrast` 1, `AlphaThreshold` 0, `Tiling` 0.25, `DetailNormalScale` 0.6,
  `DetailNormalTiling` 0.0278, gradient heights −0.25 / 0.1 / 10.5, roughness reach 0.4,
  power 5, `UseDetailNormalMap` on; every brush in the reference build sets them all anyway.
  `Visual/HeightTransition` defaults to 0.2 (`eoc::TerrainTemplate::Visit`); 213 of 753
  terrains leave it out.
- **Brush textures** live in texture arrays shared by all terrains
  (`ls::TerrainBrushSystem::ManagedTextureArray`): `DXT5_sRGB` (base colour), `DXT5` (normal
  and detail normal maps), `DXT1` (physical maps), at 1024/512/256/128 texels with 11/10/9/8
  mips by texture quality (`TEX_RES`, `TEX_MIPS`).

Survey of the install: 458 terrains have one base layer, 222 two, 73 none; the base brush is
always also a regular layer.

### In the viewer

`viewer/src/terrain.rs` and `shaders/terrain.wesl` draw terrain with this shader: brush maps in
three BC texture arrays at 1024² (`bg3_world::terrain_shading::load_texture_arrays`; the
Nautiloid's and the crash site's maps are all BC3/BC1 of at least that size, so the blocks are
used as they are; anything else would turn its array into RGBA8), per terrain the layer
parameters, weight map array, vertex normal map and base ramp (`terrain_shading`, unit-tested,
with a CPU model of the blend), and the result goes through the game's light loop
(`game_lighting.wesl`) like level meshes. Exact: the projection, height blend, top-four
selection, base layer split and ramp, checker fallback, layer shading (maps, detail normals,
height gradient), override masks, defaults and output channels. **Assumptions** and
approximations:

- The vertex normal map is built from central differences of the heights (the engine builds it
  itself; its filter wasn't traced), stored as RGBA8.
- `Terrain_Sampler` (weights) is taken to clamp, and the layer sampler is trilinear with 8×
  anisotropy (the game's follows the graphics settings).
- Every layer is enabled (what clears the per-terrain layer bit set wasn't traced).
- Triangulation (see Geometry) and holes are as before; decals aren't drawn.
- Shading model 1 (black brushes) outputs the albedo unlit; the deferred pass's handling of
  model 1 wasn't read.
- The Bevy mesh normal (for shadow bias) is per vertex from the heights, not the shaded normal.

`BG3_STANDARD_TERRAIN=1` keeps the earlier path for A/B: a base colour texture baked per patch
on the CPU (`bg3_world::terrain_albedo`, linear blend of the normalised weights, level-ground
projection only) on Bevy's `StandardMaterial`. `BG3_TERRAIN_BEVY_LIGHTING=1` lights the game's
terrain inputs with Bevy's PBR. Before the bake the viewer coloured terrain with flat
placeholder colours per brush name, which on the Nautiloid drew a smooth pink-orange sheet
through the lower deck (easily mistaken for the Avernus backdrop).

Checked by eye (no reference capture of terrain from the original yet): on the Nautiloid's
lower deck the ash/flesh boundaries are now sharp and follow the flesh texture's height, with
normal-mapped relief; on the crash site (`WLD_Main_A --terrain-levels WLD_Crashsite_D
--terrain-only --camera=220,45,190 --look-at=250,12,250`) cliffs take the side projection and
the large white patches of the baked version are gone.

## Open questions

- **Sub-level registry:** how does the engine know which levels make up `WLD_Main_A`? Searches
  turned up nothing: no LSF attribute in `Mods/*` names the sub-levels, `meta.lsf` has no list,
  and `Layers/*.lsx` are editor layers. The only symbol is `strSubLevelName`. Next step: find where
  level loading enumerates `Levels/` (REA).
- Triangle layout: the engine draws four triangles per cell around a centre vertex (see
  Drawing); the viewer still splits each cell along one diagonal.
- Meaning of the v8 header's two `IVec4`s, of the block data, and of the >4-layer extra section.
- How the engine builds `Terrain_LocalNormalMap`, and what clears a layer's bit in the
  per-terrain layer set (both assumed above).
- A reference capture of terrain from the original game to compare against.
