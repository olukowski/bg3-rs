# Level → mesh reference chain

Status: traced by hand for one Nautiloid object with `bg3x lsx` / `bg3x find`. Not yet implemented.

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
