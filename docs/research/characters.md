# Characters: visuals, skeleton, animation, equipment

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

Unresolved: initially dead scenery characters (`DeadTrall`, `*_Dead_*`, corpses) play the
standing idle. Their
pose presumably comes from `DefaultState` (11 on dead thralls), Osiris, or lying/death animations
(`Public/Engine/Assets/Characters/_Anims/Engine_Default_Lying*.bin`).

### Combat clips and playback

The viewer reads `Public/Shared/Animation/ShortNames.lsx` at startup: each `ShortName` has a
`Name` and `UUID`. `CombatAnimation::simple_name()` names the requested type; the UUID comes
from the install, not an embedded extracted table. `ResolvedCharacter::animation()` uses
exactly the idle/run chain: the effective character/template `AnimationSetResourceID` first,
then its base visual's shared `AnimationWaterfall` in file order. Each set checks the default
subset. `AnimationSetResource::find()` also supports explicit subsets and their
`FallBackSubSet` links, ending at the default subset, but the viewer does not yet select
weapon subsets. A missing type falls through to the next set; a type absent from the whole
chain stays unavailable. No additional rig fallback is invented outside the waterfall. The order remains a
**data-inferred assumption**, not engine-verified behaviour. Related original symbol:
`ls::AnimationSetSystem::Update` (4.1.1.7631656).

Survey commands (output must stay in `.cache/`): `bg3x find IMP_Rig` found no exact attribute
value, because that name is part of GR2 paths, not the resource name. `bg3x find IMP_Base -p
Public/Shared/Content/Assets/Characters/Creatures/Imp` locates the resource. Inspect with
`bg3x lsx - Public/Shared/Content/Assets/Characters/Creatures/Imp/[PAK]_Body/_merged.lsf`,
likewise `.../Humans/[PAK]_Male_Body/_merged.lsf`, and the ShortNames resource above.
The imp skeleton/set resources are named `IMP_Base`; their animation tracks use `IMP_Rig`.

Requested simple names and observed default-subset mappings in reference build 4.1.1.7631656
(human timings below are HUM_M):

- Melee: `CMBT_Melee_RHand_01_Attack`, mapping to the corresponding
  `HUM_M_Rig_DFLT_…` (0.433 s) and `IMP_Rig_DFLT_…` (0.667 s) clips.
- Ranged: `CMBT_Range_RHand_01_Attack`, mapping to
  `HUM_M_Rig_1HS_CMBT_Range_RHand_01_Attack` (1.033 s). **Missing in the imp default subset**;
  its crossbow subset (`32bbe70b-…`) has `IMP_Rig_XB_CMBT_Range_RHand_01_Attack` (0.667 s).
  We deliberately retain the existing default-subset policy; an imp ranged attack uses
  the 0.7 s fallback pause.
- Cast: `SPL_Somatic_Target_01_Cast`, mapping to the corresponding human DFLT clip (1.5 s)
  and `IMP_Rig_DFLT_SPL_Somatic_WeaponAttack_01_Cast` (1.167 s).
- Hit: `REAC_Hit_F_Combat_01`, mapping to the corresponding human DFLT clip (1.633 s)
  and `IMP_Rig_DFLT_REAC_Hit_F_01` (1.233 s).
- Death: `DIE_Physical_Stationary_Front_01`, mapping to the corresponding human DFLT clip
  (2.0 s) and `IMP_Rig_DFLT_DIE_Physical_Stationary_F_01` (2.167 s).
- Dead pose: `DIE_Generic_Deathpose_01`, mapping to corresponding human and imp DFLT clips
  (0.467 s / 0.167 s). This is a short pose, not a fall animation.
- Downed loop: `STAT_Downed_Peace_01_Loop`, mapping to the corresponding human DFLT clip
  (15.958 s) and `IMP_Rig_DFLT_STAT_Weakened_Combat_01_Loop` (4.233 s).
  The imp mapping is the install's choice, even though its name says weakened.

