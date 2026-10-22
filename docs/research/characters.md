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
  `Eyes_IrisColour`, `Cloth_Primary`, …) with an `Enabled` flag, plus `MaterialPresets`.
  Applied by the character shaders; layering and scope in `character-shaders.md`.

Clothing meshes are drawn on top of the body. Wearable visuals name the partial body cuts in
`VertexColorMaskSlots`; resolution exposes these requests (see Equipment below) and the viewer
applies them as the game's vertex shaders do (`character-shaders.md`, "Body cut masks").

### Cloth proxies

Visuals with simulated cloth carry a `ClothProxyMapping` (read by
`ls::VisualResource::ClothInfo::Visit`): each `MapKey` is the ObjectID of a coarse cloth
simulation mesh (`*_Cloth_Mesh`, ~200 vertices, material `Engine_Default_Mesh_Lit_Opaque`), and
its `MapValue` names the high-resolution meshes it drives, with a `ClosestVertices` buffer.
The GR2 marks the same meshes with a cloth proxy type: `UserMeshProperties.Flags[2]` bits 8–9
(value `0x100`), the field `ls.fs.Gr2.GetClothProxyType` returns (disassembly,
4.1.1.7631656). In the Githyanki female armour bank, MapKeys and the GR2-flagged meshes are
the same set (capes included). The proxies are listed in `Objects` like render meshes, so
drawing every LOD0 object put a flat grey sheet over Lae'zel's pteruges. Resolution marks them
(`MeshObject::cloth_proxy`) and `VisualResource::drawn_lod0_objects()` skips them.
**Assumption:** the engine never draws a proxy (not traced at runtime; the reference captures
show no proxy surface, and the high-resolution meshes cover the same area). Cloth simulation
itself is not reproduced: the high-resolution meshes keep their skinning.

### Skeleton and skinning

The skeleton GR2 (`SkeletonBank.SourceFile`, e.g. `HUM_M_Base.GR2`) holds the body rig with
`Dummy_*` helper bones (`Dummy_R_Hand`, `Dummy_L_Hand`, `Dummy_Sheath_Hip_L`, …). Its rest pose is
the bind pose: `rest_world · InverseWorldTransform = I` for every bone of every Nautiloid skeleton.

Mesh GR2s carry their own skeletons, and their `BoneBindings` name bones that the base skeleton
lacks: face bones (`brow_l_01`, `adams_apple`), hair sockets (`Socket_Hair_*`), wing, tentacle and
hair-strand bones (59–153 per character). The viewer extends the base skeleton with each missing
bone from the mesh's skeleton, parented by name to the nearest ancestor the base skeleton has,
and placed by the bone's inverse bind relative to that parent (`rest_world · IBM = I`), not by
the foreign rig's local rest: the rigs differ (Lae'zel's hair rig has `Head_M` 9 cm above her
body rig's), and the local rest buried her hair inside her head. **Assumption:** the engine
keeps attached meshes in their authored bind pose. Body animations don't drive these bones, so they keep their rest pose relative to the parent
(the game presumably drives them with facial animation, physics and cloth).

Vertex `BoneIndices` index the mesh's `BoneBindings`, so they are remapped by name to joint
indices; inverse bind matrices come from the skeleton (`InverseWorldTransform`: row-major with
row vectors, which as a flat array is the column-major layout of the column-vector matrix).

**Rigid meshes.** Some meshes have no `BoneWeights`/`BoneIndices` vertex components at all, only
`BoneBindings`: on the Nautiloid, the separate `*_Ears_Mesh` of the head visuals (githyanki,
elf, half-elf, human, half-orc, tiefling, cambion and halfling heads seen), the imp, tiefling
and cambion horns, Minsc's earrings, Shadowheart's circlet, leather caps (`HUM_*_ARM_Headwear_*`), Gale's Mystra
piece (all one binding, `Head_M`), and Wyll's eye and eye stone (`eye_l`, `eye_r`). The game
loader asks Granny whether each mesh is rigid (`gr::FactoryGr2::Parse` calls `GrannyMeshIsRigid`
and keeps the result on the renderable template), and the mesh binding's pose update builds one
composite matrix (`world · InverseWorldTransform`) per binding bone
(`gr::GrannyMeshBinding::UpdatePose` → `GrannyBuildIndexedCompositeBufferTransposed`). A rigid
mesh follows its first binding, as in Granny's rigid-mesh convention, so the viewer binds every
vertex to that bone with weight 1 (`skinned_mesh`). Before this, rigid meshes got no joint
attributes and were drawn as children of the character root in the rest pose: ears and horns
floated in place while the head animated. Checked with `--frame Lae --frame-mode face
--frame-yaw 90` (ear back on the head mid-idle) and the install test
`characters::tests::laezel_ears_follow_the_head`.

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
  `*_XB_*`). Lookup follows `FallBackSubSet` and ends at the default subset (**assumption**).
  Sheathed characters use the default subset; drawn weapons select a weapon subset from the
  equipped weapon types (see "Drawn and sheathed weapons" below).
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
`FallBackSubSet` links, ending at the default subset; `ResolvedCharacter::animation_in_subset`
asks for a weapon subset in every set of the chain, in the same order. A missing type falls through to the next set; a type absent from the whole
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
no sheathe/unsheathe clips, preparation/recovery chain, blending, directional reactions, or
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

