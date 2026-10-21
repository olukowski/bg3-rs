# The engine side of Osiris: API inventory and `bg3-sim`

Status: inventory of the opening complete; `crates/bg3-sim` answers 88% of the engine queries
of the opening, raises 82% of the engine events the story's own calls cause and runs the
opening's dialogs itself (every event of the six dialogs the story starts), matching the
original game on every graded answer (details below). This page is the to-do list for the rest.

Sources: the runtime log of a new game from `InitGame` through character creation into the
Nautiloid (`full-nautiloid.log`, see [osiris-runtime-log.md](osiris-runtime-log.md); never
committed), the GustavX story's declarations, the game's data files, and the reference binary
(4.1.1.7631656, arm64 slice, read with `objdump` against the symbol index; no decompiler output
is reproduced here).

## How to reproduce

```sh
cargo run --release -p bg3x -- sim-replay --inventory   # the tables at the end of this page
cargo run --release -p bg3x -- sim-replay --both        # replay, check mode then sim mode
cargo run --release -p bg3x -- sim-replay --no-dialogs  # dialog events fed from the log instead
cargo run --release -p bg3x -- sim-replay --dump .cache/sim.tsv   # every miss, for analysis
cargo test --release -p bg3-sim --test sim_replay -- --ignored    # the same as a test
cargo test --release -p bg3-sim --test dialog_e2e -- --ignored    # the dialogs, end to end
```

The trace defaults to `.cache/traces/full-nautiloid.log` (override with a path or
`BG3_OSIRIS_TRACE` for the test). Examples in the inventory are abbreviated: the UUID part of
every GUIDSTRING is replaced by `…`.

## The API the opening uses

The story declares 558 engine calls, 425 engine queries and 320 engine events. The opening uses
89 calls (1 115 times), 103 queries (13 543 times) and 102 events (12 381 deliveries, 367 of
them to events no rule uses).

The heavy hitters, which decide how much of the story can run on our state:

| Subsystem | Most used | What it needs |
|---|---|---|
| Entities | `Exists` 3 047, `IsCharacter` 1 207, `IsItem` 113, `GetTemplate` 94 | which objects exist (globals + loaded level + runtime-created), kind, template |
| Statuses | `IsStatusFromGroup` 2 578, `HasActiveStatus` 1 507, `GetStatusType` 420, `StatusApplied`/`StatusAttempt` 210 each | stats `StatusData`, active statuses per object |
| Tags | `TagSet` 6 673 (event), `IsTagged` 1 360, `SetTag` 272 | template/race/weight tags at spawn, story tags, the tag merge |
| Inventory | `AddedTo`/`TemplateAddedTo` 708 each, `Equipped`/`TemplateEquipped` 421 each, `IsWeapon` 436 | treasure generation, equipment, item stats |
| Combat/AI/perception | `HitpointsChanged` 583, `Saw` 457, `LostSightOf` 360, `GetHitpointsPercentage` 370, `IsSummon` 358, `HasPassive` 182 | HP, perception, passives (progressions) |
| Math | `IntegerSum` 551 | nothing |
| Flags | `GetFlag` 44, `SetFlag` 25, `ClearFlag` 29, `FlagSet` 34 | flag definitions and values |
| Timers | `TimerLaunch` 22, `ObjectTimerLaunch` 21, `ObjectTimerFinished` 26, `TimerFinished` 16 | a clock |
| Dialog | `DialogIsAutomated` 71, `DialogRequestStop` 14, `DialogStarted`/`Ended` | the dialogue runner (other track) |

