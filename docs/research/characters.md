# Characters: visuals, skeleton, idle animation, equipment

Status: traced with `bg3x lsx` / `bg3x find` and implemented as pure data in
`crates/bg3-world/src/characters.rs`, rendered by `viewer/src/characters.rs`. All **54** Nautiloid
(`TUT_Avernus_C`) characters resolve to a character visual, a skeleton GR2 and an idle animation
(`cargo test --release -p bg3-world nautiloid_characters_resolve -- --ignored --nocapture`; set
`BG3_CHAR_DEBUG=1` to list every part and material). Reference build 4.1.1.7631656.

Rules marked **assumption** are inferred from the data and not yet confirmed against the
original (by RE or a runtime trace).

## Chain

```
level character (Mods/<Mod>/Levels/<Level>/Characters/_merged.lsf)
  └─ TemplateName ─► root template ─► ParentTemplateId ─► …        (first non-empty field wins)
       CharacterVisualResourceID ─► CharacterVisualBank resource
       AnimationSetResourceID   ─► AnimationSetBank resource (optional override)
       Equipment                ─► stats `new equipment` (Equipment.txt)
CharacterVisualBank resource
  ├─ BaseVisual    ─► VisualBank resource (no meshes; Template = EMPTY_VISUAL_TEMPLATE)
  │     ├─ SkeletonResource   ─► SkeletonBank ─► skeleton GR2
  │     └─ AnimationWaterfall ─► AnimationSetBank IDs (in order)
  ├─ BodySetVisual ─► VisualBank resource (the body; Slot = NakedBody on creatures)
  ├─ Slots[]       ─► {Slot, VisualResource, Bone}: head, hair, beard, clothing, wings, …
  └─ MaterialOverrides: MaterialPresets (group → MaterialPresetBank), Scalar/Vector3 parameters
AnimationSetBank resource
  └─ AnimationSet/AnimationBank/AnimationSubSets/Object {MapKey = subset GUID, FallBackSubSet}
        └─ Animation/Object {MapKey = animation type GUID, ID ─► AnimationBank resource}
AnimationBank resource: SourceFile (animation GR2), Duration, Looping, SkeletonResource, Events
```

### Level object and templates

Character objects store few fields; most come from the template chain, exactly like scenery and
lights. Fields read (`CharacterProps`): `CharacterVisualResourceID`, `AnimationSetResourceID`,
`Equipment`, `Stats`, `EquipmentRace`, `DefaultState`. Of the 54 Nautiloid characters, 22 set
their own `CharacterVisualResourceID` (one-off visuals named `<object name>_<MapKey>`), the rest
inherit it. Root templates of creatures also carry a plain `VisualTemplate`, which is the
character visual's `BaseVisual` (e.g. `IMP_Base`); it is not needed for drawing.

An empty attribute (`Equipment=""`) is treated as unset and inherited (**assumption**, consistent
with the sparse-template rule in `lighting.md`).

### Character visual

`CharacterVisualBank` lives in the same `_merged.lsf` bank files as everything else
(`[PAK]_CharacterVisuals`, `[PAK]_Creature_Presets`, per-creature `[PAK]_Body`, …).

- `BaseVisual` → a `VisualBank` resource with `SkeletonResource` and `AnimationWaterfall` children
  and no `Objects`.
- `BodySetVisual` → the body mesh visual. The parts drawn are the body set plus every slot visual
  (heads have several meshes: head, eyes, mouth interior, lashes, tear line, and an invisible
  helper).
- `Slots`: free-form slot names (`Head`, `Body`, `Hair`, `Beard`, `Private Parts`, `Wings`,
  `DragonbornTop`, …). `Bone` is empty on every Nautiloid slot; the visual's own `AttachBone`
  would apply to rigid attachments.
- `ShowEquipmentVisuals`: false on the NPC presets seen; with it false, the clothing is already in
  the slots.
- `MaterialOverrides`: per-character skin/hair/eye colours (`HemoglobinAmount`, `MelaninColor`,
  `Eyes_IrisColour`, `Cloth_Primary`, …) with an `Enabled` flag, plus `MaterialPresets`. Read, not
  yet applied (needs the character shaders).

Clothing meshes are drawn on top of the full body; vertex-colour masks on the body
(`SupportsVertexColorMask`, `VertexCut` graphs) presumably hide what clothing covers. Not modelled.

### Skeleton and skinning

The skeleton GR2 (`SkeletonBank.SourceFile`, e.g. `HUM_M_Base.GR2`) holds the body rig with
`Dummy_*` helper bones (`Dummy_R_Hand`, `Dummy_L_Hand`, `Dummy_Sheath_Hip_L`, …). Its rest pose is
the bind pose: `rest_world · InverseWorldTransform = I` for every bone of every Nautiloid skeleton.

