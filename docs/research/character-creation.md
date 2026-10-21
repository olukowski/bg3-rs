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
  ones, `ActionResource` boosts outside combat, appearance (only the preset's template:
  race and body type), voice, origin characters as the avatar, multiclassing, levels above 1.
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