The female human base visual lists its female sets first, then the male shared sets. Its
melee/cast/hit/death/dead/downed clips resolve to HUM_F tracks; ranged falls through to the
HUM_M ranged clip above. The ignored Nautiloid resolution test confirms all seven requested
types resolve on the ordinary human characters and the imp ranged type remains missing.
The separate crippled rig (`S_FirstFight_Imp_Melee_01`, `IMP_Crippled_Rig`) resolves melee,
death and the shared dead pose, but not ranged, cast, hit or downed. It is not silently
substituted with the regular imp rig. Viewer loading confirmed these results after GR2
decoding too (`BG3_CHAR_DEBUG=1`); Lae’zel resolves Gith melee, female human cast/hit/death/
downed, and the shared male human ranged clip.

Playback is a **viewer approximation**. The game thread classifies `SpellCast` from the
install's spell flags: `IsSpell` uses cast; otherwise `IsMelee` uses melee, and a projectile
with `IsAttack` uses ranged. Others use cast. Attack and cast clips play once, then return
to idle; damage plays a hit reaction. Death plays once with non-looping sampling and holds
the final frame, so it cannot interpolate across the loop seam or restart the fall. Only
missing death clips use the old transform topple. Downed characters use the downed loop
(or the dead pose if only that resolves); healing returns them to idle. Locomotion cannot
replace one-shots or incapacitated poses. Bones not driven by a new clip reset to rest.

AI results already contain the completed move and its subsequent events. The viewer queues
the walk followed by those events; each attack/hit/death waits for its decoded GR2 duration
before presenting the next event or requesting another AI turn. Missing attacks retain the
0.7 s pause. Logs follow presentation order, although HP/action availability snapshots still
represent the current simulation state. This is not the game's animation-event synchronization:
no weapon-aware subsets, preparation/recovery chain, blending, directional reactions, or
spell-specific animation blueprint selection is implemented. Initial scenery corpses still
need their `DefaultState` interpreted.

The combat camera pans for 0.5 s when a presented turn starts (also when an AI actor in a
shared turn begins its move). It preserves orientation and distance to the actor-height
look-at plane, then follows that actor's movement. WASD/QE or right-mouse look cancels that turn's pan; the next turn enables
another pan. `F` toggles following. This is viewer camera policy, not a port of the original
camera controller; out-of-combat fly-camera behaviour is unchanged.

Synthetic tests cover set override precedence, shared-default fallback, missing types,
subset cycles/broken links, and runtime name decoding. Viewer timing tests cover one-shot
completion with frame overshoot, return to idle, held death frames, downed loops, revival,
and zero-duration clips; camera tests cover pan endpoints and turn reset after interruption.
The ignored `nautiloid_characters_resolve` test checks the names against the install and
prints each character's resolved clips with `BG3_CHAR_DEBUG=1`.

#### Verification (2026-10-21)

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace` all exited 0 (292 tests including doctests passed; 40 ignored).
The ignored `nautiloid_characters_resolve` install test also exited 0. No runtime trace of the
original game's animation controller was captured; this verifies resolution and our playback,
not 1:1 clip selection or transition timing.

Captured and inspected with `--play --combat-test --combat-autoplay --shot … --shot-after N`:

- `.cache/shots/combat-follow-3s.png`: avatar melee pose, avatar near the centre.
- `.cache/shots/combat-follow-6s.png`: camera centred on Lae’zel after her ranged attack.
- `.cache/shots/combat-follow-9s.png`: camera has moved to the next acting imp.
- `.cache/shots/combat-follow-12s.png`: imp attacking the avatar, wings raised.
- `.cache/shots/combat-hit-5s.png`, `combat-death-6s.png`, `combat-death-7s.png` in the same
  directory: alternate angle showing the first imp recoil, start its death fall, then lie down.
  These add `--camera=-53,10,-410 --look-at=-46,4,-403` (BG3 coordinates).
- `.cache/shots/combat-next-turn-8s.png`: death playback has completed and the camera
  centres on the next imp; the corpse has left the frame.

Per-shot logs remain beside the PNGs and record the played clips and decoded durations.
The camera does not avoid geometry: a much higher/wider trial placement was occluded by the
ship. Cast/downed clip resolution and playback timing are covered by the install/unit checks;
the short autoplay runs did not exercise those states visually. `F` and manual cancellation
have logic tests, but were not driven by automated GUI input. Existing skin/material and
level-rendering limitations remain visible in the shots.

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
- Initial death/lying poses for scenery corpses (combat deaths now animate).
- Confirm the waterfall/override order and the fallback rule against the original
  (`ls::AnimationSetComponent` and related symbols).
