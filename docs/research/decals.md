# Decals

Status: data traced with `bg3x lsx`; engine behaviour from the macOS binary (4.1.1.7631656,
targeted `llvm-objdump` of the functions named below); shading from the decal graph's compiled
shaders in `Materials.metallib` (`bg3x metallib --extract …`, `llvm-dis`; disassembly kept in
`.cache/`). Implemented in `crates/bg3-world/src/decals.rs` (data, projection math, draw order)
and `viewer/src/decals.rs` + `viewer/src/shaders/decals.wesl` (rendering).

## Data

A level's decals are level objects with `Type=decal` in
`Mods/<Module>/Levels/<Level>/Decals/_merged.lsf` (region `Templates`, `GameObjects` nodes), merged
across modules by `MapKey` like scenery. Each has `Transform` (position, rotation quaternion,
uniform scale), `TemplateName`, `LayerList` (editor layers) and, optionally, its own values for the
template fields. The Nautiloid (`TUT_Avernus_C`) has 108: 59 in `Gustav`, 49 in `GustavDev`.

The root templates (`Type=decal`, mostly in `Public/Shared/RootTemplates`) carry the decal fields.
`ls::DecalTemplate::Visit` reads them as value/is-set pairs, so an unset field inherits (same
sparse-field rule as other templates), with these defaults:

| Field | Meaning | Default |
|---|---|---|
| `Material` | `MaterialBank` resource ID | — |
| `Tiling` (fvec2) | texture scale | 1, 1 |
| `offset` (fvec2, lower case) | texture offset | 0, 0 |
| `Dimensions` (fvec3) | box size along local X, Y, Z (m) | 1 |
| `NormalBlendingFactor` | see Shading | 0 |
| `AngleCutoff` | see Shading | 0 |
| `Layer` (uint32) | draw-order layer | 0 |
| `Opacity` | overall opacity | 1 |

Other attributes on decal templates (`CameraOffset`, `DecalType`, `PhysicsTemplate`, …) are not
read by `ls::DecalTemplate::Visit`. Level instances commonly override `Dimensions` and `Layer`
(e.g. `DCL_MF_Nautiloid_Interior_Destruction_B`: template 4×4×8, layer 5; instances 4×1×8).

`ls::DecalObject::Render` builds the decal's world matrix from the rotation, the position
(camera-relative) and a per-axis scale; the shader treats the result as a unit box, so the box
spans `Dimensions` metres. **Assumption**: the uniform `Scale` multiplies the dimensions (every
Nautiloid decal has scale 1).

### Materials

12 of the Nautiloid's 13 decal materials use the engine graph
`Public/Engine/Assets/Materials/Engine_Default_Decal_Deferred.lsf` with three textures
(`basecolor` `_BMA`, `normalmap` `_NM`, `physicalmap` `_PM`, in the usual channel layouts of
`materials.md`) and `Reflectance`. Several use engine default textures for the normal and
physical maps. The 13th, used by the 20 `VFX_Environment_Crashsite_Fire_Decal` objects, is the
animated VFX graph `VFX_Decal_Deferred_AlphaBlend_Base_Burning` (panning triplanar coverage,
colour ramps, glow); it is not ported. Other decal graphs exist (`…_Decal_Deferred_VT`,
`…_WSTile`, `Base_Decal_GM_FM_AlphaTest`, `ATM_Decal_Caustic`) and aren't used here.

### Who receives decals

The object G-buffer shaders take an engine parameter `_ObjectReceiveDecal` and write it as bit 8
of the G-buffer flags. It comes from the object's `RecieveDecal` (sic) field, a template field
read by `ls::GameObjectTemplate::Visit` with **default true** (with `AllowReceiveDecalWhenAnimated`,
default false, next to it). On the Nautiloid, 1,940 scenery objects leave it unset (the decks and
walls: they receive), 79 inherit false and 48 set false themselves (rubble, emissive machine
parts, VFX helpers); items mostly inherit false. The terrain shaders (`Terrain_PS*`,
`Core.metallib`) set bit 8 unconditionally. Graphs named `…_No_Decals` presumably don't.
Characters are drawn without it in the viewer (their templates mostly set false; animated
objects need `AllowReceiveDecalWhenAnimated`).

## Engine

- `ls::DecalObject::PrepareDraw` fills the decal constant buffer: `WorldOffsetMatrix` and
  `InverseWorldOffsetMatrix` (camera-relative), `NormalMatrix`, `DecalTilingAndOffset`
  (`Tiling`, `offset`) and `_DecalDeferredProperties` = (`NormalBlendingFactor`, **60**,
  `AngleCutoff`, `Opacity`). The 60 is a constant: the fade distance below.
- `ls::CullingHelpers::SortDecalObjects` orders the decals before drawing: by `Layer`, lower
  first (so higher layers end on top), then by the radius of the world bounds, larger first
  (small decals over big ones); ties fall back to pointer order.
- The deferred decal sub-stages (`ls::DeferredDecalRenderSubStage`,
  `ls::DeferredGameDecalRenderSubStage`) draw the boxes after the G-buffer pass; there is also an
  emissive game-decal stage, and effect decals (`ls::FxDecal`).
- Surfaces (blood, water, fire on the ground) are drawn by `ecl::SurfaceDeferredRenderer`, which
  creates its own unit-box decal objects (`InitializeDecalObject`). They are a separate system,
  driven by the surface grid, not by level decal objects.