Per-subsystem tables with every function, count and example are at the
[end of this page](#inventory).

## Event classification

Which engine events the sim must raise itself and which come from outside. The harness
(`bg3_sim::replay::classify`) uses exactly this split.

**Sim-owned** (raised by the sim; every one in the log is checked against the sim's queue):

| Event | Raised when | Source |
|---|---|---|
| `FlagSet`, `FlagCleared` | a story `SetFlag`/`ClearFlag` changes a value | `scr::SetFlag` → `OsirisVariableHelper::SetIndividualFlag` reads the old value first; trace: three identical `SetFlag` calls give one `FlagSet` |
| `TagSet`, `TagCleared` | the merged tag set of an object changes: at spawn (template, race, weight tags) and after `SetTag`/`ClearTag` | `esv::TagSystem::Update` → `StoryGameEventManager::ThrowTagSet` / `ThrowTagCleared` |
| `TimerFinished`, `ObjectTimerFinished` | a story timer runs out | `esv::GameTimerSystem`, `OsirisGameFunctions::OnGameTimerFinished` |
| `StatusAttempt`, `StatusApplied`, `StatusRemoved` | `ApplyStatus`/`RemoveStatus` from the story (story action id -1), `StatusList` statuses at spawn (id 0) | the status machine (not modelled beyond this) |
| `BaseFactionChanged` | `SetFaction` changes a character's faction (old and new faction as bare UUIDs) | trace: each of the 9 `SetFaction` calls is followed by one, the old faction being the level data's `Faction` |
| `DialogStarted`/`AutomatedDialogStarted`, `DialogActorJoined`, `DialogActorLeft`, `DialogEnded`/`AutomatedDialogEnded`, `NestedDialogPlayed`, `InstanceDialogChanged`, `RollResult`, `DialogRollResult`, `FlagSet`/`FlagCleared` with a dialog instance | a dialog the story started (`StartDialog_Internal`) plays on the sim | `esv::StoryDialogEventManager`, `esv::DialogRollListener`; the order is the runner's (`docs/formats/dialog.md`, "Events") |

Some sim-owned events have causes the sim does not model yet; the report counts them as "fed,
caused outside the sim", separately from real gaps:

- Dialog events of dialogs the sim did not start: the engine starts world dialogs on its own
  (`WRLD_TUT_DragonFlyBy_001`, instance 5 of the opening). The sim learns them from the event
  (`Sim::observe`), so instance ids and the involved-actor queries stay right.
- `TagSet`/status events on objects the sim did not load: loot made by treasure generation, the
  avatar made by character creation. The sim learns those objects from the events that name
  them (see below).

**External** (fed from the log; the sim observes them, `Sim::observe`): everything else, by
source:

| Source | Events |
|---|---|
| game flow / level loading | `GameModeStarted`, `LevelLoaded`, `LevelGameplayStarted`, `LevelGameplayReady`, `LevelUnloading`, `LevelTemplateLoaded`, `EnteredLevel`, `LeftLevel`, `MovieFinished` |
| character creation and users | `CharacterCreationStarted`/`Finished`, `CharacterMadePlayer`, `CharacterSelectedForUser`, `CompanionSelectedForUser`, `CharacterReservedUserIDChanged`, `GainedControl`, `LeveledUp` |
| party | `CharacterJoinedParty`, `CharacterLeftParty`, `AttachedToPartyGroup` |
| inventory and equipment (treasure generation, equipment sets) | `AddedTo`, `TemplateAddedTo`, `Equipped`, `TemplateEquipped`, `EquipFailed`, `StackedWith`, `GoldChanged`, `UnlockedRecipe` |
| combat, HP, death, perception, AI | `HitpointsChanged`, `Died`, `Dying`, `Saw`, `LostSightOf`, `ReactionInterruptAdded`, `MoveCapabilityChanged`, `CanBeLootedCapabilityChanged`, `ObscuredStateChanged`, `SwarmAIGroupJoined`, `CastSpell`, `UsingSpell`, `RollResult`, `Fell`, `Falling` |
| interaction | `RequestCanUse`, `Opened`, `Closed`, `TemplateOpening`, `EnteredTrigger`, `LeftTrigger`, `Activated`, `Deactivated`, `WentOnStage`, `ReposeAdded` |
| dialog | `DialogStartRequested` (the player clicks someone); dialog events of dialogs the sim did not start |
| crime | `CrimeIsRegistered`, `CrimeProcessingStarted`, `OnCrimeRemoved` |
| scripts and misc | `UserEvent`, `OnStoryOverride`, `TutorialEvent`, `TutorialClosed`, `QuestAccepted`, `QuestUpdateUnlocked`, `RelationChanged`, `Teleported`, … |

Observing an external event updates the sim the way the engine's own state changed: objects
named with a `CHARACTER`/`ITEM`/`TRIGGER` parameter exist (kind from the type),
`TemplateAddedTo`/`EnteredLevel` give an object's template (and so its kind and template tags)
and level, `AddedTo` its inventory holder, engine `TagSet`/`TagCleared`/`StatusApplied`/
`StatusRemoved` its tags and statuses, `FlagSet` from dialogs the flag value,
`CharacterJoinedParty`/`LeftParty` the party, `CharacterReservedUserIDChanged` the user,
`LevelUnloading`/`LevelLoaded` the loaded level.

`HitpointsChanged` and `Died` are deliberately **not** turned into state: the engine's value
changes before the event is delivered (`GetHitpointsPercentage` reports 0 and `IsDead` 1 for a
character whose `HitpointsChanged`/`Died` is still queued), so mirroring them would answer
wrongly; those queries stay with the trace until HP and death are simulated.

## What `bg3-sim` implements

Headless and deterministic (`crates/bg3-sim`, no Bevy). State: entities, flag values, tags,
statuses, timers, the sim clock and the queue of events for the story. Calls change the state
and queue events; nothing is delivered to the story during a call, matching the engine (every
`COsiris::Event` in the log is at nesting depth 0).

**Data** (`bg3_sim::load`, `GameData::load`):

| What | From |
|---|---|
| flag definitions (26 582) | `Public/<Module>/Flags/*.lsf`: `UUID`, `Name`, `Usage`, `Description` |
| tag definitions (1 103) | `Public/<Module>/Tags/*.lsf`: `UUID`, `Name`, `Categories` |
| statuses (4 443) | stats `StatusData` entries: `StatusType`, `StatusGroups` |
| root templates (25 550) | `Public/<Module>/RootTemplates/_merged.lsf`: `Name`, `Type`, `ParentTemplateId`, `Tags`, `Race`, `Stats`, `StatusList` |
| races | `Public/<Module>/Races/Races.lsx`: `ParentGuid`, `Tags` |
| weight categories | `Public/<Module>/WeightCategories/WeightCategories.lsx`: `MaxWeight` (g), `Tags`; stats `Weight` (kg) |
| factions | `Public/<Module>/Factions/Factions.lsx`: `Faction` (name), `UUID`; objects' `Faction` |
| module levels | `Mods/<Module>/meta.lsx`: `StartupLevelName`, `CharacterCreationLevelName`, `MenuLevelName`, `LobbyLevelName` (a new game starts in the character-creation level and goes to the startup level; their objects are loaded) |
| global objects (759) | `Mods/<Module>/Globals/<Level>/{Characters,Items,Triggers}/_merged.lsf`, all levels: they exist all game |
| level objects | `Mods/<Module>/Levels/<Level>/{Characters,Items,Triggers}/_merged.lsf`, while the level is loaded |
| character sheets | stats `Character` entries (abilities, `Level`, `Vitality`, `Armor`, `ProficiencyBonus`/`ProficiencyBonusScaling`, `SpellCastingAbility`, `Passives`, proficiency boosts in `DefaultBoosts`); `LevelOverride` of objects and templates; level maps from `Public/<Module>/Levelmaps/LevelMapValues.lsx` |
| difficulty classes | `Public/<Module>/DifficultyClasses/DifficultyClasses.lsx`: `UUID`, `Difficulties` (the DC) |
| dialogs | the dialog library (`bg3_dialog::library`, `DialogBank` resources) |
| combat | factions and relations, action resources, weapons, armour, equipment sets, spells, classes, origins, progressions, spell lists, objects' `Equipment`/`SkillList`/`CombatGroupID` (`bg3_sim::combat::data`, see [combat.md](combat.md)) |

**Combat** (`bg3_sim::combat`, see [combat.md](combat.md)) raises the combat events
(`EnteredCombat`, `CombatStarted`, `CombatRoundStarted`, `TurnStarted`, `ShareInitiative`,
`TurnEnded`, `UsingSpell`, `UsingSpellOnTarget`, `StartAttack`, `CastSpell`, `AttackedBy`,
`CastedSpell`, `Dying`, `KilledBy`, `Died`, `LeftCombat`, `CombatEnded`, `DownedChanged`) in the
order of a recorded fight, and implements `EnterCombat`, `EndTurn`, `IsInCombat` and
`SetOnStage` (the off-stage set only).

**Entities.** Identified by UUID (the engine resolves only the UUID,
`esv::StoryUtils::OsirisParamGUIDToGameObject`) and rendered back as `Name_<uuid>`
(`Sim::render`), which is how the engine prints them in events. `Exists` is true for any object
with that UUID (`OsirisGameFunctions::Exists`: `ls::uuid::HasByUUID<esv::Character>`, then the
UUID→handle map). Globals exist from the start; a level's own objects from the time it loads
(`TeleportPartiesToLevelWithMovie` requests the next level, it loads when the current one
unloads).

**Flags** (`Sim::set_flag`, `get_flag`). `scr::SetFlag(world, helper, headmaster, entity, flag,
set, dialog_instance, notify)` dispatches on the definition's `Usage` byte (`ls::Flag::Visit`):

| Usage | Meaning | Object |
|---|---|---|
| 0, 1 | not settable from the story (error 13) | |
| 2 | user flag (`OsirisVariableHelper::SetUserFlag`) | a character; copied to every party character with the same user id |
| 3 | party flag (`SetPartyFlag`) | a character |
| 4 | object flag (`SetIndividualFlag` on the object) | a character or an item (error 1 otherwise) |
| 5 | global flag (`SetIndividualFlag` on the null object) | ignored |
| 6 | object flag in the character's dialog variables (`SetFlagDialog`) | a character or an item |
| 7 | user flag, shared variant (`SetUserFlag` with its last bool set) | a character |

Unknown or null flag: error 12; null/missing object for a non-global flag: error 0; user/party
flag on a non-character: error 2. A failed `SetFlag` is a failed call for Osiris
(`OsirisDialogFunctions::SetFlag` returns false). The story's fourth argument is the last bool
(`notify` here; 1 in every call of the opening): `FlagSet`/`FlagCleared(flag, holder,
dialog_instance)` is raised only when the value changes. `GetFlag` (`scr::GetFlag`) dispatches
the same way.

**Tags** (`Sim::set_tag`, `clear_tag`, `is_tagged`, `update_tags`). Per object, tag sources as
in `esv::tags::*TagComponent`: template (template chain, race, instance), Osiris (`SetTag`) and
engine (weight category, tags reported by engine events). `SetTag`/`ClearTag`
(`OsirisGameFunctions::SetTag`/`ClearTag`) only accept characters and items, edit the Osiris
source and mark the object dirty; `esv::TagSystem::Update` merges the sources and raises
`TagSet`/`TagCleared` for the difference. `IsTagged` checks the Osiris source first and then the
merged set (`OsirisGameFunctions::IsTagged` → `LegacyConversion::IsTagged`), so a story tag is
visible at once and a cleared one until the next merge. The spawn tags are *inferred from the
trace*: race tags (race, then its parents), then every template's tags from the root ancestor
down, then the instance's, then the weight category of the stats `Weight`; this reproduces 5 516
of the 6 673 `TagSet` events of the opening.

**Statuses** (minimal). Active set per object with remaining duration. `ApplyStatus` makes the
status active at once (the trace: `HasAppliedStatus` is true right after `ApplyStatus`, before
`StatusAttempt`/`StatusApplied` arrive) and raises `StatusAttempt` + `StatusApplied` with story
action id -1; `RemoveStatus` raises `StatusRemoved`. `StatusList` statuses of an object and its
templates are applied at spawn (id 0). `IsStatusFromGroup`/`GetStatusType` read the stats
(`scr::IsStatusFromGroup` maps the group name with `eoc::StringToStatusGroup`). Not modelled:
immunities, stacking, durations in turns, boosts, engine-defined statuses without a stats entry
(`DYING`, `INSURFACE`, …; their type would come from the binary's status-type table).

**Timers** (`bg3_sim::timers`). All story timers go to `esv::GameTimerSystem::LEGACY_StartTimer`
keyed by `(name, object)`, the null object for `TimerLaunch`; duration `ms / 1000.0` as `f32`.
Flags: `TimerLaunch` 1, `ObjectTimerLaunch` 8 (+1 if its fourth argument is non-zero),
`RealtimeObjectTimerLaunch` 8|16. `ObjectTimerCancel` and `RealtimeObjectTimerCancel` both call
`LEGACY_StopTimer`, so either cancels either kind (the trace cancels a realtime timer with
`ObjectTimerCancel`). Relaunching restarts (trace: two launches, one finish). `Sim::advance`
fires due timers in due-time order.

**Dialogs** (`bg3_sim::dialog`). `StartDialog_Internal(dialog, ?, speaker × 6, [out] ok, [out]
instance, ?)` creates an instance with the next id (ids count from 1 in request order; the
engine's own world dialogs take ids too, the sim follows them from their start event) and the
non-null speakers in slots 0, 1, …; it starts at the next update (`Sim::update_dialogs`, part of
`Sim::tick`), a frame later as in the trace, where `DialogAddActorAtReservedSlot` adds an actor
in between (to the slot whose speaker list names it) and `DialogStarted` already lists it. A
`bg3_dialog::DialogRunner` plays it with the sim as its `DialogWorld` (`SimDialogWorld`):
check/set-flags on the sim's flags with the `scr::SetFlag` holder rules, tag checks on the merged
tags plus Osiris and dialog tags, `Tag` set-flags into the dialog tag source (announced by the tag
merge), `Quest` set-flags into a set of unlocked steps, rolls through `bg3-rules` (below). The
runner's events become the story events listed above. `DialogRequestStop(actor)` stops the
dialogs the actor speaks in (a requested one that has not started is dropped).
`IsSpeakerReserved` is true for actors of requested and running dialogs.
`DialogGetInvolvedPlayer/NPC(instance, n)` and the counts split the actors (slot order) into
players (party members or user-controlled characters) and NPCs; ended instances still answer
(the trace asks after `DialogEnded`). `DialogIsAutomated`, `DialogIsAllowingJoinCombat` and
`DialogAllowDeadSpeakers` read the resource's switches; `DialogSpeakerHasLines(dialog, n)` is
true when any node is spoken by slot `n − 1`, text or not (a slot whose only node is a cinematic
answers 1, an item that never speaks 0).

**Character sheets and rules** (`bg3_sim::character`, `bg3_sim::rules`). Characters with a
`Character` stats entry get a sheet at spawn: abilities, level (`LevelOverride` if set, else the
entry's `Level`), hit points (`Vitality`, *inferred*), armour class (`Armor`; no equipment),
proficiency bonus (`ProficiencyBonus`, else the `ProficiencyBonusScaling` level map), saving
throw and skill proficiencies, expertise and skill bonuses from the unconditional
`DefaultBoosts`, spellcasting ability and passives. The sim implements `bg3_rules::RulesContext`
over these (entities by a stable handle that survives level unloads), so conditions
(`HasStatus`, `Tagged`, `Dead`, ability scores, …) and functors (`ApplyStatus`, `RemoveStatus`,
`DealDamage`, `RegainHitPoints`) run on sim entities; HP changes raise `HitpointsChanged`
(`esv::StoryGameEventManager::ThrowHitpointsChanged`), statuses from functors raise
`StatusAttempt`/`StatusApplied` with story action id 0 (*unverified*). Dialog rolls are ability
checks (`bg3_rules::rules::roll_ability_check`) of the roll node's speaker against the
difficulty class's DC, on the sim's seeded dice; a `DialogScript` can force the outcomes (the
trace harness does, and checks each forced outcome is reachable with a non-critical d20).

**Queries answered by the sim:** `Exists`, `IsCharacter`, `IsItem`, `GetTemplate`, `GetRegion`
(follows inventory holders), `GetFlag`, `GetFlagName`, `IsTagged`, `HasActiveStatus`,
`HasAppliedStatus`, `HasActiveStatusWithGroup`, `HasAppliedStatusWithGroup`,
`IsStatusFromGroup`, `GetStatusType`, `TimerExists`, `ObjectTimerExists`, `IsSummon`,
`IsWeapon` (stats type `Weapon`), `IsPartyMember`, `GetFaction`, `GetActiveModStartupLevel`,
`IsCharacterCreationLevel`, `IsGameLevel` (not the character-creation, menu or lobby level;
*inferred*), and the math/string functions
(`IntegerSum` & co., `Concatenate`, `ConcatenateInteger`, `GUIDToString`, `StringToGUID`), and
with dialogs loaded `StartDialog_Internal`, `DialogIsAutomated`, `DialogIsAllowingJoinCombat`,
`DialogAllowDeadSpeakers`, `DialogSpeakerHasLines`, `IsSpeakerReserved`,
`DialogGetInvolvedPlayer`, `DialogGetInvolvedNPC`, `DialogGetNumberOfInvolvedPlayers`,
`DialogGetNumberOfInvolvedNPCs`. A
query the sim implements still falls back when it lacks the data: objects created outside the
sim (their tags may come from character-creation choices), objects of unknown kind, statuses
without a stats entry.

**Calls run by the sim:** `SetFlag`, `ClearFlag`, `SetTag`, `ClearTag`, `TimerLaunch`,
`TimerCancel`, `ObjectTimerLaunch`, `ObjectTimerCancel`, `RealtimeObjectTimerLaunch`,
`RealtimeObjectTimerCancel`, `ApplyStatus`, `RemoveStatus`, `TeleportPartiesToLevelWithMovie`,
`SetFaction`, `DialogRequestStop`, `DialogAddActorAtReservedSlot`.

**For other systems:** `bg3_sim::StoryState` (flags, tags, existence, kind, rendering, names),
`bg3_dialog::DialogWorld` (`SimDialogWorld`) and `bg3_rules::RulesContext`; writes queue the
same events as the Osiris calls. `bg3_sim::driver` runs the story on the sim without a trace:
`driver::run_events` delivers the queued events one at a time, and `bg3_sim::Game` owns the VM
and the sim for a front end: `tick(dt)` (dialogs start, timers fire, events are delivered),
`dialog_advance` / `dialog_choose` (player input), `take_notifications` (dialog lines, choices,
rolls to show). `bg3x dialog <name> --walk --sim` plays a dialog on the sim's world.

## Small story APIs and item use (2026-10-21)

`story_misc` dispatches the same compiled-story signatures in replay and live games. Symbols
were located in the reference build's symbol index; this addition used data resources and
runtime logs, without decompilation. `story_data` reads resources through the install VFS.
No resource contents or trace files are committed.

| Functions | State / answer | Limits |
|---|---|---|
| `SetHasDialog/2`, `SetCanInteract/2`, `SetCanFight/2`, `SetDetached/2`, `MakeNPC/1` | `Entity::story` retains dialog/interact/fight overrides, detachment and player/NPC state | Flags are readable by a front end; combat and presentation systems must opt into these overrides. `MakeNPC` does not remove a character from the party. |
| `CharacterDisableCrime/2`, `CharacterEnableCrime/2`, `CharacterDisableAllCrimes/1` | per-crime overrides and the all-crimes switch; disabling all clears older overrides, a later enable can override it | Crime detection and investigation remain absent. |
| `AddPreferredAiTargetTag/2`, `AddPassive/2` | ordered, duplicate-free target tags and script passives; an existing character sheet receives the passive too | AI targeting and passive boost execution are not added here. |
| `GetHostCharacter/1` | avatar selected by `CharacterSelectedForUser`, as a bare UUID | Single local host; platform/user ownership beyond the opening is not established. |
| `GetInventoryOwner/2`, `TemplateIsInInventory/3` | current inventory holder; count of known matching-template items, including nested containers, with cycle protection | Treasure generation and stack quantities are not added. All 97 owner and 63 template queries agree with the opening log; its template counts are zero. |
| `DataGetBackgroundTagCount/2`, `DataGetGodTagCount/2`, `DataGetReallyTagCount/2` and the three corresponding `…At/3` queries | `Origins/Origins.lsx` maps `GlobalTemplate` to `ReallyTags`, `BackgroundUUID`, `GodUUID`; background/god `Tags` are read from their definition resources | These are character-creation data, not the current merged tags. Empty/missing lists fail, indices start at **1**, outputs are `Name_uuid`. Custom character-creation choices are not available. All 57 queries agree with the trace. |
| `IterateOriginTags/3` | current tags in the `Origin` category → `CharacterTagEvent(character, tag, each_label)`, then `EntityEvent(character, finish_label)` | Template tags are available during story INIT, before their merge. The ten logged origin tag events match. |
| `IterateTagsCategory/3` | definitions in the requested enum category → `TagEvent(tag, each_label)`, then `EntityEvent(NULL, finish_label)` | 95 non-ignored category tag events and both completion events match. Iteration uses deterministic resource order; the original's order **within a multi-tag category is not established** by the replay's queue matcher. |
| `CheckRulesetModifierBool/3`, `CheckRulesetModifierString/3`, `GetRulesetModifierString/2` | modifier `Default`, overlaid by `RULESET_LARIAN`, then `DIFFICULTY_MEDIUM` values from `Ruleset/{RulesetModifiers,Rulesets,RulesetValues}.lsx` | Balanced only; unknown modifiers remain data gaps. Boolean/string checks return an integer comparison result, not query success/failure. All 16 queries agree. |
| `GetDisplayName/2`, `GetIdentity/3` | nearest instance/template translated-string handle; identity selector 0 from `MALE`/`FEMALE` identity tags | Runtime custom display names (`ResStr_…`) cannot be inferred from a template, so replay leaves characters without an explicit runtime name to the log (including companions loaded from saves). Other identity selectors remain data gaps. |
| `CharacterGetOwner/2`, `IsPartyFollower/2`, `IsActive/2` | explicit owner/follower/active state; activation/deactivation events update active state | No summoning/ownership or follower subsystem is introduced; the opening's queried defaults agree. |
| `CharacterHasDLC/3` | explicit, optional per-character entitlement set | Platform entitlements cannot be inferred from installed paks. Without host-provided data, this is a **data gap**, not an invented false answer. |
| `GetMaxPartySize/1` | 4, verified by all five opening queries | The reference campaign default; configurable/modded party limits are not loaded. |

`InitializePlayerData/1` is a **placeholder**: it records initialization on the entity, without
creating equipment, spells, or a character-creation selection. `SetMagicPocketsFlag/3` and
`ClearMagicPocketsFlag/3` maintain a separate `(character, flag)` set and retain their full
requests. Their propagation into ordinary story flags/party inventories remains a placeholder;
the two existing `FlagSet` gaps remain visible in replay.

The following are **recorded requests**, available through `Sim::take_story_effects`, with
no simulated UI/save/recipe/scheduler effect: `RegisterWaypoint/2`, `EnableTutorialEvent/2`,
`SetForceUpdate/2`, `UnlockRecipe/3`, `AddEntryToCustomBook/2`, `MapMarkerChangeLevel/2`,
`LookAtEntity/3`, `SetAiHint/2`, `AutoSave/0`. Their arguments remain available to a front end.

### Item-use evidence and supported lifecycle

`full-nautiloid.log` contains twelve complete uses (48 instance/template use events). The
opening's five script `Use(character, item, int, int, completion_label)` calls precede their
use events. For calls with a nonempty completion label, `EntityEvent(character, label)` comes
before `UseStarted`; story reactions to that event can run first. The common sequence is:

1. `UseStarted(character, item)`; its entire story reaction runs.
2. `TemplateUseStarted(character, root_template, item)`.
3. `UseFinished(character, item, 1)`; its story reaction runs.
4. `TemplateUseFinished(character, root_template, item, 1)`.

For the operation chair and drain beds these events are adjacent engine deliveries, apart
from the story processing between them. Shadowheart and the other occupied chair also have
story/status events between start and finish. The Brinepool is `S_TUT_Start_Brinepool`:
`UseStarted` invokes `PROC_ProcessUseItemWithDialog`; `TemplateUseStarted` follows, then
`DialogStarted` for `TUT_Start_Brinepool`, then the successful finish pair **while the dialog
is still running**. Dialog completion is later. Doors also have `Opened`/`Opening` and
`TemplateOpening` events before their use pair; those action effects are not implemented here.
The trace has **no timestamps or frame markers**: adjacency does not prove the same tick, and
no exact duration can be inferred.

`Game::use_item(user, item)` and script `Use/5` now queue the lifecycle without delivering
events inside the call. `Sim::tick` starts requests and completes them on a later tick;
requested dialogs start before item completions in that tick. This scheduling is an explicit
headless approximation, not a measured animation duration. The two integer script options are
retained without inventing their full meaning. Script use can seat an NPC on an item already
disabled with `SetCanInteract(…, 0)`, as the opening does; player use respects that switch.
Sit/lie uses retain `Entity::story::using_item`; animation and physical occupancy constraints
are not simulated.

Use availability comes from the **nearest explicit** `OnUsePeaceActions` list, instance first,
then the root-template chain; an explicit empty list blocks inheritance. The Brinepool has an
instance action (type 8) overriding an empty ancestor list; chair/bed actions (types 14/15)
are inherited. Combat lists and action attributes are retained for an executor. The current
lifecycle accepts these three observed action kinds, without nonempty conditions or healing;
other action kinds are rejected rather than reported as executed. Spell/consumable/door/lever
action effects, path-to-item, range, and animations need their own implementations. Combat uses are rejected until combat action selection is established; no claim is made
about unobserved action outcomes.

`live_game::using_brinepool_starts_the_traced_dialog` starts a new game, finishes the intro,
uses the pool through `Game::use_item`, and asserts the five-event sequence through dialog
start and successful item completion. Synthetic tests cover disabled player vs scripted use,
unsupported actions, one completion per request, removal during use, nested inventory cycles,
and origin queries driving story iteration.

Replay now grades tag iteration and the four use events as sim-owned. With no clock in the
log, requested script uses are advanced at logged use boundaries (as timers already are).
Player-driven uses remain trace inputs in replay; the new install test exercises that API
without a trace.

## Validation: `sim-replay`

The VM boots the story and receives the logged events in order. Sim-owned events are matched
against the sim's queue: if the sim raised the same event (same text), the sim's is delivered;
otherwise the logged one is fed and counted. Timers: the log has no timestamps, so when it
delivers a timer event the sim must have that timer running, and it fires then. Queries the sim
implements are answered by the sim and graded against the logged answer of the same event;
the rest fall back to the log. Two modes:

- **check**: the story runs on the logged answers; the sim's are only graded (each answer
  judged on its own).
- **sim**: the story runs on the sim's answers (the honest end-to-end number: a wrong answer
  changes what the story does next).

Dialogs: when the log delivers an event of a dialog the sim runs and the sim has not raised it
yet, the dialog plays on (lines finish, choices are taken) until it does. The player's choices
are not in the log; when a dialog starts, the harness searches them on a copy of the sim (depth
first, like `bg3-dialog`'s trace test: the walk whose events equal the logged ones) and plays
them as the player's input. Roll outcomes come from the log's `RollResult`s.

Results on `full-nautiloid.log` (12 382 segments = `InitGame` + 12 381 events):

| | check mode | sim mode |
|---|---|---|
| segments identical to the game (every fact, rule, call, query, PROC, goal record) | 100.00% | **100.00%** |
| databases with identical fact history | 1 133 / 1 133 | 1 133 / 1 133 |
| queries answered by the sim | 12 183 / 13 543 (89.96%) | 12 183 / 13 543 (89.96%) |
| sim answers that disagree with the game | 0 | 0 |
| queries from the log: not implemented / implemented, no data | 842 / 518 | 842 / 518 |
| calls run by the sim | 779 / 1 115 (69.87%) | 779 / 1 115 |
| dialog instances run by the sim, every event equal to the log | 6 / 6 | 6 / 6 |

Before the small-story/item-use change, the measured baseline was **11 919 / 13 543
(88.01%)**, with 0 disagreements and 510 / 1 115 calls run. Afterwards, 12 181 answers agree,
0 disagree, and **2 are ungraded**: `GetHostCharacter` calls in the DLC timer log a successful
query with an unset output, followed by four typed calls returning the avatar UUID. The VM
normalizes the output type; the discrepancy appears to be a call-site typing quirk. Those
two incomplete logged answers are preserved in both
modes and excluded from agreement/disagreement totals. All 12 382 segments and all 1 133
database fact histories remain identical in both modes. Only successful queries for which
**every output is unset in the log** are treated this way; success/failure and defined output
mismatches still count as disagreements.

The loaded-save trace `combat-20261021-183321.log` was also checked against an unchanged
checkout of the branch's starting commit: **11 disagreements before and after** (faction,
party membership and dialog participants absent from the loaded-save initialization), with
answers increasing from 1 084 / 1 178 to 1 092 / 1 178. It does not start with a new-game
initialization sequence, so it is not the zero-disagreement opening benchmark. No combat or
dialog implementation files were changed by this work.

Before the dialogs ran on the sim (`--no-dialogs`, and the state of this page before): 11 742
queries answered (86.7%), 458 calls, 5 863 sim-owned events generated, otherwise the same.

Sim-owned events (both modes):

| Event | In the log | Raised by the sim | Fed: cause outside the sim | Fed: gap | Raised but not in the log |
|---|---:|---:|---:|---:|---:|
| `FlagSet` | 34 | 32 | 0 | 2 | 0 |
| `DialogStarted` / `AutomatedDialogStarted` | 3 / 4 | 3 / 3 | 0 / 1 (world dialog) | 0 | 0 |
| `DialogActorJoined` / `DialogActorLeft` | 10 / 10 | 10 / 10 | 0 | 0 | 0 |
| `DialogEnded` / `AutomatedDialogEnded` | 3 / 3 | 3 / 3 | 0 | 0 | 0 |
| `RollResult` / `DialogRollResult` | 6 / 4 | 6 / 4 | 0 | 0 | 0 |
| `TagSet` | 6 673 | 5 516 | 1 015 | 142 | 18 |
| `TagCleared` | 4 | 2 | 0 | 2 | 0 |
| `TimerFinished` | 16 | 16 | 0 | 0 | 0 |
| `ObjectTimerFinished` | 26 | 26 | 0 | 0 | 0 |
| `StatusAttempt` | 211 | 135 | 7 | 69 | 17 |
| `StatusApplied` | 210 | 135 | 7 | 68 | 17 |
| `StatusRemoved` | 7 | 4 | 0 | 3 | 0 |
| `BaseFactionChanged` | 9 | 9 | 0 | 0 | 0 |
| `TagEvent` | 103 | 95 | 8 (ignored) | 0 | 0 |
| `CharacterTagEvent` | 10 | 10 | 0 | 0 | 0 |
| `EntityEvent` | 24 | 16 | 1 | 7 | 0 |
| `UseStarted` / `TemplateUseStarted` | 12 / 12 | 5 / 5 | 3 / 3 | 4 / 4 | 0 |
| `UseFinished` / `TemplateUseFinished` | 12 / 12 | 5 / 5 | 3 / 3 | 4 / 4 | 0 |

Every event the sim raised for a story call (flags, tags, timers, statuses) matches the game;
the gaps are all engine-side: two `FlagSet`s from `SetMagicPocketsFlag` (propagation not implemented),
`TagSet`s from gender and passive/boost tags (`MALE` on templates without it, `*_ADV` tags),
`TagCleared` from a shapeshift, and statuses applied by passives. The extras are spawn
`StatusList` statuses and weight tags the game did not apply (doors and some chests, see open
questions).

How the numbers moved while building it (sim mode, segments identical): no sim (plain replay)
100% with 100% answers from the log → first sim 99.96% (wrong `IsTagged` for objects created by
character creation, a race override on the instance) → 100% with 0 disagreements after objects
created outside the sim fell back to the log and the instance `Race` was read.

## Live game (no trace)

`bg3_sim::start::new_game` starts the campaign in the trace's order (`InitGame`,
`GameModeStarted("Campaign", 0, 0)`, the character-creation level, `CharacterMadePlayer`,
`CharacterReservedUserIDChanged`, `GainedControl`, `CharacterSelectedForUser`,
`CompanionSelectedForUser`, `CharacterJoinedParty`, `CharacterCreationFinished`, the story's
`TeleportPartiesToLevelWithMovie`, `LevelUnloading`, `LevelLoaded`, `LevelGameplayStarted`,
`LevelGameplayReady`). **Placeholder:** character creation is skipped; the avatar and guardian are
made from the trace's preset root templates with fixed UUIDs, and the user profile id is a fixed
string. The party lands on the level's `TriggerStart` trigger (`S_TUT_StartPoint_001`, the
position of the trace's `Teleported(…, "RegionSwap")`).

`bg3_sim::presentation` adds, for live games only (`Stage::live`; the replay numbers above don't
include them):

| Function / event | Behaviour | Source |
|---|---|---|
| `SetOnStage` → `WentOnStage(<bare uuid>, on)` | only when the state changes | trace (`esv::OsirisGameFunctions::SetOnStage`) |
| `SetVisible` | state only | `esv::OsirisGameFunctions::SetVisible` |
| `TeleportTo`, `TeleportToPosition` | into another level: `LeftLevel`, `EnteredLevel`, `Teleported(…, "RegionSwap")`; else `Teleported(…, "Invalid")` / `ItemTeleported`; then `EntityEvent(object, event)` | trace (Shadowheart, the avatar, the pod); order *inferred* |
| `CharacterMoveTo` | straight line at the default run/walk speed (no path finding yet), `EntityEvent` on arrival | trace; speed is a placeholder |
| `TriggerRegisterForCharacter` / `Unregister…` → `EnteredTrigger` / `LeftTrigger` | registered characters for area triggers; every character for `RoomTrigger`s | trace (*inferred* split), `esv::trigger::TrackingSystem` |
| `GetPosition`, `GetReservedUserID`, `GetUserProfileID` | from the sim's state | |
| `DialogStartRequested(target, player)` | raised for the player's click; the story selects and starts the dialog (`QRY_SelectAndStartDialog`) | `esv::CharacterProtocol::ProcessMsg` → `esv::StoryDialogEventManager::ThrowDialogRequestEvent` (the only caller) |

Trigger shapes (level `Triggers` objects and their root templates, `PhysicsType`): 1 sphere
(`Radius`), 2 box (`Extents` = half sizes, *inferred*: only then is the start point inside
`S_TUT_SaveTrigger_Start` and `S_TUT_MovementTutorialArea_001`, which the trace reports the
avatar entering; test `live_game::start_point_is_in_the_traced_triggers`), 3 prism over `Points`
(local x/z) up `Height` from the position (*inferred*).

What runs: the intro dialog, the automated wake-up line, walking into `S_TUT_Misc_RubbleRoom_001`
starts the devourer victim's ambient dialog, clicking the victim starts `TUT_Lab_TrappedDevourer`
with its rolls; 14 characters the story takes off stage in the trace are off stage
(`live_game::new_game_plays_the_opening`, `bg3x play`). The Brinepool now works through item
use, as described above. The measured `play --quiet` missing-functions list falls from **67
to 29**; newly reached initialization branches account for some of what remains:

| Remaining functions | Why |
|---|---|
| `RemoveSpell/3` (36), `GetEquippedItem/3` (11), `IsInInventoryOf/3`, `IterateInventoryByTemplate/4`, `ToInventory/5` | Spellbook/equipment mutations and inventory iteration/transfers need their own semantics. |
| `DialogSetVariableTranslatedString/3` (5) | Runtime translated dialog variables/custom names. |
| `BlockFlee/1`, `CombatGetGuidFor/2`, `SetImmortal/2`, `CanTrade/2`, `SetCanTrade/2`, `IsControlled/2` | Combat, trade, control and ownership integration beyond this change. |
| `GetModuleVersion/5`, `IsGameStateRunning/1`, `IterateUsers/2`, `NotifyCharacterCreationFinished/0`, `StartCharacterCreation/0`, `SatisfiesTagExpression/3`, `Random/2` | Session/module/character-creation services, tag expressions and engine random semantics. |
| `HideAllActivities/1`, `PlayHUDSound/2`, `PlayLoopEffect/5`, `QuestUpdate/3`, `SetDoNotFaceFlag/2`, `SetLevelStartingDialog/2`, `ShowActivity/1`, `ShowTutorial/2`, `StartActivity/1`, `TriggerSetSoundState/4` | Additional presentation/journal requests outside the requested API inventory. |

Uncounted data gaps remain visible in replay (notably platform DLC entitlements and custom
names); being registered does not imply that every call has enough data to answer.

## RE notes (symbols, 4.1.1.7631656)

- `esv::OsirisDialogFunctions::SetFlag/GetFlag/ClearFlag` → `scr::SetFlag`, `scr::GetFlag`:
  dispatch above. `ls::Flag::Visit` reads `UUID`, `Name`, `Description`, `Usage` (byte at
  +0x30).
- `esv::OsirisVariableHelper::SetIndividualFlag` reads the old value (`ls::VariableManager::GetFlag`)
  before writing; `SetUserFlag` walks `eoc::party::LEGACY_GetCharacters` and sets the flag on
  every character with the same user id (`esv::Character` +0x188).
- `esv::OsirisGameFunctions::SetTag/ClearTag`: character or item only
  (`StoryUtils::OsirisParamGUIDToCharacter`/`ToItem`), edit `esv::tags::OsirisTagComponent`,
  mark the entity dirty. `ClearTag` also calls `esv::TagSystem::RemoveDialogTag`.
- `esv::OsirisGameFunctions::IsTagged`: `OsirisTagComponent` first, then
  `LegacyConversion::IsTagged` (`eoc::TagComponent`). `esv::StoryUtils::IsTagged` (tag rules)
  merges `eoc::TagComponent`, `OsirisTagComponent` and `DialogTagComponent`.
- `esv::TagSystem::Update` merges template, Osiris, race, boost, dialog, Anubis and debug tag
  components; `StoryGameEventManager::ThrowTagSet`/`ThrowTagCleared` raise the events.
- `esv::OsirisGameFunctions::ObjectTimerLaunch`/`RealtimeObjectTimerLaunch`/`TimerLaunch`:
  `GameTimerSystem::LEGACY_StartTimer(name, entity, ms / 1000.0f, 1, flags)`; the cancels:
  `LEGACY_StopTimer(name, entity)`; an empty name or an unknown object fails the call.
- `esv::OsirisGameFunctions::Exists`: parses the GUID, `ls::uuid::HasByUUID<esv::Character>`,
  then the UUID→handle mapping.
- `esv::OsirisGameFunctions::GetStatusType` switches on the status type of the status prototype
  (built-in statuses included); `scr::IsStatusFromGroup` uses `eoc::StringToStatusGroup`.

## Open questions and next steps

1. **Character creation and loot** create objects the sim only learns from events; with
   `CharacterCreationFinished`-time data (race, body type, deity, class) and treasure tables,
   the sim could create them itself.
2. **Tags the sim misses** (142 gaps): gender tags not on the template chain (body type of the
   character visual?), passive/boost tags (`Tag()` boosts in passives), shapeshift tag removal.
3. **Spawn statuses**: `StatusList` on doors (`DEBRIS_THRESHOLD_LESSER`) and some characters
   is not applied by the game at spawn (17 extras); 68 spawn statuses come from elsewhere
   (passives, auras).
4. **Status machine**: when exactly `HasActiveStatus` turns true relative to `StatusApplied`;
   immunities, stacking, turn durations; built-in status types from the binary.
5. **Next queries by volume**: `GetHitpointsPercentage` 370 and `IsDead` (HP exist on character
   sheets now, but the opening's HP changes come from combat and scripted deaths the sim does
   not run), `HasPassive` 182 (progressions), `GetInventoryOwner` 97 and
   `TemplateIsInInventory` 63 (inventory), `CharacterHasDLC` 66, `GetReservedUserID` 64.
6. **Timing**: the trace has no timestamps; a capture with frame numbers or game time would let
   the sim drive timers and event batching itself instead of following the log's order.
7. `SetMagicPocketsFlag`/`ClearMagicPocketsFlag` (flags that follow an item into a character's
   inventory) explain the two `FlagSet` gaps.
8. **Dialogs.** The meaning of `StartDialog_Internal`'s second and last integer arguments (0
   in every call of the opening); whether the engine maps the given speakers onto the dialog's
   speaker list instead of taking them in order (in order fits every call of the opening);
   `DialogIsAutomated` reads `automated` only (fits the opening; world/behaviour dialogs not
   seen); when a requested dialog really starts (the sim: the next update).
9. **Character sheets.** The created avatar's sheet comes from its template's stats entry, not
   from character creation (race, class, abilities chosen by the player), and origins' class
   proficiencies come from progressions the sim does not run; dialog checks of player
   characters therefore use too few modifiers. Max HP is `Vitality` (*inferred*; boosts, class
   levels and Constitution not modelled) — except for origin characters once they fight:
   combat gives them their class HP and their race/class progressions' resources (see
   [combat.md](combat.md)).

