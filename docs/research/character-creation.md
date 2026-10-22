# Character creation

Status: implemented in `crates/bg3-sim/src/cc/` (data, rules, applying the result to the avatar),
used by `bg3_sim::start::new_game`, `bg3x cc` and `bg3-viewer --play --cc`. Reference build
4.1.1.7631656. Sources: the data files below, the symbol table (`.cache/symbols/bg3.tsv`; no
function bodies were read for this), and the runtime trace `full-nautiloid.log` (a real
character creation: a male deep gnome cleric of Mystra with the Guild Artisan background).

Marks: **faithful** = follows the data or the trace with no interpretation; *approximate* /
*assumption* = our reading, not checked against the binary.

## Data

| What | File | Used fields |
|---|---|---|
| races, subraces | `Public/<Module>/Races/Races.lsx` | `UUID`, `Name`, `DisplayName`, `ParentGuid` (subrace → race), `ProgressionTableUUID`, `RaceEquipment`, `Tags`, `ExcludedGods` |
| classes, subclasses | `ClassDescriptions/ClassDescriptions.lsx` | `BaseHp`, `HpPerLevel`, `ParentGuid` (subclass → class), `ProgressionTableUUID`, `PrimaryAbility` / `SpellCastingAbility` (0 none, 1 Str … 6 Cha, *inferred*), `ClassEquipment`, `HasGod`, `MustPrepareSpells`, `Tags` |
| backgrounds | `Backgrounds/Backgrounds.lsx` | `Passives` (their `Boosts` give the two skills), `Tags`, `Hidden` (origin-only backgrounds) |
| deities | `Gods/Gods.lsx` | `Name`, `DisplayName`, `Tags` (deity + alignment tag) |
| progressions | `Progressions/Progressions.lsx` | `TableUUID`, `Level`, `ProgressionType` (0 class, 1 subclass, 2 race), `IsMulticlass`, `Boosts`, `PassivesAdded`/`Removed`, `Selectors`, `SubClasses` |
| selector lists | `Lists/{Ability,Skill,Spell,Passive,Equipment}Lists.lsx` | `UUID`, the item list |
| body presets | `CharacterCreationPresets/CharacterCreationPresets.lsx` | `RaceUUID`, `SubRaceUUID`, `BodyType`, `BodyShape`, `RootTemplate` (the avatar's template; also which races are playable) |
| class ability presets | `CharacterCreationPresets/AbilityDistributionPresets.lsx` | `ClassUUID`, six scores |
| selector defaults | `DefaultValues/{Abilities,Skills,Spells,Passives,Equipments}.lsx` | `TableUUID`, `Level`, `SelectorId`, `RaceUUID`, `OriginUUID`, `Add` |
| custom origin | `Origins/Origins.lsx`, `Generic` | `Passives` (`DeathSavingThrows`), `AppearanceTags` (`GENERIC`), `ReallyTags` (`REALLY_GENERIC`) |
| items, passives | stats `Equipment.txt`, `Weapon`/`Armor`/`Object`, `PassiveData` | equipment sets; `Slot`, `ArmorClass`, `Armor Class Ability`, `Ability Modifier Cap`, `ArmorType`, `Shield`, `Proficiency Group`, `BoostsOnEquipMainHand`; passive `Boosts`, `BoostConditions` |

Modules load in `BASE_LOAD_ORDER`; a later module's entry with the same UUID replaces the
earlier one whole (*assumption*, as for other UUID resources). This matters: `SharedDev`
re-defines every class's level-1 row with a `SelectAbilityBonus` and drops the races' fixed
`Ability` boosts (the +2/+1 is the player's to place), and `GustavX` re-orders the cleric's
level-1 `SubClasses` (Death Domain first).

### Selectors

`Selectors` is a `;` list of calls (`crate::cc::rules::parse_selectors`):

| Selector | Arguments | Meaning |
|---|---|---|
| `AddSpells` | list, selectorId, ability, resource, prepareType, cooldown | grants the whole spell list |
| `SelectSpells` | list, count, replaceCount, selectorId, ability, … | pick `count` spells |
| `SelectSkills` | list, count, selectorId | pick `count` skill proficiencies |
| `SelectSkillsExpertise` | list, count, ?, selectorId | expertise in skills already proficient |
| `SelectPassives` | list, count, selectorId | e.g. fighting style |
| `SelectEquipment` | list, count, selectorId | e.g. bard instrument |
| `SelectAbilityBonus` | list, selectorId, amounts… | `+2` and `+1` to different abilities |
| `SelectAbilities` | list, count, amount, selectorId | older half-elf form |

The engine side: `eoc::character_creation::_private::Apply*Selectors` /
`Validate*Selectors`, `eoc::character_creation::Validate`, `ValidateGod` (names only).

## Rules (`CcData::validate`)

- **Race/subrace**: a playable race (has a body preset); a subrace is required when the race
  has playable subraces and must be one of them; a body preset must exist for race, subrace,
  body type and shape. *Faithful to the data.*
- **Class/subclass**: a class with a level-1 progression; its level-1 row's `SubClasses`
  (cleric, paladin, sorcerer, warlock) require one of them, otherwise none.
- **Deity**: required for classes with `HasGod` (cleric), from `Gods.lsx` minus the race's and
  subrace's `ExcludedGods`; not allowed otherwise. *Approximate* (`ValidateGod` not read).
- **Point buy** (*approximate*): scores 8–15 before bonuses, 27 points, cost `s − 8` plus one
  more per step above 13 (the D&D 5e table). The numbers are not in the data (the UI binds
  `UsedAbilityPoints` / `TotalAbilityPoints`; `eoc::CharacterCreationStatsComponent` has
  `m_PointBuyAbilities`); every class preset in `AbilityDistributionPresets` spends exactly
  27 with this table (test `cc_install::presets_spend_the_point_buy_budget`). Unspent points
  are allowed (the UI shows them as to-do; whether the game blocks them is not known).
- **Picks**: every selector of the level-1 rows needs exactly `count` distinct options from
  its list; a skill already granted (race, background, another pick) can't be picked again,
  expertise needs proficiency, a spell already known can't be picked again.

## Defaults (`CcData::complete`, *approximate*)

Open choices are filled the way the CC screen starts out: first subrace, first level-1
subclass in data order, first allowed deity, the class's `AbilityDistributionPresets` entry,
and per selector the `DefaultValues` rows for its table and level (the selector's own
`SelectorId` before a generic row, a race-specific row before a generic one, concatenated),
skipping options already granted, then the list in order. The cleric's default bonus is
Wisdom +2, Strength +1; the fighter's style is Defence.

## The level-1 character (`CcData::build`)

- **Abilities**: point buy + bonus picks + unconditional `Ability(X, n)` boosts.
- **Hit points**: class `BaseHp` + Constitution modifier (*verified by trace* for clerics and
  Lae'zel, see `combat.md`), plus integer `IncreaseMaxHP` boosts.
- **Proficiencies, saves, skills, expertise, passives, spells**: the race, subrace, class and
  subclass level-1 rows' `Boosts` and `PassivesAdded`, the background's and the custom
  origin's passives (unconditional passive `Boosts` apply: background skills), then the
  picks; `AddSpells` lists; the main-hand weapon's `UnlockSpell` boosts when proficient
  (*approximate*: equipment boosts).
- **Equipment**: the race's `RaceEquipment` set, then the subclass's `ClassEquipment` if it
  has one (paladin oaths, draconic sorcerer) else the class's, then `SelectEquipment` picks;
  the first entry of each `add equipmentgroup`. *Verified by trace*: the cleric's
  `EQP_CC_Cleric` items and the gnome underwear are the items the trace's avatar receives.
- **Armour class**: body armour `ArmorClass` + capped ability modifier, else 10 + Dex; shield;
  passives with `BoostConditions` for the armour worn: `AC(1)` while `WearingArmor()`
  (Defence), `ACOverrideFormula` while `not WearingArmor()` (barbarian, monk with no shield,
  draconic resilience; taken when higher). `WearingArmor` = body armour whose `ArmorType` is
  not `None`/`Cloth` (*assumption*). The sim recomputes AC in combat
  (`Sim::update_armor_class`, base `Armor` of the template's `Character` entry, 10 for the
  `Hero*` entries).
- **Tags**: background, class, subclass, deity tags, the `Generic` origin's appearance and
  "really" tags. *Verified by trace*: the trace's avatar gets exactly `GUILD_ARTISAN`,
  `CLERIC`, `MYSTRA`, `ALIGN_GOOD`, `GENERIC`, `REALLY_GENERIC`, in that order, after
  `CharacterCreationFinished` (`TagSet` events with no story `SetTag`); its race and template
  tags (`DEEPGNOME`, `UNDERDARK`, `GNOME`, `SHORT`, `HUMANOID`, `CIVIL_RACE`,
  `VFX_ASHPILE_MEDIUM`, `MALE`) are announced before it (test
  `cc_install::trace_choice_reproduces_the_trace_avatar`).

## What the engine does around `CharacterCreationFinished`

Trace order (`full-nautiloid.log`), as `start::new_game` replays it: `LevelLoaded(SYS_CC_I)`,
`CharacterCreationStarted`, `LevelGameplayStarted`, `LevelGameplayReady`,
`MovieFinished(GUS_CGI01_Part1)`; then (the player creating the character) the avatar
`EnteredLevel(avatar, <preset template>, SYS_CC_I)`, `CharacterMadePlayer`,
`CharacterReservedUserIDChanged(-65536 → 65537)`, `GainedControl`, the guardian's
`EnteredLevel`, `CharacterSelectedForUser`, `CompanionSelectedForUser`,
`CharacterJoinedParty`, the race/template `TagSet`s, `CharacterCreationFinished`
(`esv::OsirisGameFunctions::NotifyCharacterCreationFinished` is the story's side). After it the
engine gives the items (`AddedTo`, `TemplateAddedTo`, `Equipped` / `EquipFailed`) and the
class/background/deity tags; the story already sees `IsTagged(avatar, GUILD_ARTISAN) = 0`
before and `IsTagged(avatar, MYSTRA) = 1` before the `TagSet(MYSTRA)` arrives.

`Sim::apply_created_character` (`cc/apply.rs`) runs right after the sim delivers
`CharacterCreationFinished`: the avatar's `CharacterSheet`, its engine tags (announced at the
next tag merge), and for combat the race/class origin (progression resources and `AddSpells`
lists) plus `cc::apply::combat_setup` (starting weapons/armour in hand, proficiencies, the
picked spells). `sim-replay` does the same with the trace's choice
(`Options::character`, `bg3x sim-replay --no-cc` to turn it off): the six tags are then
generated by the sim instead of fed (6064 events generated, 0 disagreements).

The engine's symbols for this: `esv::character_creation::SubsystemHelper`,
`eoc::CharacterCreationStatsComponent`, `eoc::character_creation::*DefinitionComponent`,
`eoc::character_creation::_private::ApplyBasics` / `ApplyRaceProgression` /
`ApplyProgression` / `TryApplyTemplate`, `eocnet::CharacterCreationUpdateMessage`.

## The trace's character (`start::trace_choice`)

Race `Gnome` / `Gnome_Deep`, masculine body (preset template `f660278d-…`), `Cleric`,
background Guild Artisan, deity Mystra: all read off the trace (tags, template, items). The
subclass, scores and picks are not in the trace and come from the defaults (Death Domain,
10/14/13/8/15/12, Wisdom +2 / Strength +1), giving 9 HP and AC 17.

## Not modelled

- Items as entities (the trace's `AddedTo`/`Equipped` events), prepared spells
  (`MustPrepareSpells`, `ValidateSpellPreparation`), conditional boosts other than the armour
  ones, `ActionResource` boosts outside combat, voice, origin characters as the avatar,
  multiclassing, levels above 1. Appearance limits are listed below.
- The subclass order and defaults the real CC screen shows (data order here).

## Front ends

- `bg3x cc [--list] [--race R --subrace S --body m|f --shape 0|1 --class C --subclass S
  --background B --deity D --abilities 15,14,13,12,10,8 --bonus Wis,Con --skills … --spells …
  --passives … --equipment …] [--spec]` prints the sheet (or the rule violations); names match
  internal names, English display names or UUIDs. `--spec` prints the choice for
  `bg3-viewer --play --character <spec>`.
- `bg3-viewer --play --cc`: a Bevy UI screen (`viewer/src/cc_ui.rs`); Start re-runs the
  viewer with `--character` (winit can't open a second event loop in one process, and the
  level viewer sets up before its window opens).

## Appearance

Implemented in `cc/appearance.rs`, read through the VFS in `BASE_LOAD_ORDER`, with the
same UUID replacement and stable first position as the other CC resources. The whole-row
replacement policy remains an **assumption**. Inspected the following resources directly
with `bg3x lsx -` on reference build 4.1.1.7631656; local survey output is in `.cache/cc-survey/`.
No appearance tables are embedded in source.

- `Public/{Shared,SharedDev}/CharacterCreation/CharacterCreationAppearanceVisuals.lsx`:
  `UUID` identifies a selectable entry; `SlotName` identifies the slot and `VisualResource`
  points into `VisualBank`. `DisplayName` is a loca handle. The modern rows filter by
  `RaceUUID`, `BodyType` and `BodyShape`. Observed slots include Head, Hair, Horns, Tail,
  Private Parts and DragonbornTop/Jaw/Chin. A few older Shared rows instead name a
  `RootTemplate`; these are matched to the selected body preset. No `SubRaceUUID` was
  present in the inspected visual rows; the loader supports one if supplied. Matching a
  `RaceUUID` against either the race or selected subrace, and treating an omitted filter
  as unrestricted, are **assumptions**. `HeadAppearanceUUID` constrains dragonborn pieces
  to a selected CC head entry. Until a matching head is selected these pieces are hidden;
  matching this dependency against the template's default head is not implemented.
- `Public/{Shared,GustavDev}/CharacterCreation/CharacterCreationSharedVisuals.lsx`:
  shared Hair, Beard and Piercing rows have no race/body attributes in the inspected data.
  `Races/Races.lsx` supplies the missing race restriction: `Visuals/Object` references
  shared CC entry UUIDs. For categories referenced by those lists (Hair and Beard), only
  the race/subrace ancestry's listed entries are offered. Inheriting/combining those lists
  through `ParentGuid` is an **assumption**. Categories without any race list, such as
  Piercing, remain unrestricted (**assumption**). A shared hairstyle name can resolve to
  different resource UUIDs for different races; the default gnome uses the dwarf-fit mesh
  named by its race list, rather than the same-name humanoid mesh. `BoneName` supplies rigid piercing attachment;
  otherwise the VisualBank `AttachBone` or the existing skinned-mesh binding is used.
  A null `VisualResource` is an explicit “none” choice, distinct from leaving the slot at
  its template default. Choices store the CC entry UUID, preserving entries that share
  the same resource. CLI UUID matching also accepts a resource UUID and picks its first
  allowed CC entry. Display-name substrings similarly prefer an exact name, then the
  first matching entry in merged load order (**assumption** for ambiguous names).
- `CharacterCreationPresets/CharacterCreation{Skin,Hair,Eye}Colors.lsx`: `UUID`, `Name`,
  `DisplayName`, `UIColor`, `MaterialPresetUUID`; SharedDev adds skin rows with `SkinType`
  (e.g. Dragonborn). All colour rows are currently offered, like an unrestricted palette
  (**assumption**); `SkinType` is retained but is not a race filter. UI colours are retained
  as strings and display names are resolved through loca.
- `CharacterCreation/CharacterCreationAppearanceMaterials.lsx`: `MaterialType` partitions
  the choices into Tattoo, Makeup, Scar, Passive, HairHighlight, HairGraying, LipsMakeup,
  HornColor and HornTipColor. Each choice supplies `MaterialPresetUUID`. The UI offers
  these data categories as selectors; it does not invent separate colour categories for
  the entries grouped under Passive. The original UI's grouping of those rows is not
  reconstructed. Camera and preview root-template attributes are not used for gameplay.
- `CharacterCreationPresets/CharacterCreationPresets.lsx` continues to select the avatar's
  `RootTemplate` by race/subrace/body, without changing its voice or camera metadata.
- SharedDev's `CharacterCreationPresets/CharacterCreationMaterialOverrides.lsx` is loaded
  into typed swap records: `SourceMaterialUUID`, `TargetMaterialUUID`, `MaterialType`,
  `ActiveMaterialPresetUUID`, `InactiveMaterialPresetUUID`, `Name`, `UUID`. The inspected
  rows are dragonborn Passive material swaps. They are retained for future CC preview
  activation; swapping graphs based on those activation presets is **not modelled**.

### Defaults and application order

There is no observed default-head or default-hairstyle flag. Unspecified slots and colours
therefore preserve the template visuals/presets, including its lack of hair. Array position
is not used to invent a hairstyle. `DefaultForBodyType` is present on Private Parts rows;
`complete_appearance` uses a matching explicit entry (**assumption** about the flag's exact
engine interpretation). `DefaultSkinColor` appears on older visual rows; it is retained as
metadata (mostly MaterialPresetBank references, with one legacy CC colour-entry reference)
but does not replace an unspecified skin colour: the template remains authoritative.
The engine's automatic selection rules for that field have not been observed.

The sim retains `Appearance` inside `CreatedCharacter.choice`. The viewer receives resolved
VisualBank IDs, attachment bones and material preset IDs. `ResolvedCharacter::apply_appearance`
replaces every existing part in a selected slot (or removes it for an explicit null visual),
leaves unselected slots alone, then recomputes body masks. This happens before equipment
layering, skeleton extension and material preparation, so existing skinned-part binding and
rigid attachment paths apply to new hair/beards/piercings too.

Shared hair also needs the install's skeleton sockets. Hair VisualBank resources declare
`NeedsSkeletonRemap`; their GR2 `BoneBindings` name `Socket_Hair_*` bones. The avatar body's
`SkeletonBank/Sockets` defines `Name`, `Bone`, `Position`, `Rotation`, `BoneConstraint`,
`ConstraintsPos` and `ConstraintsRot`. For a selected visual with `NeedsSkeletonRemap`, the
viewer adds the fixed target sockets as joints under their named body bones, and gives the
part the source GR2 inverse-bind matrices remapped by name. This computes target socket
world pose × source inverse bind for each binding. Non-remapped visuals keep the existing
assembly path; no offsets or scales are invented.

Remapped meshes are often authored on another race's rig: every CC beard offered to dwarves,
gnomes and halflings is a human or githyanki beard (`BEARD_HUM_M_Short_D` binds at y ≈ 1.7 m;
a male dwarf's `Head_M` is at 1.32 m), and shared long hair carries spring/braid chains that
no body skeleton has. Their bones resolve in this order (`Skeleton::extend_remapped`):

1. The avatar's skeleton so far (body bones, sockets, bones added by non-remapped visuals).
2. The rig of the visual owning the mesh's `SkeletonSlot`. Beards (`SkeletonSlot` `1`) bind
   `beard_*` face-rig bones, which exist only in the head's skeleton; the head visual
   registers slot `1` (`SkeletonSlot` `1` on head VisualBank resources, whose GR2 skeleton
   and `SkeletonBank` GR2 carry the same `beard_*` bones at the head's height). Those bones
   are placed at the head rig's bind pose. Visuals owning a slot are therefore assembled
   before all remapped ones.
3. The mesh's own GR2 rig (braid chains), placed at the bone's bind offset from its own
   parent under the remapped parent (a `Socket_Hair_*` or a face bone), not at the source
   rig's model-space height.

Skinning then uses target world × source inverse bind for each binding, so a human beard
vertex follows the dwarf's jaw bone and a human-height braid hangs from the dwarf's hair
socket. Before this, the beard bones and braid chains were taken from the source rig in model
space: a dwarf's beard rendered 0.4 m above his face and long hair/braids floated above the
head, while socket-bound hair (Rover's Tumble on a deep gnome) looked right.

Original evidence (reference build 4.1.1.7631656): `gr::GrannyVisualTemplate::InitInstance`
builds each visual's own skeleton from its GR2 (`gr::GrannySkeleton::Init`,
`ls::Visual::SetBaseSkeleton`) and calls `gr::GrannyMeshBinding::RemapToSkeleton` for its
meshes; `ls::Visual::AddAttachment` and `SetBaseSkeleton` call
`ls::Visual::GetOrCreateSkeletonSlot(FixedString, Visual*, Skeleton*)`, i.e. attached visuals
are tied together through named skeleton slots (call sites found by disassembling the thin
arm64 slice). The remapped path in `gr::GrannyMeshBinding::UpdatePose` (0x105b7a688) copies
target bone/socket world transforms into the source pose and builds an indexed composite
buffer using the source skeleton; missing bones can resolve through
`ls::Skeleton::GetSocketWorldTransform` (0x105b7a7b0–0x105b7a824, composite buffer at
0x105b7aa90). **Assumption:** a source bone that maps to nothing keeps its authored bind-local
offset under its remapped parent (the usual Granny local-pose behaviour; not traced at
runtime). REA/Ghidra was not used for pseudocode; local surveys stay in `.cache/`.

Verification: synthetic `remapped_source_chain_keeps_local_offset_under_target_socket`,
`remapped_bone_prefers_the_skeleton_slot_rig`,
`shared_cc_mesh_binds_to_target_socket_with_source_inverse_bind` and
`skeleton_bank_reads_fixed_hair_socket`; install test
`dwarf_cc_hair_and_beard_bind_on_the_head` (bg3-viewer, ignored) resolves
`Dwarves_Male_Hill_Player` with `HAIR_DWR_M_Braids_Long_B_Spring` and `BEARD_HUM_M_Short_D`
and checks the skinned bind-pose vertices against `Head_M` (the old path put the beard at
y 1.68–1.81 m and fails it). A local survey skinning every remapped Hair/Beard visual on male
hill dwarf, female gnome, male halfling, male human and female half-orc bodies went from
67/79/79/2/4 outliers (mean > 0.3 m from `Head_M` or top > 0.35 m above it) to 2/2/2/1/2; the
remainder are goblin-only or Orin's hair, which CC does not offer these races. Captures:
`.cache/shots/dwarf-before-face.png` / `dwarf-after-face.png` (Mineshaft beard),
`dwarf-braid-old-side.png` / `dwarf-braid-new-side.png` (Innkeep Braid + Glorious
Muttonchops), `halfling-braid-new-side.png`, `gnome-rover-new-face.png`.

The implementation currently handles fixed sockets (zero constraint masks and no
`BoneConstraint`), which cover the inspected hair bindings. Constrained sockets, the
`SkeletonSlot` `Hair` used by two hair visuals, socket overrides by head skeletons, and the
engine's complete missing-bone fallback remain unmodelled.

Material application order: graph defaults → material instance → **all template presets** (including `ColorPreset`) →
selected Skin, Hair, Eye presets → other chosen material presets in sorted category order →
the character visual's enabled explicit parameters. Thus a chosen colour follows the template's
preset of that category. The existing preset/explicit-parameter order is supported by
`ls::VisualSetSlots::InitVisualSlots` (see `character-shaders.md`); the insertion point and
ordering among CC selections are an **assumption**, not a runtime observation of CC. A null
material preset contributes no parameter assignments; it does not erase an earlier template
preset. Overlapping preset categories and active/inactive dragonborn graph swaps are a remaining
limit of this initial material selection path.

### Spec, CLI and UI

Old specs remain valid. New optional fields are `visual=<slot>:<CC entry UUID>` and
`material=<category>:<CC entry UUID>`, repeated once for each chosen slot/category. Empty
appearance preserves the legacy spec representation. Validation rejects unknown IDs, wrong
slots/categories and visuals outside the selected race/body/template/head filters.

`bg3x cc --list` prints all applicable visual options and all material categories with display
names and UUIDs. `--head`, `--hair`, `--beard`, `--skin`, `--hair-color`, `--eye-color` accept an allowed
entry/resource UUID or a case-insensitive display-name substring. Other categories can be set
through the spec or the viewer's Appearance section. Each UI selector includes “Template”;
explicit data “none” entries remain separate. Race/subrace/body or head changes clear visual
choices that no longer apply. The screen scrolls with the mouse wheel.

Synthetic verification: `spec_round_trip`, appearance parsing/UUID replacement, filter and
wrong-slot validation, explicit defaults/null visual resolution, CLI substring matching,
and `cc_replaces_slots_removes_none_and_layers_presets`. Install verification:
`cc_install::default_deep_gnome_appearance_and_hair_spec` loads the default male deep gnome,
requires Hair visuals and multiple skin colours, validates selected hair and round-trips its spec.


### Local verification of appearance (2026-10-22)

`cargo fmt --all --check`, workspace/all-target clippy with warnings denied, and
`cargo test --workspace` pass. The release bg3-sim ignored suite passes, including the
new deep gnome appearance/spec test. `sim-replay --check` against the local
`full-nautiloid.log` reports 12,197 agreeing answers and zero disagreements.

The capture `.cache/shots/cc-appearance.png` uses a race-allowed shared hairstyle and a
lighter skin entry selected by CLI display name. Opened the image: hair follows the scalp,
without the floating gap of an unremapped mesh, and the face/ears visibly change colour.
`.cache/shots/cc-default.png` captures the same avatar with its default template appearance.
Both use `--frame Gnomes_Male_Deep_Player --frame-mode face --shot-after 10`.
The captures/specs and all install-derived survey output remain gitignored.