Surveyed directly from the installed 4.1.1.7631656 data with `bg3x lsx`, `bg3x find`,
`bg3x stat` and `bg3x cat`; no reverse-engineering tools or original-game runtime tracing.
Local XML and search output is under `.cache/equipment/`. The implementation reads the
same resources at runtime; no extracted tables are embedded or committed.

#### From starting items to worn meshes

1. A level/global character's `Equipment` attribute inherits through its root-template
   `ParentTemplateId` chain. For `S_Player_Laezel`, inspect
   `Mods/Gustav/Globals/WLD_Main_A/Characters/_merged.lsf`: `TemplateName` points to
   `ORIGIN_Laezel` in `Public/Gustav/RootTemplates/_merged.lsf`, and `Equipment` is
   `EQ_Laezel`. Its parent `Githyanki_Female_Player`, then `BASE_Githyanki_Female` in
   `Public/Shared/RootTemplates/_merged.lsf`, supplies `EquipmentRace`.
2. `Public/<Module>/Stats/Generated/Equipment.txt` defines `new equipment`,
   `add initialweaponset`, and groups of `add equipment entry` item stats names.
   `EQ_Laezel` contains `ARM_HalfPlate_Body_Githyanki` and `ARM_Boots_Metal_Githyanki`,
   alongside weapons, supplies, underwear and camp clothes. For the created avatar the
   viewer uses `Sim::combat.created_characters[avatar].equipment`, the actual public
   `bg3_sim::cc::CreatedCharacter` result, including race/class equipment and selected
   items. It does not infer the avatar's class from the body template.
3. Item stats inherit via `using`: their effective `Slot` selects the equipment slot,
   and `RootTemplate` selects an item in `Public/<Module>/RootTemplates/_merged.lsf`.
   `ARM_HalfPlate_Body_Githyanki` is a `Breast` item whose template is
   `ARM_HalfPlate_Githyanki`. `VisualTemplate` on this template is the **dropped** visual;
   drawing that on the character would be wrong.
4. The item template's child `Equipment` supplies worn appearance:
   `Visuals/Object/@MapKey` is an **equipment-race GUID**, and each of its child
   `MapValue/@Object` values is a `VisualBank` resource ID. Keep every value, in order.
   The separate child `Slot/@Object` names a visual slot such as `Body` or `Footwear`.
   `VisualSet/MaterialOverrides` supplies item colour/material overrides; these are
   exposed on `EquippedItem` for shader work, not applied by this change.
5. The character's inherited `EquipmentRace` already identifies race **and body type**.
   `Mods/Shared/EquipmentSettings/EquipmentRaces.lsx` names Human Male, Human Female,
   strong variants, Githyanki Female, Gnome Male, etc.; `SharedDev` adds more definitions.
   Each `EquipmentRace` uses `Guid`, optional `DefaultParent`, and `CharacterVisual`
   (the associated base appearance, not an item visual). Modules load in the normal
   override order. For a requested race, consult the item's direct visual entry first;
   otherwise follow `Equipment/ParentRace/Object` (`MapKey` → `MapValue`) if explicitly
   present, or `EquipmentRace/@DefaultParent`. For example Elf/Drow Female and
   Githyanki Female have Human Female as their default parent. Loops and missing links
   terminate safely. An explicit empty visual entry is kept empty. **Assumption:** a
   null per-item parent stops fallback; the precedence and sparse per-race template
   merge are inferred from the schema, not verified against engine execution.