## Inventory

Generated by `bg3x sim-replay --inventory` from `full-nautiloid.log`. "Sim": `yes` = implemented
by `bg3-sim`, `raises` = sim-owned event, `external` = fed from outside. "Failed / ignored":
calls logged as failed, queries logged as failed, events logged as ignored (no rules).

- 558 calls declared by the story, 89 used in the trace
- 425 querys declared by the story, 103 used in the trace
- 320 events declared by the story, 102 used in the trace

| Subsystem | calls (uses) | queries (uses) | events (deliveries) | in bg3-sim |
|---|---|---|---|---|
| Character creation | 3 (12) | 1 (6) | 5 (5) | 2 of 9 |
| Combat, AI & perception | 13 (168) | 8 (652) | 21 (1941) | 8 of 42 |
| Crime | 5 (51) | 10 (48) | 3 (9) | 3 of 18 |
| Dialog | 5 (40) | 11 (177) | 8 (38) | 20 of 24 |
| Entities & templates | 10 (180) | 12 (4518) | 9 (229) | 12 of 31 |
| Flags | 5 (83) | 2 (63) | 1 (34) | 7 of 8 |
| Inventory & items | 8 (19) | 15 (673) | 19 (2456) | 10 of 42 |
| Levels & game flow | 5 (7) | 6 (226) | 10 (358) | 7 of 21 |
| Math & strings | 0 (0) | 9 (635) | 0 (0) | 8 of 9 |
| Movement | 2 (7) | 0 (0) | 3 (7) | 0 of 5 |
| Other | 2 (11) | 0 (0) | 0 (0) | 1 of 2 |
| Party & users | 5 (26) | 12 (586) | 7 (22) | 7 of 24 |
| Presentation | 5 (7) | 1 (3) | 0 (0) | 1 of 6 |
| Quests & journal | 2 (4) | 0 (0) | 2 (2) | 1 of 4 |
| Statuses | 2 (37) | 6 (4532) | 6 (440) | 11 of 14 |
| Tags | 5 (313) | 8 (1418) | 4 (6790) | 16 of 17 |
| Timers | 6 (79) | 0 (0) | 2 (42) | 8 of 8 |
| Tutorials | 6 (71) | 2 (6) | 2 (8) | 1 of 10 |

