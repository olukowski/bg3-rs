# Level → mesh reference chain

Status: traced with `bg3x lsx` / `bg3x find`, implemented in `crates/bg3-world`. All 2,068 Nautiloid scenery objects resolve: 1,860 to an existing GR2, 208 to `EffectBank` (VFX).

## Where a level lives

A level such as `TUT_Avernus_C` (the Nautiloid) is split across **modules**. A later module adds to or
overrides an earlier one:

- `Mods/Gustav/Levels/TUT_Avernus_C/…`
- `Mods/GustavDev/Levels/TUT_Avernus_C/…`

Per-level directories: `Scenery/`, `Items/`, `Characters/`, `Triggers/`, `Lights/`,
`CombinedLights/`, `Decals/`, `FogVolumes/`, `Splines/` (each a `_merged.lsf`); `Terrains/`,
`LightProbes/`, `Layers/*.lsx` (layer GUIDs), `Ai/` (`aigrid.data`, `materialgrid.data`, nav portals),
`LevelTemplates/`, `WorldMap/`. Related data also lives in `Mods/<Mod>/Globals/<Level>/` and in
HLOD assets under `Generated/Public/GustavDev/Assets/HLOD`.

## Chain for a scenery object

1. **Level object.** `Scenery/_merged.lsf` → region `Templates` → `GameObjects` nodes:
   `MapKey` (instance GUID), `Name`, `Type=scenery`, `TemplateName` (root template GUID),
   `LevelName`, `Transform{Position: fvec3, RotationQuat: fvec4 (xyzw), Scale: float}`, `LayerList`,
   and optionally `HLOD` / `GroupID`.
2. **Root template.** `Public/<Module>/RootTemplates/_merged.lsf` → `Templates/GameObjects` with
   `MapKey == TemplateName`. It provides `VisualTemplate` (visual resource ID), `PhysicsTemplate`,
   and `ParentTemplateId`. **Template inheritance:** fields left empty presumably come from the
   parent. This still needs confirming.
3. **Visual resource.** Some `Public/<Module>/Content/**/[PAK]_<Name>/_merged.lsf` → region
   `VisualBank` → `Resource` with `ID == VisualTemplate`:
   - `SourceFile`: the GR2 path, e.g. `Generated/Public/Shared/Assets/…/X.GR2`.
   - `Template`: the GR2 model/mesh object name.
   - `Objects[]`: `{ObjectID: "<model>.<mesh>.<n>", MaterialID, LOD}`, one per mesh and LOD.
   - Also `Bones`, `AnimationWaterfall`, `Base`, and similar.
4. **Material.** Usually in the same bank file, region `MaterialBank` → `Resource` with `ID == MaterialID`:
   - `SourceFile`: the material shader-graph resource (e.g. `…/ENV_DetailMap_VT.lsf`).
   - `MaterialType`.
   - `ScalarParameters`, `Vector3Parameters`, … `{ParameterName, Value, BaseValue, Enabled}`.
   - `Texture2DParameters {ParameterName, ID → TextureBank}`.
   - Virtual-texture parameters pointing into `VirtualTextureBank`.

   Many environment materials (`*_VT`) take their main maps from **virtual textures** (Granite), not
   from plain DDS files.
5. **Other banks** in the same file: `TextureBank` (texture ID → DDS `SourceFile`), `PhysicsBank`,
   `VirtualTextureBank`.

## Implications for the viewer

- To resolve IDs we need a global **resource-bank index** (visual, material, texture, physics, VT)
  built by scanning `Public/**/Content/**/_merged.lsf` (about 3.2k bank files). One full scan takes
  about 20 s single-threaded, so the index should be cached (in `.cache/`, keyed by game version) and
  parallelised.
- First milestone rendering can use the LOD0 mesh with flat or debug shading. Proper materials need
  the shader-graph resources, DDS textures and, for many surfaces, virtual textures.

## What the game doesn't draw

Status: found with `cargo run --release -p bg3x --example object_survey` (every LOD0 mesh of a
level with its template, GR2 flags, material and size) and screenshots with categories hidden;
the rules come from the macOS binary (4.1.1.7631656). Before this, the viewer drew 137
Nautiloid meshes the game doesn't (102 blockers, 35 cloth simulation sheets): grey slabs, wedges and 48 m discs cutting through the deck
and walls.