6. A worn `VisualBank/Resource` supplies `SourceFile` (GR2), `Slot`, `AttachBone`, and
   `Objects` (`ObjectID`, `MaterialID`, `LOD`). The viewer draws all LOD-0 mesh objects.
   The chain shirt's Human Male entry has trousers and torso visuals
   (`HUM_M_ARM_ChainShirt_A_Pants`, `HUM_M_ARM_ChainShirt_A_0_Body`); the torso itself
   includes solid and cloth meshes. Human scale mail likewise has body and trousers.
   Lae'zel's Githyanki Female entry resolves
   `GTY_F_ARM_Laezel_Githyanki_HalfPlate_A_Body`, with separate gith metal boots.
   These are race-fitting skinned meshes, bound by mesh `BoneBindings` names to the
   character skeleton. Extra cloth bones extend that skeleton just like head/hair bones;
   cloth simulation remains absent.
7. Replacement operates on visual slots, not item stats slot names:
   `Mods/Shared/EquipmentSettings/EquipmentSlots.lsx` maps `Breast` → `Body`,
   `Boots` → `Footwear`, `Helmet` → `Headwear`, etc. The item `Equipment/Slot` and
   selected visuals' `Slot` identify preset parts to replace; settings are used when
   the item omits its visual slot. Every part of a multipart item survives replacement.
   `ShowEquipmentVisuals=false` preserves the preset and suppresses equipment rendering,
   including rigid hand items. The naked `BodySet` remains for partial cuts.
   **Assumption:** same-slot preset replacement and adventuring-set visibility; the viewer
   excludes `VanityBody`/`VanityBoots` because it has no camp-outfit toggle. Helmet hair
   variants (`LongHair`, `ShortHair`, etc.) are not yet selected.

#### Partial cuts and attachments

No `HideSlots` field was found on the inspected armour templates. Instead each worn
visual has repeatable `VertexColorMaskSlots/@Object`, with names such as `Torso`,
`Thighs`, `Private_Parts`, `Underwear_Bra`, and `ModestyLeaf`. The definitions live in
`Mods/Shared/EquipmentSettings/VisualSlots.lsx` under
`VisualSetSlotsSystem/NakedBodySystem/Object`: `MapKey` is the name, `MapValue` is a
vertex-colour byte identifier. `SupportsVertexColorMask` is a visual capability.
These are identifiers, not a guessed bitmask or whole-body hide flag.

`EquippedItem::body_masks` exposes each item's requests; after `apply_equipment`,
`ResolvedCharacter::body_masks` is the deduplicated union from active preset/equipment
parts, each with its name and optional byte value. Unknown names stay visible in the
API as `None`. This is the shader integration hook: cut only matching vertex-colour
regions of compatible underlying meshes. No whole naked-body deletion is substituted
for partial cuts. The channel (vertex colour green, byte → texel of a 16×16 mask) and the
comparison are established in `character-shaders.md`; which of a character's visuals share
the mask remains an **assumption** (all supporting ones).