### Character creation

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `InitializePlayerData/1` | 10 |  | yes | `InitializePlayerData((CHARACTER)S_Player_Karlach_…)` |
| call | `NotifyCharacterCreationFinished/0` | 1 |  |  | `NotifyCharacterCreationFinished()` |
| call | `StartCharacterCreation/0` | 1 |  |  | `StartCharacterCreation()` |
| query | `IsCharacterCreationLevel/2` | 6 |  | yes | `IsCharacterCreationLevel("SYS_CC_I", [out] 1)` |
| event | `CharacterCreationFinished/0` | 1 |  | external | `CharacterCreationFinished()` |
| event | `CharacterCreationStarted/0` | 1 |  | external | `CharacterCreationStarted()` |
| event | `CharacterSelectedForUser/2` | 1 |  | external | `CharacterSelectedForUser(Gnomes_Male_Deep_Player_…, 65537)` |
| event | `CompanionSelectedForUser/2` | 1 |  | external | `CompanionSelectedForUser(Humans_Female_Daisy_…, 65537)` |
| event | `LeveledUp/1` | 1 |  | external | `LeveledUp(S_LOW_Mystra_InvisibleHelper_…)` |

### Combat, AI & perception

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `ClearRelation/2` | 104 |  |  | `ClearRelation((FACTION)ACT0_TUT_LabDevourer_…, (FACTION)Hero_Player1_…)` |
| call | `RemoveSpell/3` | 36 |  |  | `RemoveSpell(Gnomes_Male_Deep_Player_…, "Shout_Bard_Perform_DOS2_1_Drum", 1)` |
| call | `SetFaction/2` | 9 |  | yes | `SetFaction((CHARACTER)S_Player_Karlach_…, (FACTION)Origin_Karlach_…)` |
| call | `SetCanFight/2` | 5 |  | yes | `SetCanFight(S_TUT_Helm_MindFlayer_…, 0)` |
| call | `SetRelation/3` | 4 |  |  | `SetRelation((FACTION)ACT0_TUT_LabDevourer_…, (FACTION)Hero_…, 0)` |
| call | `SetAiHint/2` | 2 |  | yes | `SetAiHint((CHARACTER)S_TUT_Helm_MindFlayer_…, (TAG)AI_HINT_UNIQUE_…)` |
| call | `SetCanJoinCombat/2` | 2 |  |  | `SetCanJoinCombat(S_TUT_Lab_TrappedDevourer_…, 0)` |
| call | `AddPassive/2` | 1 |  | yes | `AddPassive(S_Player_ShadowHeart_…, "FearOfWolves_Shadowheart")` |
| call | `BlockFlee/1` | 1 |  |  | `BlockFlee(Gnomes_Male_Deep_Player_…)` |
| call | `Die/6` | 1 |  |  | `Die(S_TUT_Lab_DevourerVictim_…, DEATHTYPE.PHYSICAL, NULL_…, 0, 0, 0.0)` |
| call | `SetHitpointsPercentage/3` | 1 |  |  | `SetHitpointsPercentage(S_TUT_Lab_TrappedDevourer_…, 100.0, "Guaranteed")` |
| call | `SetImmortal/2` | 1 |  |  | `SetImmortal((CHARACTER)S_GLO_Monitor_…, 1)` |
| call | `UseSpell/5` | 1 |  |  | `UseSpell(S_TUT_Lab_TrappedDevourer_…, "Projectile_Jump", S_TUT_UsEscapePosition_Jump_…, NULL_…, 0)` |
| query | `GetHitpointsPercentage/2` | 370 |  |  | `GetHitpointsPercentage(S_GLO_CharacterCreationDummy_003_…, [out] (REAL))` |
| query | `HasPassive/3` | 182 |  |  | `HasPassive(S_GLO_GithKnight_…, "MagicArrow", [out] 1)` |
| query | `GetFaction/2` | 86 |  | yes | `GetFaction(S_OriginIntro_DarkUrgeDummy_…, [out] (GUIDSTRING))` |
| query | `CanFight/2` | 5 |  |  | `CanFight(Gnomes_Male_Deep_Player_…, [out] 1)` |
| query | `GetRelation/3` | 4 |  |  | `GetRelation((FACTION)ACT0_TUT_LabDevourer_…, (FACTION)Hero_…, [out] (INTEGER))` |
| query | `CanLearnSpell/4` | 2 | 2 |  | `CanLearnSpell(Gnomes_Male_Deep_Player_…, LOOT_SCROLL_Revivify_…, 0, [out] 1)` |
| query | `IsInCombat/2` | 2 |  | yes | `IsInCombat(S_TUT_Lab_TrappedDevourer_…, [out] 0)` |
| query | `CombatGetGuidFor/2` | 1 | 1 |  | `CombatGetGuidFor(S_Player_Laezel_…, [out] (GUIDSTRING))` |
| event | `HitpointsChanged/2` | 583 |  | external | `HitpointsChanged(S_GLO_CharacterCreationDummy_003_…, 100.0)` |
| event | `Saw/3` | 457 |  | external | `Saw(S_GLO_CharacterCreationDummy_004_…, OriginIntro_Player1_…, 0)` |
| event | `LostSightOf/2` | 360 |  | external | `LostSightOf(OriginIntro_Player3_…, S_GLO_CharacterCreationDummy_004_…)` |
| event | `ReactionInterruptAdded/2` | 339 | 339 | external | `ReactionInterruptAdded/2` |
| event | `MoveCapabilityChanged/2` | 82 |  | external | `MoveCapabilityChanged(S_GLO_CharacterCreationDummy_003_…, 0)` |
| event | `CanBeLootedCapabilityChanged/2` | 31 |  | external | `CanBeLootedCapabilityChanged(S_GLO_BaneDummyForGortashSwD_…, 1)` |
| event | `Died/1` | 31 |  | external | `Died(S_GLO_BaneDummyForGortashSwD_…)` |
| event | `SwarmAIGroupJoined/2` | 17 | 17 | external | `SwarmAIGroupJoined/2` |
| event | `BaseFactionChanged/3` | 9 |  | raises | `BaseFactionChanged(S_Player_Karlach_…, …, …)` |
| event | `ObscuredStateChanged/2` | 8 |  | external | `ObscuredStateChanged(S_WYR_VampireSpawns_Dalyria_…, "HeavilyObscured")` |
| event | `RollResult/6` | 6 |  | raises | `RollResult("…_3", Gnomes_Male_Deep_Player_…, Gnomes_Male_Deep_Player_…, 0, 1, CRITICALITYTYPE.NONE)` |
| event | `ShortRestCapable/2` | 6 | 6 | external | `ShortRestCapable/2` |
| event | `RelationChanged/4` | 4 |  | external | `RelationChanged(…, …, 0, 1)` |
| event | `CastSpell/5` | 1 |  | external | `CastSpell(S_TUT_Lab_TrappedDevourer_…, "Projectile_Jump", "projectile", "", 2)` |
| event | `CastedSpell/5` | 1 |  | external | `CastedSpell(S_TUT_Lab_TrappedDevourer_…, "Projectile_Jump", "projectile", "", 2)` |
| event | `Dying/1` | 1 |  | external | `Dying(S_TUT_Lab_DevourerVictim_…)` |
| event | `Falling/2` | 1 |  | external | `Falling(S_TUT_Lab_TrappedDevourer_…, NULL_…)` |
| event | `Fell/2` | 1 |  | external | `Fell(S_TUT_Lab_TrappedDevourer_…, NULL_…)` |
| event | `ShapeshiftedHitpointsChanged/2` | 1 |  | external | `ShapeshiftedHitpointsChanged(S_GLO_DevilishOx_…, 100.0)` |
| event | `UsingSpell/5` | 1 |  | external | `UsingSpell(S_TUT_Lab_TrappedDevourer_…, "Projectile_Jump", "projectile", "", 2)` |
| event | `UsingSpellAtPosition/8` | 1 |  | external | `UsingSpellAtPosition(S_TUT_Lab_TrappedDevourer_…, -13.4, 17.6, -421.6, "Projectile_Jump", "projectile", "", 2)` |

