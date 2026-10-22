# Terrain

Status: format understood well enough to rebuild geometry, holes and layer weights; drawn with
baked brush textures (simplified blend, see Drawing). Implemented in
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
- `TerrainBaseLayers/TerrainBaseLayer`: `TerrainBrushResourceId` plus `Curves` (curve keys for
  slope- and height-based auto-painting, presumably editor-only).

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

Sources: disassembly of `ecl::EntityManager::AddTerrain` (objdump on the arm64 slice) and the
terrain shaders `Terrain_VS` / `Terrain_PS4` in `Core.metallib` (`bg3x metallib Core.metallib
--extract …`, read with `llvm-dis`), 4.1.1.7631656.

- **Every terrain is drawn.** The client gives each terrain object a `VisualComponent` and a
  `TerrainObject` with no condition on the template or the object (`Flag`, `WalkOn`,
  `IsDecorative`, … don't matter). On the Nautiloid the terrains follow the sculpted deck
  meshes closely: around the imp fight `WLD_TUT_Avernus_C_Lower_A` lies within about ±0.45 m of
  the deck surface, and from straight above it is the first surface hit for 30 of 169 sample
  rays (found with the viewer's `BG3_PICK`).
- **Geometry** (`Terrain_VS`): vertices at the cell corners plus one at each cell centre, so a
  cell is four triangles around its centre. Heights come from a heightmap texture sampled at the
  vertex. The centre vertex reads the hole texture at its cell; a hole discards the whole cell.
  The viewer splits each cell along one diagonal instead (same outline, slightly different
  interior shape).
- **Layer weights** (`Terrain_PS<n>`, `n` = layer count rounded up to a multiple of 4): one RGBA
  texel per cell and per 4 layers, sampled bilinearly at terrain-local `(x, z)` / size, so texel
  centres sit at cell centres. Weights don't sum to 255 in the data (e.g. `[255, 0, 145, 0]`,
  `[0, 0, 18, 0]` on the Nautiloid).
- **Texture coordinates**: projected from the terrain-local position, choosing the projection by
  the terrain normal (triplanar, sharpened above a threshold). On level ground they are
  `(x, −z)` times a per-layer scale (presumably the brush's `Tiling`).
- **Blend**: by height. Each layer's albedo alpha is a height; with the layer's weight `w`, a
  value `w · |a|^contrast` is kept where `w + a − 1` exceeds a threshold, then scaled and offset
  by per-layer parameters. The final factors are relative to the highest layer (minus a
  transition width), plus a fallback "base layer" for the remaining weight. Which brush fields
  (`HeightScale`, `HeightOffset`, `HeightContrast`, `AlphaThreshold`) and which terrain field
  (`Visual/HeightTransition`?) feed the shader's parameters is not traced yet.

The viewer (`bg3_world::terrain_albedo`, `viewer/src/terrain.rs`) bakes a base colour texture
per 64×64-cell patch on the CPU, 16 texels per cell, from the brushes' `BaseColorMap`s at a mip
matching that density. It uses the bilinear cell-centre weights and the level-ground
projection, but **assumes** a linear blend of the weights normalised by their sum, and applies
neither the height blend, the side projections, the normal/physical maps, nor the per-layer
`ParamOverrides`. Before this the viewer coloured terrain with flat placeholder colours per
brush name, which on the Nautiloid drew a smooth pink-orange sheet through the lower deck
(easily mistaken for the Avernus backdrop).

## Open questions

- **Sub-level registry:** how does the engine know which levels make up `WLD_Main_A`? Searches
  turned up nothing: no LSF attribute in `Mods/*` names the sub-levels, `meta.lsf` has no list,
  and `Layers/*.lsx` are editor layers. The only symbol is `strSubLevelName`. Next step: find where
  level loading enumerates `Levels/` (REA).
- Triangle layout: the engine draws four triangles per cell around a centre vertex (see
  Drawing); the viewer still splits each cell along one diagonal.
- Meaning of the v8 header's two `IVec4`s, of the block data, and of the >4-layer extra section.
- The `Curves` on `TerrainBaseLayer`, and whether they matter at runtime.
- Brush shading: the height-blend parameters, the base-layer fallback, slope projections,
  normal and physical maps, detail normals (see Drawing).