`VisualAttachmentMappings.lsf` is a separate visual-ID → list-of-visual-IDs table:
`VisualSetSlotsSystem/AttachmentsMappings/Object/@MapKey` → child `MapValue/@Object`.
For example its Cambion female naked-body key maps to a wing visual (confirmed with
`bg3x find` in the creature's Female_Body/Female_Wings banks). It is not the
race-selection map. None of the chain-shirt, scale-mail or gith half-plate visuals
surveyed needs this table. It is loaded into `EquipmentSettings::attachments`, but
automatic expansion is not implemented: how it interacts with explicit character
slots still needs observation. This limitation does not block the tested wearables.

Rigid visuals outside the weapon slots use their `AttachBone`. Visuals in a weapon slot
(shields included) are placed by their equipment type, sheathed or drawn: see "Drawn and
sheathed weapons" below. Related symbols found by grepping the supplied symbol index:
`eoc::EquipmentData::Visit`, `ResolveAppliedRace`, `GetVisualResources` (4.1.1.7631656).
These symbol references identify the original subsystem; they do not claim its
implementation was inspected.

#### Equipment groups

The surveyed Shared, SharedDev and Gustav `Equipment.txt` files contain respectively
1,187, 151 and 180 groups, all with a single `add equipment entry "name"` and no
condition or weight arguments. This includes Lae'zel and the CC cleric/fighter sets.
Thus there is no alternative to choose for these subjects. The resolver keeps the
first entry for a hypothetical multi-entry group, explicitly an **assumption**;
the data inspected does not establish whether such entries mean random choices or
some other policy. Synthetic tests retain this assumption without calling it verified.

#### Verification

Synthetic tests cover multipart race entries, default and item-parent fallback,
explicit empty entries, null/cyclic/missing parents, sparse item-template inheritance,
preset-slot replacement, equipment visibility, mask metadata and actual item-list
resolution without any dropped visual. The ignored install test
`worn_armour_resolves_laezel_and_human_fighter` checks Lae'zel's global/template race
and the CC Human Male player template (`Humans_Male_Player_Dev`) with `EQP_CC_Fighter`, requiring real worn `Breast`
visuals and existing GR2s for both.

Verified on 2026-10-21: `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
(333 passed including doctests, 48 ignored), and
`cargo test --release -p bg3-world worn_armour_resolves_laezel_and_human_fighter -- --ignored --nocapture`
all exited 0 against the final source. Logs are under `.cache/equipment/`.
An initial viewer/test build overlapped a source edit and saw an older dependency
layout; both were rebuilt successfully after the edit. This verifies our data
resolution and rendering, not engine runtime behaviour.

Captured and opened locally (all viewer processes exited 0):

- `.cache/shots/equipment-party.png`: `--play --combat-test --shot … --shot-after 3`.
  Both characters wear armour; the gnome has a wooden shield. This wider angle is
  partly obscured by the combat UI.
- `.cache/shots/equipment-laezel-close.png`: same command plus
  `--camera=-49,4.5,-400 --look-at=-46,3.8,-402.5`. Lae'zel has the silver engraved
  half-plate chest, shoulder pieces, skirt panels and gith metal boots. The gnome's
  chain-shirt back, trousers, boots and shield are clear alongside her.
- `.cache/shots/equipment-avatar-front.png`: adds `--combat-autoplay` and
  `--camera=-49,4.3,-405.8 --look-at=-46,3.6,-402.5`. Close frontal view of the default
  deep-gnome cleric casting: chainmail torso and sleeves, trousers, boots, mace and
  shield. The sleeve/torso meshes follow the posed skeleton. The UI covers some feet.
- `.cache/shots/equipment-human-fighter.png`: actual `bg3x cc --race Human --class Fighter --spec`
  passed to `--character`, with `--camera=-43.6,4.3,-405.8 --look-at=-46,3.6,-402.5`.
  Human scale-mail torso, separate trousers and leather boots are visible; Lae'zel is
  beside him. This angle is dim, but the outfit silhouettes and pieces are distinct.
- `.cache/shots/equipment-avatar-close.png`: alternate reverse angle at the previous
  camera, with the default cleric. It is darker and the avatar is partly behind the UI;
  the frontal autoplay capture is better for inspecting his chain shirt.

The visible red skin, grey head/hair and overly white cloth are existing material
limitations. Red body regions still poke through at the avatar's hips/knees/feet;
mask requests are exposed but not applied by this change. No claim of complete body
occlusion or final dye/skin/hair appearance is made. The shared-turn camera can centre
Lae'zel even when the supplied initial look-at targets the avatar; autoplay gives a
clearer frontal avatar view after his move. No original-game comparison trace was
captured, and no shader files were edited.


### Drawn and sheathed weapons

Status: the bone selection, the weapon-type pair and the animation-subset lookup were read
from the 4.1.1.7631656 arm64 slice (symbol index, `objdump`, headless Ghidra on single
functions, for understanding only). The data comes from the install at runtime:
`bg3_world::unsheath`, loaded into `ResourceBanks::weapons`. The weapon state is sim state
(`bg3_sim::Sim::unsheath_state`). The viewer attaches weapon visuals and switches clip sets
from it (`viewer/src/characters.rs`, `Unsheath`).

#### State

`eoc::unsheath::StateComponent` holds an `EState`. Its values, in order, are **Instrument,
Melee, Ranged, Sheathed**: the `ls.anubis.game.UnsheathState` value names in the binary,
and the order `ResolveSubSetFromState` switches on. The component also has `m_Priority`
(`EPriority`) and `m_IsAnimated`. The server decides the state in
`esv::unsheath::StateSystem::ProcessEntities`. It gathers requests from one component per
cause (`DefaultComponent`, `CombatJoiningComponent`, `CombatLeavingComponent`,
`ScriptOverrideComponent`, `SpellAnimation*`, `Climbing`, `LockPicking`, `Consuming`,
`Combining`, `Coating`, `Resurrect`, `Summoned`, user requests). It sorts them (a `QuickSort`
over the request list) and applies the winner. The priority values and the comparator were
not traced.

What the sim models:

- **Default: Sheathed.** Out of a fight every character starts sheathed (`unsheath_state`
  returns Sheathed for characters without an entry). Both reference screenshots show
  weapons on the back out of combat.
- **Joining a fight draws the active weapon set** (`CombatJoiningComponent`). The sim does
  not model weapon-set switching (`eoc::WeaponSetComponent`). **Assumption**: a character
  with a ranged weapon and no melee weapon or shield is on the ranged set; everyone else is
  on the melee set. So joining gives Melee, or Ranged for such archers. This happens after
  `CombatStarted` / `Joined`.
- **A cast sets the state from the spell's `Sheathing` field**
  (`eoc::unsheath::ResolveStateFromSpell`). The values come from the `SpellSheathing` value
  list:
  - `Melee`, `Ranged` and `Instrument` give the state of the same name.
  - `WeaponSet` gives the active set's state.
  - `DontChange` keeps the current state.
  - `Sheathed` and anything unknown give Sheathed.
  - **Assumption**: `Somatic` keeps the current state. The engine resolves it through a
    lookup that was not traced.

  The sim raises `CombatEvent::Unsheathed` before `SpellCast`. So `Target_MainHandAttack`
  (`Sheathing "Melee"`) draws the melee weapon and `Projectile_MainHandAttack` (`"Ranged"`)
  the bow.
- **Leaving a fight sheathes** (`CombatLeavingComponent`). It applies to survivors at
  `check_combat_end`. Dead characters keep their state.

Not modelled: the out-of-combat default timer (`esv::unsheath::DefaultSystem`), script
overrides (`SetWeaponUnsheathed`, `Push/PopUnsheathedState`), the other causes, and the
sheathe/unsheathe animation (`ecl::CharacterTask_SheathUnsheath`). The viewer switches
instantly when it presents the event.

#### Where a weapon goes

`Public/<Module>/EquipmentTypes/EquipmentTypes.lsx` has one `EquipmentType` per weapon
kind, keyed by `UUID`. An item's root template points to it with `EquipmentTypeID`; this is
inherited through `ParentTemplateId` (**assumption**, same sparse-field rule as
`VisualTemplate`). For example `WPN_Mace` → template `WPN_HUM_Mace_A_0` → type `Mace`.
The attributes and the defaults when an attribute is missing come from
`eoc::EquipmentTypeData::Visit`:

- `BoneMainSheathed`, `BoneMainUnsheathed`, `BoneOffHandSheathed`,
  `BoneOffHandUnsheathed`, `BoneVersatileSheathed`: default `NONE`.
- `BoneVersatileUnsheathed`: default `Dummy_R_Hand`.
- `SourceBoneSheathed`, `SourceBoneVersatileSheathed`: default `NONE`.
- `SourceBoneVersatileUnsheathed`, `SourceBoneAlternativeUnsheathed`: default
  `Dummy_Attachment`.
- `WeaponType_OneHanded`, `WeaponType_TwoHanded`: animation weapon types.
- Sound types (not used here).

`Bone*` are character skeleton bones: `Dummy_Sheath_Upper_R/L`, `Dummy_Sheath_Hip_L/R`,
`Dummy_Sheath_Lower_R/L`, `Dummy_Sheath_Ranged`, `Dummy_Sheath_Music`, and the hand bones.
Every humanoid skeleton bank surveyed has them. `SourceBone*` are bones of the weapon model:
weapon GR2s carry `Dummy_Sheath`, `Dummy_Attachment` and variants. The weapon is placed so
its source bone coincides with the character bone (mesh = character bone ·
inverse(source bone rest)).

`ecl::equipment::GetAttachmentBoneName(type, unsheathed, off_hand, versatile, alternative,
slot, &bone, &source)` picks them. The weapon-slot branch was read in the assembly, which
mapped the four flags to registers. The source bone starts as `Dummy_Attachment`.

- **No equipment type:** drawn → `Dummy_R_Hand`; sheathed → `Dummy_Weapon_R_1H` with
  source `Dummy_Sheath`. No surveyed skeleton has that bone, so such a weapon is not drawn.
- **Drawn with the alternative grip:** off hand → `BoneOffHandUnsheathed`; main hand →
  `BoneVersatileUnsheathed`. Source `SourceBoneAlternativeUnsheathed` (`Dummy_Attachment_Rage`
  on several two-handers).
- **Versatile:** the off hand uses its off-hand bones; the main hand uses
  `BoneVersatile(Un)sheathed`. Source `SourceBoneVersatile(Un)sheathed`.
- **Otherwise:** `BoneMain…` / `BoneOffHand…` for the drawn or sheathed state. Source:
  `SourceBoneSheathed` when sheathed, `Dummy_Attachment` when drawn.

The client caller (`GetVisualsDesiredState`) computes the flags:

- `off_hand`: the slot is the melee or ranged off hand.
- `versatile`: the weapon has the `Versatile` property and the opposite hand's slot is in a
  set the function tests. **Assumption**: that set means the other hand is empty.
- `alternative`: needs a flag the caller reads, plus a melee weapon that is `Versatile` or
  `Twohanded` (likely raging). The sim never sets it.
- `unsheathed`: one flag per slot. **Assumption**: Melee draws the melee slots, Ranged the
  ranged slots, Instrument the instrument, Sheathed nothing. This is consistent with the
  slots `ResolveSubSetFromState` reads.

`WeaponFlags` bit positions come from the install's `ValueLists.txt` (`Twohanded` 0x400,
`Versatile` 0x800, `Melee` 0x1000).

Consequences, checked by the ignored test
`weapons_sheath_on_the_back_and_select_weapon_subsets`:

- Lae'zel's lone longsword (versatile) is sheathed on `Dummy_Sheath_Upper_R` with source
  `Dummy_Sheath` and drawn in `Dummy_R_Hand`.
- Her shortbow hangs on `Dummy_Sheath_Ranged` while sheathed or in Melee, and goes to
  `Dummy_L_Hand` in Ranged.
- The CC cleric's mace hangs on `Dummy_Sheath_Upper_R`.
- The `Shield` type names no sheathed bone. A sheathed shield is not attached, and the viewer
  hides it. The avatar reference screenshot shows no shield on the back either.
- Both weapons are always shown, on their sheath bone or in the hand. The old viewer rule
  "ranged only without melee" is gone.

#### Animation subset

`eoc::unsheath::ResolveSubSetFromState(world, entity, state)` picks the subset:

- Melee reads the melee main/off-hand slots; Ranged the ranged ones.
- Instrument reads the instrument (weapon type `Voice` without one).
- Sheathed returns the empty subset (`DFLT`).

`eoc::ResolveWeaponData` turns the two hands into a (main, off) weapon-type pair. Both default
to `Unarmed1H`. With both hands filled, each uses `WeaponType_OneHanded`. A single weapon
uses `WeaponType_TwoHanded` when it is `Twohanded` or `Versatile` (`WeaponComponent` flags
`& 0xC00`), otherwise `WeaponType_OneHanded`. A weapon without an equipment type leaves its
hand `Unarmed1H`.

`eoc::WeaponAnimationSetData::GetAnimationSet(main, off)` looks the pair up exactly in
`Public/<Module>/WeaponAnimationSetData/WeaponAnimationSetData.lsx`. `MainHand`/`OffHand`
default to `Unarmed1H` (`eoc::WeaponAnimationSet::Visit`). Its `AnimationSetUUID` is the
`MapKey` of an `AnimationSubSets` entry in the character's animation sets. For example
(`Slashing1H`, `Unarmed1H`) is the one-handed subset `f6fa9d34-…`, and (`Crossbow2H`,
`Unarmed1H`) the crossbow subset `32bbe70b-…` noted above. **Assumption**: a pair that is not
listed selects no subset (the default). The engine returns a static string there that was not
identified.

Observed with the install (`BG3_CHAR_DEBUG=1` viewer log; ignored test output):

- Lae'zel, Melee: (`Sword2H`, `Unarmed1H`) → `808afd36-…`. Idle
  `GTY_F_Rig_2HS_IDLE_Still_Peace_01`, run `HUM_F_Rig_2HS_LOCO_Run_F_Peace_01`, melee
  `GTY_F_Rig_2HS_CMBT_Melee_RHand_01_Attack`.
- Lae'zel, Ranged: (`Bow2H`, `Unarmed1H`) → `f709f223-…`. Idle and ranged attack are `BOW`
  clips.
- Lae'zel, sheathed: her own `GTY_F_Rig_Lae'zel_DFLT_IDLE_Still_Peace_01`.
- The deep-gnome cleric, Melee: (`Slashing1H`, `Shield1H`) → `fbddf847-…`.

The viewer resolves the idle, run and combat clips per drawn state with the existing set
order (**assumption**, see "Combat clips"). It swaps them in on `CombatEvent::Unsheathed`. A
type missing from a weapon subset falls back within each set to the default subset, as
before.

#### Verification (2026-10-22)

- Synthetic tests in `crates/bg3-world/src/unsheath.rs` cover:
  - every branch of the bone selection, the untyped fallback and unnamed (hidden) bones;
  - the weapon-type pairs;
  - the subset per state;
  - versatile only with a free hand;
  - spell `Sheathing`;
  - reading both files with the engine defaults.
- `weapons_are_drawn_for_the_fight_by_spell_and_sheathed_after` (bg3-sim) checks the order
  draw-after-`CombatStarted` and switch-before-`SpellCast`, and that the end of the fight
  sheathes.
- The ignored install test above checks the real data.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` and `cargo test --release -p bg3-sim -p bg3-world -- --ignored` all
  exited 0. `bg3x sim-replay --check` on `full-nautiloid.log` reports 12197 agreeing answers and
  0 disagreements. The new events are presentation-only and raise no story events.

Screenshots (local, `.cache/shots/`):

- `sheath-ooc.png` / `sheath-ooc-avatar-a.png` (back, `--no-follow
  --camera=51.3,18.9,-424.6 --look-at=53.06,18.45,-426.5`) and `sheath-ooc-avatar-b.png`
  (front, `--camera=54.9,18.9,-428.4`): `--play --shot-after 8`, out of combat. The cleric's mace hangs diagonally across his back, handle up at
  his right shoulder, and the shield is not shown. This matches the reference screenshot.
- `sheath-combat.png`: `--play --combat-test --shot-after 10`. Lae'zel holds the longsword
  two-handed in the `2HS` idle with the bow sheathed on her back. The avatar has the mace in
  his right hand and the shield on his left arm.
- `sheath-after-combat.png`, `sheath-laezel-after-a.png` / `-b.png`: `--combat-autoplay`
  until the fight ends. Both are sheathed again in their peace idles. The `-a`/`-b` shots add
  `--no-follow --camera=-42.3,4.2,-400.6` / `--camera=-37.8,4.2,-396.3` with
  `--look-at=-40.05,3.7,-398.43`.

**Reference screenshot (Lae'zel):** it was taken during the imp fight, so her longsword is
drawn, not sheathed; the earlier "other diagonal" discrepancy was a misreading. It does show her
holding the longsword **one-handed** (grip in the right hand by her hip, blade angled up behind
her, left hand free), whereas we pick the versatile (two-handed) pairing and its clips when the
other hand is empty. That is evidence against the "versatile means the other hand is empty"
**assumption**; how the engine decides the versatile grip is not yet read.

## Open

- Character shaders: what remains open is listed in `character-shaders.md`.
- Sheathing: the state arbitration (`EPriority`, the other causes), weapon sets, script
  overrides, sheathe/unsheathe animations, the alternative (rage) grip, and when the
  versatile (two-handed) grip applies (the reference shows Lae'zel one-handed).
- Helmet hair, camp-outfit switching and automatic visual attachments.
- Facial animation and physics/cloth for the extra mesh bones.
- Initial death/lying poses for scenery corpses (combat deaths now animate).
- Confirm the waterfall/override order and the fallback rule against the original
  (`ls::AnimationSetComponent` and related symbols).


### Character-creation appearance

The created avatar can replace template slots and append selected material presets before
equipment is layered (`ResolvedCharacter::apply_appearance`). CC shared visuals whose
VisualBank sets `NeedsSkeletonRemap` bind through fixed `SkeletonBank/Sockets` transforms
and their source GR2 inverse-bind matrices. This handles the default gnome's shared hair
without guessed fitting offsets. `gr::GrannyMeshBinding::UpdatePose` (4.1.1.7631656,
focused disassembly) resolves target sockets before building the source skeleton's indexed
composite buffer. Ordinary template meshes retain the assembly policy described above.
See `character-creation.md` for the data filtering, ordering assumptions, socket limitations
and local captures.