### Crime

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `CharacterDisableCrime/2` | 25 |  | yes | `CharacterDisableCrime((CHARACTER)S_Player_Karlach_…, "DangerousMonsterReaction")` |
| call | `CharacterDisableAllCrimes/1` | 15 |  | yes | `CharacterDisableAllCrimes((CHARACTER)S_GLO_JergalAvatar_…)` |
| call | `BlockNewCrimeReactions/2` | 4 |  |  | `BlockNewCrimeReactions(S_TUT_Lab_TrappedDevourer_…, 1)` |
| call | `CharacterEnableCrime/2` | 4 |  | yes | `CharacterEnableCrime((CHARACTER)S_GLO_Monitor_…, "SelfHealing")` |
| call | `CharacterStopCrimeWithID/2` | 3 |  |  | `CharacterStopCrimeWithID(…, 1)` |
| query | `CharacterGetCrimeRegion/2` | 12 |  |  | `CharacterGetCrimeRegion(S_GLO_BaneDummyForGortashSwD_…, [out] (STRING))` |
| query | `CrimeGetType/2` | 6 |  |  | `CrimeGetType(1, [out] (STRING))` |
| query | `CrimeHasProperty/3` | 6 |  |  | `CrimeHasProperty(1, (DISTURBANCEPROPERTY)RecheckOnFastTravel_…, [out] 1)` |
| query | `CrimeTypeHasProperty/3` | 6 |  |  | `CrimeTypeHasProperty("SelfHealing", (DISTURBANCEPROPERTY)BlockIfPlayerInCombat_…, [out] 1)` |
| query | `CharacterRegisterCrime/6` | 3 |  |  | `CharacterRegisterCrime(S_TUT_Lab_TrappedDevourer_…, "SelfHealing", NULL_…, S_TUT_Lab_TrappedDevourer_…, 1, [out] 1)` |
| query | `CrimeGetCriminal/3` | 3 |  |  | `CrimeGetCriminal(1, 1, [out] (GUIDSTRING))` |
| query | `CrimeGetCriminalsCount/2` | 3 |  |  | `CrimeGetCriminalsCount(1, [out] (INTEGER))` |
| query | `CrimeGetLeadInvestigator/2` | 3 | 3 |  | `CrimeGetLeadInvestigator(1, [out] (GUIDSTRING))` |
| query | `CrimeGetNewID/1` | 3 |  |  | `CrimeGetNewID([out] (INTEGER))` |
| query | `CrimeIsContinuous/2` | 3 |  |  | `CrimeIsContinuous(1, [out] 1)` |
| event | `CrimeIsRegistered/8` | 3 |  | external | `CrimeIsRegistered(S_TUT_Lab_TrappedDevourer_…, "SelfHealing", 1, NULL_…, S_TUT_Lab_TrappedDevourer_…, NULL_…, NULL_…, NULL_…)` |
| event | `CrimeProcessingStarted/2` | 3 |  | external | `CrimeProcessingStarted(1, 0)` |
| event | `OnCrimeRemoved/6` | 3 |  | external | `OnCrimeRemoved(1, S_TUT_Lab_TrappedDevourer_…, S_TUT_Lab_TrappedDevourer_…, NULL_…, NULL_…, NULL_…)` |

### Dialog

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `SetHasDialog/2` | 17 |  | yes | `SetHasDialog(S_Player_Jaheira_…, 1)` |
| call | `DialogRequestStop/1` | 14 |  | yes | `DialogRequestStop(Gnomes_Male_Deep_Player_…)` |
| call | `DialogSetVariableTranslatedString/3` | 7 |  |  | `DialogSetVariableTranslatedString((DIALOGRESOURCE)GLO_CompanionSwap_Camp_…, "GLO_CompanionSwap_AvatarName1_…", "h…")` |
| call | `DialogAddActorAtReservedSlot/5` | 1 |  | yes | `DialogAddActorAtReservedSlot(7, S_TUT_Lab_TrappedDevourer_…, 0, 0, 0)` |
| call | `SetLevelStartingDialog/2` | 1 |  |  | `SetLevelStartingDialog(1, 1)` |
| query | `DialogIsAutomated/2` | 71 |  | yes | `DialogIsAutomated(TUT_Start_OriginIntroduction_Default_…, [out] (INTEGER))` |
| query | `IsSpeakerReserved/2` | 25 |  | yes | `IsSpeakerReserved(Gnomes_Male_Deep_Player_…, [out] 0)` |
| query | `DialogGetInvolvedNPC/3` | 16 | 8 | yes | `DialogGetInvolvedNPC(1, 1, [out] (GUIDSTRING))` |
| query | `DialogSpeakerHasLines/3` | 13 |  | yes | `DialogSpeakerHasLines(TUT_Start_OriginIntroduction_Default_…, 1, [out] (INTEGER))` |
| query | `DialogGetInvolvedPlayer/3` | 11 | 7 | yes | `DialogGetInvolvedPlayer(1, 1, [out] (GUIDSTRING))` |
| query | `DialogIsAllowingJoinCombat/2` | 10 |  | yes | `DialogIsAllowingJoinCombat(TUT_Start_OriginIntroduction_Default_…, [out] (INTEGER))` |
| query | `DialogAllowDeadSpeakers/2` | 9 |  | yes | `DialogAllowDeadSpeakers(TUT_Start_OriginIntroduction_Default_…, [out] (INTEGER))` |
| query | `DialogGetNumberOfInvolvedNPCs/2` | 7 |  | yes | `DialogGetNumberOfInvolvedNPCs(1, [out] (INTEGER))` |
| query | `DialogGetNumberOfInvolvedPlayers/2` | 7 |  | yes | `DialogGetNumberOfInvolvedPlayers(1, [out] (INTEGER))` |
| query | `StartDialog_Internal/11` | 6 |  | yes | `StartDialog_Internal(TUT_Start_OriginIntroduction_Default_…, 0, Gnomes_Male_Deep_Player_…, NULL_…, NULL_…, NULL_…, NULL_…, NULL_…, [out] 1, [out] (INTEGER), 0)` |
| query | `GetHasDefaultDialog/2` | 2 |  |  | `GetHasDefaultDialog(S_TUT_Lab_TrappedDevourer_…, [out] 1)` |
| event | `DialogActorJoined/4` | 10 |  | raises | `DialogActorJoined(TUT_Start_OriginIntroduction_Default_…, 1, Gnomes_Male_Deep_Player_…, 0)` |
| event | `DialogActorLeft/4` | 10 |  | raises | `DialogActorLeft(TUT_Start_OriginIntroduction_Default_…, 1, Gnomes_Male_Deep_Player_…, 1)` |
| event | `AutomatedDialogStarted/2` | 4 |  | raises | `AutomatedDialogStarted(TUT_Start_PAD_Start_…, 2)` |
| event | `DialogRollResult/5` | 4 |  | raises | `DialogRollResult(Gnomes_Male_Deep_Player_…, 0, TUT_Start_Brinepool_…, 0, CRITICALITYTYPE.NONE)` |
| event | `AutomatedDialogEnded/2` | 3 |  | raises | `AutomatedDialogEnded(TUT_Start_PAD_Start_…, 2)` |
| event | `DialogEnded/2` | 3 |  | raises | `DialogEnded(TUT_Start_OriginIntroduction_Default_…, 1)` |
| event | `DialogStarted/2` | 3 |  | raises | `DialogStarted(TUT_Start_OriginIntroduction_Default_…, 1)` |
| event | `DialogStartRequested/2` | 1 |  | external | `DialogStartRequested(S_TUT_Lab_DevourerVictim_…, Gnomes_Male_Deep_Player_…)` |