Mesh GR2s carry their own skeletons, and their `BoneBindings` name bones that the base skeleton
lacks: face bones (`brow_l_01`, `adams_apple`), hair sockets (`Socket_Hair_*`), wing, tentacle and
hair-strand bones (59–153 per character). The viewer extends the base skeleton with each missing
bone from the mesh's skeleton, parented by name to the nearest ancestor the base skeleton has.
Body animations don't drive these bones, so they keep their rest pose relative to the parent
(the game presumably drives them with facial animation, physics and cloth).

Vertex `BoneIndices` index the mesh's `BoneBindings`, so they are remapped by name to joint
indices; inverse bind matrices come from the skeleton (`InverseWorldTransform`: row-major with
row vectors, which as a flat array is the column-major layout of the column-vector matrix).

Mirroring into Bevy (see `coordinate-space.md`): with `M = diag(1, 1, −1)`, joint locals and
animation samples become `M·T·M` (translation `z → −z`, quaternion `(−x, −y, z, w)`) and inverse
binds `M·IBM·M`, so the skinned vertex `joint · IBM · v` is the mirror of the original. Unit tests
in `viewer/src/characters.rs` check `rest · IBM = I` after mirroring, that a posed skinned vertex
is the mirror of the same pose skinned in BG3 space, and that `_L` bones stay at +X (the
character's left when it faces +Z in Bevy). Screenshots agree: an imp holds its handaxe in its
right hand, and an unrotated human faces BG3 −Z.

### Idle animation

- **Animation type ("simple name").** The character animation blueprint
  `Public/Shared/Assets/Blueprints/Characters_Common.lsbs` (an LSF "Genome" resource) has a
  variable `IDLE_DesiredStillShortName` of type `AnimationSimpleName` whose default is
  `07e81afc-95c4-452b-ac68-5cbb74710cca`. Every Nautiloid animation set maps that GUID to a
  `*_DFLT_IDLE_Still_Peace_01` clip (`*_Still_Combat_01` for the cannon turrets), so it is the
  out-of-combat standing idle. `idle_simple_name()` reads it from the blueprint at runtime.
  Related variables: `IDLE_CurrentStillShortName` (same default), `ATT_CombatShortname`.
- **Sets.** Lookup order (**assumption**): the template's `AnimationSetResourceID`, then the base
  visual's `AnimationWaterfall` in file order; first hit wins.
- **Subsets.** A set's subsets are keyed by GUID (`""` is the default `DFLT` subset). Observed
  per-weapon subsets: one-handed (`f6fa9d34-…`, `*_1HS_*` clips) and crossbow (`32bbe70b-…`,
  `*_XB_*`). Lookup follows `FallBackSubSet` and ends at the default subset (**assumption**). The
  viewer uses the default subset; which subset the game selects depends on the equipped weapon
  and isn't traced.
- The clip is the first animation in the AnimationBank's GR2; tracks bind to bones by name and
  hold parent-local transforms. Played looping from a per-character phase offset.

Unresolved: dead characters (`DeadTrall`, `*_Dead_*`, corpses) play the standing idle. Their
pose presumably comes from `DefaultState` (11 on dead thralls), Osiris, or lying/death animations
(`Public/Engine/Assets/Characters/_Anims/Engine_Default_Lying*.bin`).

### Equipment

`resolve_equipment` follows stats `Equipment` → each `add equipmentgroup`'s first `add equipment
entry` (**assumption**: the game may choose among a group's entries) → item stats (`Slot`,
`RootTemplate`) → item root template → `VisualTemplate`. On the Nautiloid this gives 28 items:
shortswords, longswords, scimitars, tridents, a greatsword, handaxes, crossbows, a shield, and
body armour whose template visual is the loot (dropped-item) mesh.

The viewer draws weapons in hand (**assumption**, the drawn/sheathed state isn't modelled): the
weapon GR2's `Dummy_Attachment` bone is placed on the character's `Dummy_R_Hand` (melee main hand,
or a ranged main weapon when there is no melee one) or `Dummy_L_Hand` (off hand). Both names occur
in the game binary and in every humanoid skeleton; weapons also have a `Dummy_Sheath` bone that
presumably meets the skeleton's `Dummy_Sheath_*` bones when sheathed. Worn armour uses per-race
equipment visuals (`EquipmentRace`, `Mods/Shared/EquipmentSettings/*.lsx`,
`VisualAttachmentMappings.lsf`); not followed yet.

## Open

- Character shaders: skin (`CHAR_Skin_*`, melanin/hemoglobin model), hair, eyes, `VertCut`
  masking, material overrides and presets. The viewer's `StandardMaterial` approximation shows
  some skin parts in the graph's default (red) texture.
- Subset selection, sheathing, and the armour equipment-visual chain.
- Facial animation and physics/cloth for the extra mesh bones.
- Death/lying poses for dead characters.
- Confirm the waterfall/override order and the fallback rule against the original
  (`ls::AnimationSetComponent` and related symbols).