## Shading (`Engine_Default_Decal_Deferred`, `DEF` pass)

The pass reads the linear depth and a copy of the receivers' normals (`_DecalReceivedNormal`, zero
where nothing receives decals: those pixels are discarded) and writes G-buffer targets 0–2:

1. **Projection.** Reconstruct the pixel's camera-relative world position from depth, transform it
   into the decal's unit box, discard outside ±½ on any axis. Texture coordinates: u = x + ½,
   v = ½ − z, then × `Tiling` + `offset`. The box projects along its local −Y.
2. **Distance fade.** With d the camera's horizontal (x/z) distance to the pixel and F = 60:
   0 for d ≥ F, else a smoothstep of (F − d) / (F + 1.5 − d) (≈1 up to ~50 m).
3. **Angle fade.** If `AngleCutoff` > 0: c = saturate(receiver normal · decal up) − cutoff;
   discard below 0, fade = c / 0.17. With cutoff 0 there is no angle test at all (the decal
   also lands on walls inside the box, stretched).
4. **Alpha** = min(base colour alpha, saturated angle fade); discarded at 0.
5. **Normal.** The decal normal map (X in A, Y in G, Z in B) is decoded into the box frame (X
   along local X, out-of-plane along local Y, Y along local −Z). The shader doesn't write it
   directly: it forms the rotation that takes the decal's up axis to that normal — Euler angles
   π/2 · (n × −up), composed about x, then y, then z — and applies it to a base normal,
   mix(flat normal from the depth derivatives, receiver's normal, `NormalBlendingFactor`). The
   result is mixed with the receiver's normal by alpha, re-encoded, and blended into the normal
   target with the distance fade as its alpha.
6. **Material.** Base colour = saturate(`basecolor`.rgb); target 2 = (roughness = physical G,
   saturate(`Reflectance`), metalness = physical R). Both are blended with
   a = saturate(`Opacity` · alpha · distance fade). Occlusion (physical B), the shading model and
   flags, and target 3 are left alone.

## In the viewer

The viewer has no G-buffer, so the level material shader does the decal pass per pixel before it
lights the surface (`shaders/level.wesl` → `apply_decals` in `shaders/decals.wesl`), on the same
inputs the game's G-buffer holds (base colour, normal, roughness, reflectance, metalness):

- `bg3_world::decals::load_decals` resolves the level's decals over their templates;
  `sort_draw_order` applies the game's order.
- `viewer/src/decals.rs` keeps the decals on the default graph, packs a record per decal
  (game-space world → box matrix, box axes, tiling, `_DecalDeferredProperties`, texture layer,
  reflectance) into a storage buffer and builds a world-space grid (4 m cells over the decals'
  bounds; each cell lists the decals overlapping it, in draw order). Each map type becomes one
  texture array with a layer per decal material, resampled to 1024² (base colour, sRGB) and 512²
  (normal and physical maps, stored with the game's channels) with full mip chains;
  `BG3_DECAL_SIZE` changes the base colour size. 88 decals, 12 materials, ~0.15 s on the
  Nautiloid.
- Level meshes get `MeshTag` bit 1 when their object receives decals
  (`RootTemplates::receives_decals`, with the instance's own `RecieveDecal` from
  `Level::receive_decal`). Alpha-blended materials never receive (they are forward-drawn in the
  game, outside the G-buffer). Terrain tiles are switched to `TerrainDecalMaterial` (Bevy's
  standard material plus the same step, `shaders/terrain_decals.wesl`); they keep Bevy's lighting.
- The shader blends the decals of the pixel's cell in order, exactly as above, in game space (z
  mirrored). `BG3_NO_DECALS=1` turns decals off; `BG3_DECAL_DEBUG=1` lists the decals and paints a
  checker of each box's local x/z quarters wherever it projects (check of placement and
  orientation).

Exact: the data and inheritance, box size and projection, uv convention, draw order, distance
and angle fades, alpha, the normal rotation, and which material channels are written.
Approximated:

- **Assumption**: the G-buffer blend states are standard alpha blending with each target's alpha
  (normal: distance fade; base colour and target 2: a). We blend linear colour; the game's
  albedo target encoding (sRGB or not) isn't established.
- Normals are blended as vectors and renormalised, not in the game's encoded (stereographic)
  form; the flat normal comes from the world position's derivatives (the game's from the depth's).
- The receiver normal each decal starts from is the surface's own (pre-decal) normal, as with the
  game's `_DecalReceivedNormal` copy.
- Texture gradients come from the world position's derivatives (no mip blow-ups at box edges as
  with implicit derivatives); textures are resampled into arrays.
- Not drawn: the fire VFX decals (animated graph), other decal graphs, emissive and effect decals,
  surfaces (`ecl::SurfaceDeferredRenderer`: the large blood pools around the imp fight in the
  reference captures are most likely these or combat blood, not level decals), editor layer
  visibility (`LayerList`, which the viewer doesn't apply to any level object), and decals on
  characters.

Verification: `cargo test -p bg3-world decals` (projection, uv, fade, draw order, inheritance),
`viewer` unit tests (grid lookup, record packing, resampling), the install test
`nautiloid_decals_resolve`, and `BG3_DECAL_DEBUG=1` shots of the imp deck
(`--camera=-42,14.5,-409 --look-at=-46.6,3.5,-402.8`): the checker of
`DCL_MF_Nautiloid_Interior_Destruction_B_010`/`_013` sits on the deck and terrain inside their
1 m-tall boxes.