### Entities & templates

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `TriggerRegisterForCharacter/2` | 82 |  |  | `TriggerRegisterForCharacter((TRIGGER)S_Tutorials_AdvancedMovement_04_…, Gnomes_Male_Deep_Player_…)` |
| call | `RegisterWaypoint/2` | 40 |  | yes | `RegisterWaypoint("WAYP_CRE_Exterior", (ITEM)S_CRE_Exterior_Waypoint_…)` |
| call | `SetOnStage/2` | 37 |  | yes | `SetOnStage((CHARACTER)S_PLA_ZhentShipment_Spectator_…, 0)` |
| call | `SetCanInteract/2` | 8 |  | yes | `SetCanInteract((ITEM)S_TUT_Lab_OperationChair_…, 0)` |
| call | `SetDetached/2` | 3 |  | yes | `SetDetached(S_TUT_Misc_PodMindflayer_…, 1)` |
| call | `SetVisible/2` | 3 |  |  | `SetVisible(S_TUT_Misc_PodMindflayer_…, 0)` |
| call | `TriggerUnregisterForCharacter/2` | 3 |  |  | `TriggerUnregisterForCharacter(S_TUT_SaveTrigger_Start_…, Gnomes_Male_Deep_Player_…)` |
| call | `SetEntityEvent/3` | 2 |  |  | `SetEntityEvent(S_TUT_Lab_TrappedDevourer_…, "ClearPeaceReturn", 1)` |
| call | `TeleportToPosition/10` | 1 |  |  | `TeleportToPosition(S_TUT_Lab_TrappedDevourer_…, -32.0, 26.0, -354.0, "TUT_SortingChamber_UsTraveled", 0, 0, 0, 0, 0)` |
| call | `Transform/3` | 1 |  |  | `Transform(S_TUT_Lab_DevourerVictim_…, QUEST_TUT_LAB_TrappedDevourerVictim_Brainless_…, (SHAPESHIFTRULE)DISGUISE_…)` |
| query | `Exists/2` | 3047 |  | yes | `Exists(S_GLO_Halsin_…, [out] 1)` |
| query | `IsCharacter/2` | 1207 |  | yes | `IsCharacter(S_GLO_Halsin_…, [out] 1)` |
| query | `IsItem/2` | 113 |  | yes | `IsItem(S_GLO_Halsin_…, [out] 1)` |
| query | `GetTemplate/2` | 94 |  | yes | `GetTemplate(S_GLO_HollyphantDetective_…, [out] (GUIDSTRING))` |
| query | `GetDefaultState/2` | 31 |  |  | `GetDefaultState(S_GLO_BaneDummyForGortashSwD_…, [out] "")` |
| query | `IgnoreGenerics/2` | 10 |  |  | `IgnoreGenerics(S_TUT_Start_Brinepool_…, [out] 0)` |
| query | `CreateAtObject/7` | 5 |  |  | `CreateAtObject((ROOT)DLC_LOOT_Dye_Larian_…, Gnomes_Male_Deep_Player_…, 0, 0, "", 0, [out] (GUIDSTRING))` |
| query | `GetDisplayName/2` | 5 |  | yes | `GetDisplayName(Gnomes_Male_Deep_Player_…, [out] (STRING))` |
| query | `IsActive/2` | 2 |  | yes | `IsActive(Gnomes_Male_Deep_Player_…, [out] 1)` |
| query | `IsDead/2` | 2 |  |  | `IsDead(S_TUT_Lab_DevourerVictim_…, [out] 0)` |
| query | `GetIdentity/3` | 1 |  | yes | `GetIdentity(Humans_Female_Daisy_…, 0, [out] (STRING))` |
| query | `GetPosition/4` | 1 |  |  | `GetPosition((TRIGGER)S_TUT_Misc_Elevator002_Up_…, [out] (REAL), [out] (REAL), [out] (REAL))` |
| event | `EnteredTrigger/2` | 57 |  | external | `EnteredTrigger(S_Player_ShadowHeart_…, RoomTrigger_ShadowHeart_F1_…)` |
| event | `Activated/1` | 48 |  | external | `Activated(S_GLO_CharacterCreationDummy_003_…)` |
| event | `Deactivated/1` | 42 |  | external | `Deactivated(S_GLO_CharacterCreationDummy_003_…)` |
| event | `WentOnStage/2` | 34 |  | external | `WentOnStage(…, 0)` |
| event | `EntityEvent/2` | 24 |  | raises | `EntityEvent(NULL_…, "GLO_FillRaceTags_Finish")` |
| event | `LeftTrigger/2` | 11 |  | external | `LeftTrigger(Gnomes_Male_Deep_Player_…, S_TUT_SaveTrigger_Start_…)` |
| event | `OnStoryOverride/1` | 11 |  | external | `OnStoryOverride(S_TUT_Helm_MindFlayer_…)` |
| event | `AnimationEvent/3` | 1 |  | external | `AnimationEvent(S_TUT_Lab_TrappedDevourer_…, "Script_SpellFinished", 0)` |
| event | `UserEvent/2` | 1 |  | external | `UserEvent(65537, "GLO_SafeRomance_Check")` |

### Flags

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `ClearFlag/4` | 29 |  | yes | `ClearFlag((FLAG)GLO_Origin_PartOfTheTeam_Halsin_…, NULL_…, 0, 1)` |
| call | `SetFlag/4` | 25 |  | yes | `SetFlag((FLAG)Debug_CAMP_IgnoreCampTypeRestrictions_…, NULL_…, 0, 1)` |
| call | `ClearMagicPocketsFlag/3` | 20 |  | yes | `ClearMagicPocketsFlag(Gnomes_Male_Deep_Player_…, (FLAG)ORI_Karlach_State_HasSoulCoin_…, 1)` |
| call | `SetMagicPocketsFlag/3` | 8 |  | yes | `SetMagicPocketsFlag(S_Player_ShadowHeart_…, (FLAG)GLO_InfernalBox_State_CharacterHasBox_…, 1)` |
| call | `SetDoNotFaceFlag/2` | 1 |  |  | `SetDoNotFaceFlag(S_TUT_Helm_MindFlayer_…, 1)` |
| query | `GetFlag/3` | 44 |  | yes | `GetFlag((FLAG)GLO_SpeakWithDead_State_QuestionsLimitReached_…, (CHARACTER)S_Player_Minsc_…, [out] 0)` |
| query | `GetFlagName/2` | 19 |  | yes | `GetFlagName((FLAG)NIGHT_Gale_TressymJoin_…, [out] (STRING))` |
| event | `FlagSet/3` | 34 |  | raises | `FlagSet(Debug_CAMP_IgnoreCampTypeRestrictions_…, NULL_…, 0)` |