**Scenery blockers.** `ecl::EntityManager::LoadEntity` builds the client scenery's flags from its
template; `IsBlocker` is bit `0x200`. With it set, the scenery gets no `VisualLoadDesciption` /
`VisualComponent` at all, only a `LocalBoundComponent` taken from the visual resource's bounds:
it blocks (movement, camera, pointer) but is never drawn. `IsBlocker` is a template field like
the others (value + is-set pairs in `eoc::SceneryTemplate::Visit`), so a level object's own
value wins, else the first template up the `ParentTemplateId` chain that sets it (the same
inheritance assumption as for `VisualTemplate`, consistent with the data: e.g.
`CAM_InvisibleBlocker_Nautiloid_Disc_Slider_A` sets nothing and inherits `true` from
`CAM_InvisibleSlider_2x2_1H_A`). Game-wide only helper templates resolve to `true` (`PUZ_*`
invisible blockers, `CAM_*` invisible camera sliders, `FADE_*` fade helpers); their meshes use
`Engine_Default_Mesh_Lit_Opaque` or `Helper_Invisible*`. 102 Nautiloid scenery objects. The rule
is scenery-only (the code path is the `ecl::Scenery` one); items set `IsBlocker` too (the
Nautiloid's rigged tentacles, to `false`) and keep their visuals.
`bg3_world::LevelObject::draws_visual` implements it.

**Meshes without a renderable model.** `gr::FactoryGr2::Parse` reads each mesh's
`UserMeshProperties.Flags`: a MeshProxy (`Flags[0] & 0x01`) becomes an `rf::ModelProxy` and an
Occluder (`Flags[0] & 0x80`) only an `rf::OcclusionModel`; neither is drawn. A cloth mesh whose
`Flags[2] & 0x300 == 0x100` (ClothPhysics) is the flat simulation sheet: its renderable object is
created without its drawn bit (`ls::RenderableObjectTemplate::CreateInstance`), the visible
cloth being a separate mesh of the same visual (`…_Cloth_Mesh` next to `…_Mesh` on the
Nautiloid's hanging pods, flesh fleeces and hanging flesh: 7 visuals, 35 placed objects). See `docs/formats/gr2.md`;
`bg3_gr2::Mesh::is_drawn`.

**Story-controlled objects.** Items the story takes off stage (`SetOnStage(…, 0)`, e.g. the
helper cube `S_TUT_Lab_BrainReader`) are hidden by `viewer --play` as the sim says.

**Effects.** Objects whose visual is an `EffectBank` resource are VFX, not meshes; the viewer
doesn't play them. It used to draw a 0.2 m box for each (and for objects whose visual didn't
resolve); those boxes are now off unless `--placeholders` is given.

Open: one Nautiloid destruction mesh
(`WALL_MF_Nautiloid_Interior_A_Mid_D_Piece_45_E_Membrane_Destruction_Mesh`, a 25 m bar) carries
the engine default material with no flag that hides it; the zero-size door-handle and anchor
dummies do too. Whether the game draws them (destruction meshes may only show once broken)
is not established. Decals, fog volumes and triggers are not level meshes (`OBJECT_DIRS` is
`Scenery`, `Items`, `Characters`), so none of them were among the shapes; decals are projected
onto the level by the material shader (`decals.md`).

**Not missing geometry: the lower deck's "gaps".** Close-ups of the imp fight showed the deck as
straight-cut slabs with a pink-orange surface below. Neither was level data: the cuts were the
`--frame` screenshot camera's near plane, pulled up to the subject and slicing the floor in front
of it (now capped where the view meets the subject's floor level), and the pink-orange surface
was the lower deck's terrain (`WLD_TUT_Avernus_C_Lower_A`) in the viewer's old flat placeholder
colours, now drawn with its brush textures (`docs/research/terrain.md`, Drawing). The deck pieces
there (`WALL_MF_Nautiloid_Interior_A_Bottom_L_Piece_22n5_E`) are complete LOD0 meshes.