### Inventory & items

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `SendToCampChest/2` | 5 |  |  | `SendToCampChest(…, Gnomes_Male_Deep_Player_…)` |
| call | `Use/5` | 5 |  | yes | `Use((CHARACTER)S_TUT_Lab_DevourerVictim_…, (ITEM)S_TUT_Lab_OperationChair_…, 1, 0, "TUT_Lab_SitOnChair")` |
| call | `UnlockRecipe/3` | 4 |  | yes | `UnlockRecipe(Gnomes_Male_Deep_Player_…, "ALCH_Potion_Healing_RoguesMorsel", 0)` |
| call | `IterateInventoryByTemplate/4` | 1 |  |  | `IterateInventoryByTemplate((CHARACTER)S_GLO_GithKnight_…, (ITEMROOT)MAG_Primeval_Silver_Longsword_…, "GLO_CharacterBlockedFromSellingTemplate_SetNonTradeable", "")` |
| call | `PlatformMoveTo/7` | 1 |  |  | `PlatformMoveTo((LEVELTEMPLATE)LT_PLT_TUT_Elevator_002_…, -1.0, 23.5, -428.5, 1.5, "TUT_Misc_ElevatorMoved", 0)` |
| call | `SetCanTrade/2` | 1 |  |  | `SetCanTrade((CHARACTER)S_Player_Jaheira_…, 0)` |
| call | `TemplateAddTo/4` | 1 |  |  | `TemplateAddTo((ITEMROOT)LOOT_Camp_Pack_…, Gnomes_Male_Deep_Player_…, 1, 0)` |
| call | `ToInventory/5` | 1 |  |  | `ToInventory((ITEM)S_GLO_PuzzleBox_…, S_Player_ShadowHeart_…, 1, 1, 1)` |
| query | `IsWeapon/2` | 436 |  | yes | `IsWeapon(WPN_GTY_HeavyCrossbow_A_…, [out] 1)` |
| query | `GetInventoryOwner/2` | 97 | 89 | yes | `GetInventoryOwner((ITEM)S_SCL_SpidersLyre_…, [out] (GUIDSTRING))` |
| query | `TemplateIsInInventory/3` | 63 |  | yes | `TemplateIsInInventory((ITEMROOT)LOOT_SoulCoin_…, Gnomes_Male_Deep_Player_…, [out] (INTEGER))` |
| query | `ItemGetSupplyValue/2` | 15 |  |  | `ItemGetSupplyValue(Underwear_Gnomes_A_…, [out] (INTEGER))` |
| query | `GetEquippedItem/3` | 11 | 11 |  | `GetEquippedItem(Gnomes_Male_Deep_Player_…, "MusicalInstrument", [out] (GUIDSTRING))` |
| query | `CanSitOn/2` | 9 |  |  | `CanSitOn(S_TUT_TransformChamber_HackChair_…, [out] 0)` |
| query | `GetCanPickUp/2` | 9 |  |  | `GetCanPickUp(S_TUT_TransformChamber_HackChair_…, [out] 1)` |
| query | `IsLadder/2` | 7 |  |  | `IsLadder(S_TUT_Start_Brinepool_…, [out] 0)` |
| query | `IsPublicDomain/2` | 7 |  |  | `IsPublicDomain(S_TUT_Start_Brinepool_…, [out] 0)` |
| query | `IsRangedWeapon/3` | 7 |  |  | `IsRangedWeapon(Underwear_Gnomes_A_…, 1, [out] 1)` |
| query | `CharacterGetOwner/2` | 4 | 4 | yes | `CharacterGetOwner(S_GLO_CharacterCreationDummy_003_…, [out] (GUIDSTRING))` |
| query | `GetOwner/2` | 3 |  |  | `GetOwner(S_TUT_Start_Brinepool_…, [out] (GUIDSTRING))` |
| query | `IsLocked/2` | 3 |  |  | `IsLocked(S_TUT_Start_Brinepool_…, [out] 1)` |
| query | `CanTrade/2` | 1 |  |  | `CanTrade(S_WYR_Windmill_MF_…, [out] 1)` |
| query | `IsInInventoryOf/3` | 1 |  |  | `IsInInventoryOf(S_FOR_Courier_Ball_…, Gnomes_Male_Deep_Player_…, [out] 1)` |
| event | `AddedTo/3` | 708 |  | external | `AddedTo(WPN_GTY_HeavyCrossbow_A_…, S_GLO_GithKnight_…, "Regular")` |
| event | `TemplateAddedTo/4` | 708 |  | external | `TemplateAddedTo(WPN_GTY_HeavyCrossbow_A_…, WPN_GTY_HeavyCrossbow_A_…, S_GLO_GithKnight_…, "Regular")` |
| event | `Equipped/2` | 421 |  | external | `Equipped(WPN_GTY_HeavyCrossbow_A_…, S_GLO_GithKnight_…)` |
| event | `TemplateEquipped/2` | 421 |  | external | `TemplateEquipped(WPN_GTY_HeavyCrossbow_A_…, S_GLO_GithKnight_…)` |
| event | `EquipFailed/2` | 81 |  | external | `EquipFailed(LOOT_SCROLL_Revivify_…, S_TUT_GithGuide_…)` |
| event | `StackedWith/2` | 36 |  | external | `StackedWith(LOOT_SCROLL_Revivify_…, LOOT_SCROLL_Revivify_…)` |
| event | `TemplateUseFinished/4` | 12 |  | raises | `TemplateUseFinished(S_TUT_TransformChamber_DrainVictim_001_…, BLD_MF_Nautiloid_Mind_Draining_Device_Bed_…, S_TUT_TransformChamber_DrainBed_001_…, 1)` |
| event | `TemplateUseStarted/3` | 12 |  | raises | `TemplateUseStarted(S_TUT_TransformChamber_DrainVictim_001_…, BLD_MF_Nautiloid_Mind_Draining_Device_Bed_…, S_TUT_TransformChamber_DrainBed_001_…)` |
| event | `UseFinished/3` | 12 |  | raises | `UseFinished(S_TUT_TransformChamber_DrainVictim_001_…, S_TUT_TransformChamber_DrainBed_001_…, 1)` |
| event | `UseStarted/2` | 12 |  | raises | `UseStarted(S_TUT_TransformChamber_DrainVictim_001_…, S_TUT_TransformChamber_DrainBed_001_…)` |
| event | `RequestCanUse/3` | 9 |  | external | `RequestCanUse(S_Player_ShadowHeart_…, S_TUT_TransformChamber_HackChair_…, 5)` |
| event | `GoldChanged/2` | 8 |  | external | `GoldChanged(S_DEN_CharmedKid_…, 15)` |
| event | `UnlockedRecipe/2` | 4 |  | external | `UnlockedRecipe(Gnomes_Male_Deep_Player_…, "ALCH_Potion_Healing_RoguesMorsel")` |
| event | `Opened/1` | 3 |  | external | `Opened(S_TUT_Start_Door_001_…)` |
| event | `TemplateOpening/3` | 3 |  | external | `TemplateOpening(DOOR_MF_Nautiloid_Organic_A_…, S_TUT_Start_Door_001_…, Gnomes_Male_Deep_Player_…)` |
| event | `Closed/1` | 2 |  | external | `Closed(S_TUT_Start_Door_005_…)` |
| event | `DoorTemplateClosing/3` | 2 | 2 | external | `DoorTemplateClosing/3` |
| event | `InventoryBoundChanged/2` | 1 |  | external | `InventoryBoundChanged(S_GLO_PuzzleBox_…, 1)` |
| event | `PlatformMovementFinished/2` | 1 |  | external | `PlatformMovementFinished(PLT_TUT_Elevator_…, "TUT_Misc_ElevatorMoved")` |

### Levels & game flow

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `MapMarkerChangeLevel/2` | 3 |  | yes | `MapMarkerChangeLevel("GLO_CrownController_Ketheric", "TUT_Avernus_C")` |
| call | `AutoSave/0` | 1 |  | yes | `AutoSave()` |
| call | `ShowActivity/1` | 1 |  |  | `ShowActivity("Tutorial")` |
| call | `StartActivity/1` | 1 |  |  | `StartActivity("Tutorial")` |
| call | `TeleportPartiesToLevelWithMovie/3` | 1 |  | yes | `TeleportPartiesToLevelWithMovie("TUT_Avernus_C", "", "GUS_CGI01_Part2")` |
| query | `GetRegion/2` | 182 |  | yes | `GetRegion((ITEM)S_SCL_SpidersLyre_…, [out] "SYS_CC_I")` |
| query | `IsGameLevel/2` | 33 |  | yes | `IsGameLevel("SYS_CC_I", [out] 1)` |
| query | `CheckRulesetModifierBool/3` | 6 |  | yes | `CheckRulesetModifierBool((RULESETMODIFIER)HONOUR_STATS_…, 1, [out] 1)` |
| query | `IsGameStateRunning/1` | 3 |  |  | `IsGameStateRunning([out] 0)` |
| query | `GetActiveModStartupLevel/1` | 1 |  | yes | `GetActiveModStartupLevel([out] (STRING))` |
| query | `GetModuleVersion/5` | 1 |  |  | `GetModuleVersion("GustavX", [out] (INTEGER), [out] (INTEGER), [out] (INTEGER), [out] (INTEGER))` |
| event | `EnteredLevel/3` | 330 |  | external | `EnteredLevel(S_OriginIntro_DarkUrgeDummy_…, …, "SYS_CC_I")` |
| event | `LevelTemplateLoaded/1` | 13 |  | external | `LevelTemplateLoaded(…)` |
| event | `LeftLevel/2` | 3 |  | external | `LeftLevel(Gnomes_Male_Deep_Player_…, "SYS_CC_I")` |
| event | `LevelGameplayReady/2` | 2 |  | external | `LevelGameplayReady("SYS_CC_I", 0)` |
| event | `LevelGameplayStarted/2` | 2 |  | external | `LevelGameplayStarted("SYS_CC_I", 0)` |
| event | `LevelLoaded/1` | 2 |  | external | `LevelLoaded("SYS_CC_I")` |
| event | `MovieFinished/1` | 2 |  | external | `MovieFinished("GUS_CGI01_Part1")` |
| event | `MoviePlaylistFinished/1` | 2 | 2 | external | `MoviePlaylistFinished/1` |
| event | `GameModeStarted/3` | 1 |  | external | `GameModeStarted("Campaign", 0, 0)` |
| event | `LevelUnloading/1` | 1 |  | external | `LevelUnloading("SYS_CC_I")` |

### Math & strings

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| query | `IntegerSum/3` | 551 |  | yes | `IntegerSum(1, 1, [out] (INTEGER))` |
| query | `Concatenate/3` | 39 |  | yes | `Concatenate("Gale", "RecruitAvatar", [out] (STRING))` |
| query | `IntegerSubtract/3` | 11 |  | yes | `IntegerSubtract(1, 1, [out] (INTEGER))` |
| query | `Random/2` | 8 |  |  | `Random(10000, [out] (INTEGER))` |
| query | `ConcatenateInteger/3` | 6 |  | yes | `ConcatenateInteger("StuckTimelineLoad_Detector_TUT_Start_OriginIntroduction_Default_…", 1, [out] (STRING))` |
| query | `GUIDToString/2` | 6 |  | yes | `GUIDToString(TUT_Start_OriginIntroduction_Default_…, [out] (STRING))` |
| query | `GetRulesetModifierString/2` | 6 |  | yes | `GetRulesetModifierString((RULESETMODIFIER)SCRIPTED_COMBAT_MECHANICS_…, [out] (STRING))` |
| query | `CheckRulesetModifierString/3` | 4 |  | yes | `CheckRulesetModifierString((RULESETMODIFIER)SCRIPTED_COMBAT_MECHANICS_…, "HARD", [out] 1)` |
| query | `IntegerProduct/3` | 4 |  | yes | `IntegerProduct(7026, 50, [out] (INTEGER))` |

### Movement

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `TeleportTo/8` | 5 |  |  | `TeleportTo(S_TUT_TransformChamber_Pod_007_…, S_TUT_ShadowheartPodPoint_…, "", 1, 0, 1, 0, 1)` |
| call | `CharacterMoveTo/5` | 2 |  |  | `CharacterMoveTo(S_TUT_Lab_TrappedDevourer_…, S_TUT_UsEscapePosition_Leave_…, "Run", "TUT_SortingChamber_UsReadyToLeave", 1)` |
| event | `Teleported/9` | 5 |  | external | `Teleported(Gnomes_Male_Deep_Player_…, NULL_…, 0.0, 0.0, 0.0, 53.1, 17.9, -426.5, "RegionSwap")` |
| event | `ItemTeleported/7` | 1 |  | external | `ItemTeleported(S_TUT_TransformChamber_Pod_007_…, 1.0, 17.5, -343.4, 1.0, 17.4, -343.9)` |
| event | `MovedBy/2` | 1 |  | external | `MovedBy(S_TUT_Lab_TrappedDevourer_…, S_TUT_Lab_TrappedDevourer_…)` |

### Other

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `SetForceUpdate/2` | 10 |  | yes | `SetForceUpdate((ITEM)S_TUT_Misc_Tentacle_000_…, 1)` |
| call | `HideAllActivities/1` | 1 |  |  | `HideAllActivities("Progression")` |

### Party & users

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `MakeNPC/1` | 14 |  | yes | `MakeNPC((CHARACTER)S_GLO_CharacterCreationDummy_001_…)` |
| call | `RequestProcessed/3` | 9 |  |  | `RequestProcessed(S_Player_ShadowHeart_…, 5, 1)` |
| call | `Follow/2` | 1 |  |  | `Follow((CHARACTER)S_TUT_Lab_TrappedDevourer_…, (CHARACTER)S_TUT_Lab_IntellectDevourer_…)` |
| call | `IterateUsers/2` | 1 |  |  | `IterateUsers("GLO_SafeRomance_Check", "")` |
| call | `StopFollow/1` | 1 | 1 |  | `StopFollow((CHARACTER)S_TUT_Lab_TrappedDevourer_…)` |
| query | `IsSummon/2` | 358 |  | yes | `IsSummon(S_GLO_CharacterCreationDummy_003_…, [out] 1)` |
| query | `CharacterHasDLC/3` | 66 |  | yes | `CharacterHasDLC(Gnomes_Male_Deep_Player_…, (DLC)DLC_Gustav_DigitalDeluxe_…, [out] 1)` |
| query | `GetReservedUserID/2` | 64 |  |  | `GetReservedUserID((CHARACTER)S_GLO_CharacterCreationDummy_001_…, [out] (INTEGER))` |
| query | `IsPartyMember/3` | 53 |  | yes | `IsPartyMember(S_GLO_BaneDummyForGortashSwD_…, 1, [out] 0)` |
| query | `IsPartyFollower/2` | 14 |  | yes | `IsPartyFollower(S_GLO_CharacterCreationDummy_003_…, [out] 1)` |
| query | `GetUserProfileID/2` | 11 |  |  | `GetUserProfileID(65537, [out] (STRING))` |
| query | `GetHostCharacter/1` | 6 |  | yes | `GetHostCharacter([out] (unknown))` |
| query | `GetMaxPartySize/1` | 5 |  | yes | `GetMaxPartySize([out] (INTEGER))` |
| query | `IsPlayer/2` | 4 |  |  | `IsPlayer(S_GLO_CharacterCreationDummy_001_…, [out] 0)` |
| query | `IsControlled/2` | 3 |  |  | `IsControlled(Gnomes_Male_Deep_Player_…, [out] 0)` |
| query | `GetAttitudeTowardsPlayer/3` | 1 |  |  | `GetAttitudeTowardsPlayer(S_TUT_Lab_DevourerVictim_…, Gnomes_Male_Deep_Player_…, [out] (INTEGER))` |
| query | `UserGetSafeRomanceOption/2` | 1 |  |  | `UserGetSafeRomanceOption(65537, [out] (INTEGER))` |
| event | `CharacterJoinedParty/1` | 5 |  | external | `CharacterJoinedParty(S_GLO_CharacterCreationDummy_003_…)` |
| event | `CharacterReservedUserIDChanged/3` | 5 |  | external | `CharacterReservedUserIDChanged(Gnomes_Male_Deep_Player_…, -65536, 65537)` |
| event | `AttachedToPartyGroup/1` | 4 |  | external | `AttachedToPartyGroup(S_GLO_CharacterCreationDummy_001_…)` |
| event | `CharacterLeftParty/1` | 4 |  | external | `CharacterLeftParty(S_GLO_CharacterCreationDummy_001_…)` |
| event | `GainedControl/1` | 2 |  | external | `GainedControl(S_GLO_CharacterCreationDummy_003_…)` |
| event | `CharacterMadePlayer/1` | 1 |  | external | `CharacterMadePlayer(Gnomes_Male_Deep_Player_…)` |
| event | `UserCampChestChanged/2` | 1 |  | external | `UserCampChestChanged(65537, CONT_PlayerCampChest_A_…)` |

### Presentation

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `LookAtEntity/3` | 2 |  | yes | `LookAtEntity((CHARACTER)S_TUT_Helm_MindFlayer_…, (CHARACTER)S_TUT_Helm_Devil_005_…, 3.0)` |
| call | `StopLoopEffect/1` | 2 |  |  | `StopLoopEffect((INTEGER64)144115192370840100)` |
| call | `PlayHUDSound/2` | 1 |  |  | `PlayHUDSound(Gnomes_Male_Deep_Player_…, "TUT_Start_MuteSound")` |
| call | `SetTadpoleTreeState/2` | 1 |  |  | `SetTadpoleTreeState(Gnomes_Male_Deep_Player_…, 0)` |
| call | `TriggerSetSoundState/4` | 1 |  |  | `TriggerSetSoundState((TRIGGER)Amb_SV_TUT_Avernus_QD_…, "State_Mix_Level", "TUT_Avernus", 1)` |
| query | `PlayLoopEffect/5` | 3 |  |  | `PlayLoopEffect(S_TUT_TransformChamber_Pod_007_…, VFX_Script_Nautiloid_Pod_Warding_Rune_01_…, "", 1.0, [out] (INTEGER64))` |

### Quests & journal

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `AddEntryToCustomBook/2` | 3 |  | yes | `AddEntryToCustomBook("ORI_Laezel_OrpheusChapter1", "ORI_Laezel_OrpheusChapter1")` |
| call | `QuestUpdate/3` | 1 |  |  | `QuestUpdate(Gnomes_Male_Deep_Player_…, "TUT_NautiloidEscape", "Start")` |
| event | `QuestAccepted/2` | 1 |  | external | `QuestAccepted(Gnomes_Male_Deep_Player_…, "TUT_NautiloidEscape")` |
| event | `QuestUpdateUnlocked/3` | 1 |  | external | `QuestUpdateUnlocked(Gnomes_Male_Deep_Player_…, "TUT_NautiloidEscape", "Start")` |

### Statuses

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `ApplyStatus/5` | 32 |  | yes | `ApplyStatus(S_GLO_CursedIronFlask_…, "GLO_CURSEDIRONFLASK_GLOW", -1.0, 0, NULL_…)` |
| call | `RemoveStatus/3` | 5 |  | yes | `RemoveStatus(Gnomes_Male_Deep_Player_…, "HEALTHBOOST_ALLIEDNPC_EASYMODE", NULL_…)` |
| query | `IsStatusFromGroup/3` | 2578 |  | yes | `IsStatusFromGroup("SKELETON_DEATH", "SG_DropForNonMutingDialog", [out] 1)` |
| query | `HasActiveStatus/3` | 1507 |  | yes | `HasActiveStatus(WPN_GTY_HeavyCrossbow_A_…, "WYR_GORTASH_GRENADE_DETONATE", [out] 1)` |
| query | `GetStatusType/2` | 420 |  | yes | `GetStatusType("SKELETON_DEATH", [out] "POLYMORPHED")` |
| query | `HasAppliedStatusWithGroup/3` | 20 |  | yes | `HasAppliedStatusWithGroup(Gnomes_Male_Deep_Player_…, "SG_DropForNonMutingDialog", [out] 1)` |
| query | `HasActiveStatusWithGroup/3` | 4 |  | yes | `HasActiveStatusWithGroup(Gnomes_Male_Deep_Player_…, "SG_Invisible", [out] 1)` |
| query | `HasAppliedStatus/3` | 3 |  | yes | `HasAppliedStatus(S_GLO_PuzzleBox_…, "GLO_INFERNALBOX_BOUNDEDTO", [out] 0)` |
| event | `StatusAttempt/4` | 211 |  | raises | `StatusAttempt(S_GLO_JergalAvatar_…, "SKELETON_DEATH", …, 0)` |
| event | `StatusApplied/4` | 210 |  | raises | `StatusApplied(S_GLO_JergalAvatar_…, "SKELETON_DEATH", …, 0)` |
| event | `StatusRemoved/4` | 7 |  | raises | `StatusRemoved(Gnomes_Male_Deep_Player_…, "TRAVELBLOCK_CANTMOVE", …, -1)` |
| event | `StatusAttemptFailed/4` | 6 |  | external | `StatusAttemptFailed(Helm_Imp_Dead_03_…, "ENABLE_AOO", …, 0)` |
| event | `ReposeAdded/2` | 5 |  | external | `ReposeAdded(S_TUT_TransformChamber_DrainVictim_001_…, S_TUT_TransformChamber_DrainBed_001_…)` |
| event | `StatusTagSet/5` | 1 | 1 | external | `StatusTagSet/5` |

### Tags

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `SetTag/2` | 272 |  | yes | `SetTag((CHARACTER)S_GLO_Hirelings_Barbarian_…, (TAG)PLAYABLE_…)` |
| call | `AddPreferredAiTargetTag/2` | 22 |  | yes | `AddPreferredAiTargetTag((CHARACTER)S_TUT_Helm_DevilBackUp_001_…, (TAG)AVATAR_…)` |
| call | `IterateOriginTags/3` | 11 |  | yes | `IterateOriginTags((CHARACTER)S_Player_Karlach_…, "GLO_OriginTag", "GLO_DataGetOriginTags_Finish")` |
| call | `ClearTag/2` | 6 |  | yes | `ClearTag(Gnomes_Male_Deep_Player_…, (TAG)DAISY_REASONTRACK_…)` |
| call | `IterateTagsCategory/3` | 2 |  | yes | `IterateTagsCategory(TAGCATEGORY.RACE, "GLO_RaceTag", "GLO_FillRaceTags_Finish")` |
| query | `IsTagged/3` | 1360 |  | yes | `IsTagged((CHARACTER)S_GLO_Hirelings_Barbarian_…, (TAG)BLOCK_RESURRECTION_…, [out] 0)` |
| query | `DataGetBackgroundTagAt/3` | 11 |  | yes | `DataGetBackgroundTagAt((CHARACTER)S_Player_Karlach_…, 1, [out] (GUIDSTRING))` |
| query | `DataGetBackgroundTagCount/2` | 11 |  | yes | `DataGetBackgroundTagCount((CHARACTER)S_Player_Karlach_…, [out] (INTEGER))` |
| query | `DataGetGodTagCount/2` | 11 | 10 | yes | `DataGetGodTagCount((CHARACTER)S_Player_Karlach_…, [out] (INTEGER))` |
| query | `DataGetReallyTagAt/3` | 11 |  | yes | `DataGetReallyTagAt((CHARACTER)S_Player_Karlach_…, 1, [out] (GUIDSTRING))` |
| query | `DataGetReallyTagCount/2` | 11 |  | yes | `DataGetReallyTagCount((CHARACTER)S_Player_Karlach_…, [out] (INTEGER))` |
| query | `DataGetGodTagAt/3` | 2 |  | yes | `DataGetGodTagAt((CHARACTER)S_Player_ShadowHeart_…, 1, [out] (GUIDSTRING))` |
| query | `SatisfiesTagExpression/3` | 1 |  |  | `SatisfiesTagExpression(Gnomes_Male_Deep_Player_…, "CAT_… \| RAVEN_… \| RAT_… \| FROG_… \| CRAB_… \| (SUMMON_… & SPIDER_…) \| GASEOUS_FORM_… \| WYR_CIRCUS_CHEESE_… \| INTELLECT_DEVOURER_…", [out] 1)` |
| event | `TagSet/2` | 6673 |  | raises | `TagSet(S_CRE_EntrypointFromSCL02_…, …)` |
| event | `TagEvent/2` | 103 |  | raises | `TagEvent(REALLY_ZARIELTIEFLING_…, "GLO_RaceTag")` |
| event | `CharacterTagEvent/3` | 10 |  | raises | `CharacterTagEvent(S_Player_Karlach_…, KARLACH_…, "GLO_OriginTag")` |
| event | `TagCleared/2` | 4 |  | raises | `TagCleared(S_GLO_DevilishOx_…, …)` |

### Timers

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `TimerLaunch/2` | 22 |  | yes | `TimerLaunch("CheckDismissableAvatars-Framedelay", 0)` |
| call | `ObjectTimerLaunch/4` | 21 |  | yes | `ObjectTimerLaunch((CHARACTER)S_Player_Karlach_…, "GLO_SetGameStartTags", 0, 1)` |
| call | `ObjectTimerCancel/2` | 11 |  | yes | `ObjectTimerCancel((ITEM)S_TUT_Misc_RubbleHelper_000_…, "TUT_Misc_RubbleDelay")` |
| call | `RealtimeObjectTimerLaunch/3` | 10 |  | yes | `RealtimeObjectTimerLaunch(Gnomes_Male_Deep_Player_…, "GLO_Tutorials_TemporaryEquipmentTutSuppression", 3000)` |
| call | `TimerCancel/1` | 9 |  | yes | `TimerCancel("GLO_DLC_Check")` |
| call | `RealtimeObjectTimerCancel/2` | 6 |  | yes | `RealtimeObjectTimerCancel(S_TUT_Lab_TrappedDevourer_…, "GEB_PerformWhenAvailable_StopForceActivate")` |
| event | `ObjectTimerFinished/2` | 26 |  | raises | `ObjectTimerFinished(S_Player_Karlach_…, "GLO_SetGameStartTags")` |
| event | `TimerFinished/1` | 16 |  | raises | `TimerFinished("CheckDismissableAvatars-Framedelay")` |

### Tutorials

| | Function | Count | Failed / ignored | Sim | Example |
|---|---|---:|---:|---|---|
| call | `EnableTutorialEvent/2` | 34 |  | yes | `EnableTutorialEvent(Gnomes_Male_Deep_Player_…, (TUTORIALEVENT)AttackHit_Immunity_…)` |
| call | `HideTutorial/2` | 16 |  |  | `HideTutorial(Gnomes_Male_Deep_Player_…, (UNIFIEDTUTORIAL)Camera_Movement_…)` |
| call | `SetTutorialShown/2` | 7 |  |  | `SetTutorialShown(Gnomes_Male_Deep_Player_…, (UNIFIEDTUTORIAL)Camera_Movement_…)` |
| call | `DisableTutorialEvent/2` | 5 |  |  | `DisableTutorialEvent(Gnomes_Male_Deep_Player_…, (TUTORIALEVENT)UserAction_CameraTrackPlayer_…)` |
| call | `ShowTutorial/2` | 5 |  |  | `ShowTutorial(Gnomes_Male_Deep_Player_…, (UNIFIEDTUTORIAL)Inventory_Containers_…)` |
| call | `ResetTutorialsRequest/2` | 4 |  |  | `ResetTutorialsRequest(65537, 1)` |
| query | `WasTutorialCompletedForUser/2` | 4 |  |  | `WasTutorialCompletedForUser(65537, [out] (INTEGER))` |
| query | `IsAnyTutorialShowingFor/2` | 2 |  |  | `IsAnyTutorialShowingFor(Gnomes_Male_Deep_Player_…, [out] 0)` |
| event | `TutorialEvent/2` | 7 |  | external | `TutorialEvent(Gnomes_Male_Deep_Player_…, …)` |
| event | `TutorialClosed/2` | 1 |  | external | `TutorialClosed(65537, …)` |

