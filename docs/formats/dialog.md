# Dialogs (`Story/DialogsBinary/**/*.lsf`)

Implemented in `crates/bg3-dialog`: the model and loader (`model.rs`), dialog resources and
lookup (`library.rs`), definition resources (`defs.rs`), the runner (`runner.rs`) and the game
state interface (`world.rs`). Tools: `bg3x dialog <name> [--walk]` and `bg3-viewer --dialog <name>`.

Sources: the files of the reference build (4.1.1.7631656), and a static reading of the
`LSDialogs::*`, `eoc::*RollNode*`, `esv::EoCDialogGameInterface`, `esv::OsirisVariableHelper`
and `esv::StoryDialogEventManager` code in the macOS arm64 binary, for understanding only.
Symbols are cited below and in the code. Behaviour was checked against a runtime log of the
original game (see [Verification](#verification)). Statements are **confirmed** (read in the
code or checked on all files), **inferred**, or **open**.

## Files

| What | Where | Count |
|------|-------|------:|
| Compiled dialogs (what the game loads) | `Mods/<module>/Story/DialogsBinary/**/*.lsf` (`Gustav`, `GustavDev`, `Shared`, …) | 9 386 |
| Editor sources | `Mods/<module>/Story/Dialogs/**/*.lsj` | ~9 300 |
| Dialog resources | `DialogBank` regions in `Public/<module>/Content/Assets/Dialogs/**/_merged.lsf` | 9 543 unique ids |
| Flag definitions | `Public/<module>/Flags/<uuid>.lsf` | 26 582 |
| Tag definitions | `Public/<module>/Tags/<uuid>.lsf` | 1 103 |
| Script flags | `Mods/<module>/Story/Dialogs/ScriptFlags/ScriptFlags.lsx` | 86 |
| Dialog variables | `Mods/<module>/Story/Dialogs/DialogVariables/DialogVariables.lsx` | |
| Speaker groups | `Public/<module>/Voice/SpeakerGroups.lsf` | 273 |
| Timelines (cinematics) | `Public/<module>/Timeline/Generated/<name>.lsf` (+ `_Scene.lsx`, `_Prefetch.lsf`); `TimelineBank` resources | not parsed |

The `.lsj` sources are the editor's JSON (see [lsf.md](lsf.md)). They hold the same nodes plus
editor-only data (an `editorData` region with synopsis, repro steps, per-speaker attitudes; node
`editorData` such as animation tags and stage directions), and group children differently. The
compiled `.lsf` keeps only what the game reads, plus `CinematicNodeContext` stage notes. All
GUIDs and handles of the compiled files appear in their sources (`bg3_lsf`'s
`dialog_lsf_ids_match_lsj`). The runtime only reads the compiled form (`GetBinaryDialogPath`
below); whether it ever falls back to the `.lsj` is open.

### Dialog resources: identity and lookup

The game names a dialog by its **resource**, not its file. A `DialogBank` resource has:

| Attribute | Meaning |
|-----------|---------|
| `ID` | resource id: the GUID in Osiris' `DIALOGRESOURCE` values (`TUT_Start_Brinepool_279b424b-…`) and what nested-dialog nodes refer to |
| `Name` | the name part of the Osiris value |
| `SourceFile` | the `.lsj` path; the compiled file is this path with `/Story/Dialogs/` → `/Story/DialogsBinary/` and the extension → `.lsf` (`LSDialogs::DialogManager::GetBinaryDialogPath`, confirmed) |
| `automated`, `isWorld`, `isBehaviour`, `isSubbedDialog`, `isPrivateDialog`, `EnableTimeline`, `AllowDeadSpeakers`, `IsAllowingJoinCombat`, `BlockFTB` | switches copied into the dialog instance (`DialogInstance::DialogInstance(unsigned, DialogResource const*)`) |
| `childResources/Object` | the nested dialogs this dialog can play |
| `SpeakerSlotsWithLines/HasLines` | per slot: whether it has voiced lines |

The dialog file's own `UUID` is not its identity anywhere we found. In the reference build, 155
bank entries are stale (neither compiled file nor source exists), 50 compiled files have no
resource, and every one of the 614 `Nested Dialog` references names a known resource that the
parent lists in `childResources` (test `dialog_resources_resolve`). The banks' `automated` agrees
with the dialogs' own `automated` attribute everywhere.

## Compiled dialog layout

One region, `dialog`. All 9 386 files load with every child, jump, alias and root reference
resolved (`parse_every_dialog_in_install`).

```
dialog                     UUID, TimelineId, category, automated, … (below)
├─ DefaultAddressedSpeakers / Object {MapKey: i32, MapValue: i32}*
├─ speakerlist / speaker {index: FixedString "0", list: LSString, SpeakerMappingId: guid,
│                         IsPeanutSpeaker?, SpeakerTagsIndex?}*
└─ nodes
   ├─ node {constructor, UUID, speaker, …}*      (key UUID)
   └─ RootNodes {RootNodes: FixedString uuid}*   (start candidates, in order)
```

Dialog attributes (absent = false): `category` (`Generic NPC Dialog`, `Automated NPC Dialog`,
`Repeated automated NPC Dialog`, `Voice bark`, `Companion Dialog`, `Reflection Dialog`,
`PVP Dialog`), `TimelineId` (the staging timeline; 89 files have none, 4 the zero GUID),
`automated`, `AllowDeadSpeakers`, `IsAllowingJoinCombat`, `IsBehaviour`, `IsPrivateDialog`,
`IsSubbedDialog`, `IsSubsAnonymous`, `IsWorld`, `issfxdialog`, `DefaultSpeakerIndex`. No
`AllowGroupChat`-like switch exists in the compiled files; joining behaviour comes from the
resource (`isPrivateDialog`, `IsAllowingJoinCombat`) and the engine.

**Speakers.** `index` is the slot number (stored as a string). `list` is one GUID (98 files have
`;`-separated lists): a character/item UUID or a **speaker group** (`SpeakerGroups.lsf`, e.g.
`GROUP_Players` e0d1ff71-…). The engine stores it as one string and matches groups by substring
(`Dialog::Visit`, confirmed). `Dialog::GetPlayersSpeakerIndex` is the first slot containing
`GROUP_Players`. `Dialog::GetDefaultSpeakerIndex`: `DefaultSpeakerIndex` if ≥ 0, else the first
slot containing `GROUP_Players`, else the first containing `GROUP_PartyMembers` or `GROUP_ORI_DU`,
else −1 (confirmed). `DefaultAddressedSpeakers` maps a slot to the slot it addresses (−1 none;
inferred from values). `SpeakerMappingId` and `IsPeanutSpeaker` (minor ambient speakers) are not
analysed further.

### Nodes

Common attributes: `constructor`, `UUID`, `speaker` (slot; −1 none, **−666 narrator**),
`addressedspeaker`, `Root`, `Greeting`, `endnode`, `endAllDialogs`, `ShowOnce`, `optional`,
`exclusive`, `stub`, `suppresssubtitle`, `gameplaynode`, `AllowNodeGrouping` (default true),
`GroupID`/`GroupIndex`, `transitionmode` (0/1/2), `waittime`, `ApprovalRatingID`,
`OverrideApprovalRatingID`, `CameraTarget`, `SoundEvent`, `CustomMovie`, `ExtraWaitTime`,
`OriginTheme`. In the engine the booleans are bits of one u16 (`DialogNode::Visit`: bit 0
endnode, 1 optional, 2 exclusive, 3 ShowOnce, 4 gameplaynode, 5 Root, 6 suppresssubtitle,
7 Greeting, 8 AllowNodeGrouping, 9 endAllDialogs; confirmed).

Common children: `children/child {UUID}*` (order matters), `checkflags`, `setflags`, `Tags/Tag`
(dialog tags on the node, e.g. the visual tag shown with a choice), `GameData` (AI
personalities, music instrument sounds, origin sound, `LookAts {Speaker, Target}`), `editorData`.

| Constructor | Count | Engine class | Extra data | Role |
|-------------|------:|--------------|------------|------|
| `TagGreeting` | 26 633 | `TaggedGreetingNode` | `TaggedTexts` | NPC line, usually a root |
| `TagAnswer` | 84 202 | `TaggedAnswerNode` | `TaggedTexts` | NPC/narrator line |
| `TagQuestion` | 35 519 | `TaggedQuestionNode` | `TaggedTexts`, `ValidatedFlags` | player option |
| `TagCinematic` | 2 959 | `CinematicTaggedNode` | `BehaviourNodeType` | staged moment, usually no text |
| `Alias` | 7 105 | `AliasNode` | `SourceNode` | stands in for another node |
| `Jump` | 19 266 | `JumpNode` | `jumptarget`, `jumptargetpoint` (1/2) | continue elsewhere |
| `Pop` | 67 | `esv::PopNode` | `PopLevel` | go back up hub levels |
| `Nested Dialog` | 568 | `NestedDialogNode` | `NestedDialogNodeUUID` (resource id), `SpeakerLinking/SpeakerLinkingEntry {Key, Value}` | play another dialog |
| `Visual State` | 1 332 | `VisualStateNode` | | staging marker |
| `ActiveRoll` | 3 446 | `eoc::ActiveRollNode` (a question) | roll data, `TaggedTexts` | player-initiated check |
| `PassiveRoll` | 365 | `eoc::PassiveRollNode` | roll data | automatic check |
| `RollResult` | 5 480 | `eoc::RollResultNode` | `Success` | branch of a roll |
| `FallibleQuestionResult` | 64 | `FallibleQuestionResultNode` | `Result` | branch of a validated question |
| `Trade` | 207 | | `TradeMode`, `ItemsTagFilter`, `PlayerOverride` | opens trading |

The binary also registers `SelectSpeakerNode`, which no file uses.

**Texts.** `TaggedTexts/TaggedText {HasTagRule, RuleGroup {TagCombineOp, Rules/Rule {TagCombineOp,
speaker, HasChildRules, Tags/Tag {Object: guid}}}, TagTexts/TagText {TagText: TranslatedString,
LineId: guid, stub, OldText?, CustomSequenceId?, DialogVariables?}}`. `TagText` is a loca handle
(`bg3x loca <handle>`); `LineId` ties the line to voice and timeline; `DialogVariables` lists
variable UUIDs from `DialogVariables.lsx` (`[1]`-style placeholders in the text; Osiris fills them
with `DialogSetVariable*`). A few `TagText`s are `TranslatedFSString`s.

**Rolls** (`ActiveRoll`/`PassiveRoll`): `RollType` (`SkillCheck`, `RawAbility`, `SavingThrow`,
`Damage`, `Melee…Attack`, or empty), `Ability`, `Skill`, `DifficultyClassID` (a
`Public/Shared/DifficultyClasses/DifficultyClasses.lsx` entry), `Advantage` (0 none, 1
advantage, 2 disadvantage; combined with the roller's own advantage, both → none),
`AdvantageReason` (loca), `RollTargetSpeaker`, `ExcludeCompanionsOptionalBonuses`,
`ExcludeSpeakerOptionalBonuses`, `ExcludedBonusesTags`, `IsFlatSkillCheck` (passive),
`DetectThoughts`, `ShowOnce`.

### Flags

`checkflags`, `setflags` and `ValidatedFlags {ValidatedHasValue}` hold
`flaggroup {type} / flag {UUID: FixedString, value: bool, paramval?: i32}`.

| Group type | Refers to | `paramval` | Usage of the flag resource |
|------------|-----------|------------|----------------------------|
| `Global` | flag | none | 5 |
| `Object` | flag | speaker slot | 4 |
| `Dialog` | flag | speaker slot | 6 |
| `Local` | flag | none | 1 |
| `User` | flag | speaker slot | 2 |
| `Party` | flag | speaker slot | 3 |
| `UserShape` | flag | speaker slot | 7 |
| `Tag` | tag | speaker slot | — |
| `Script` | script flag | none | — |
| `Quest` | quest step | speaker slot | — |

The flag resource's `Usage` byte is the engine's `EFlagType` (names table
`ls::anubis::game::_Enum_EFlagType::NAMES`): 0 Invalid, 1 Local, 2 User, 3 Party, 4 Character,
5 Global, 6 Dialog, 7 UserShape, 8 DialogInstance. Every one of the 128 231 flag references in
the reference build uses a flag whose `Usage` matches its group type (30 reference undefined
ids; test `flag_usage_matches_group_type`). A few `paramval`s are −1 (no speaker).

**Script flags** are condition scripts: `CONDITION Keyword(args)` lines and `CHECK "c1|c2"`.
Keywords in the reference build: `IsFlag(slot, flag, value)`, `IsGameObjectFlag`, `IsLocalFlag`,
`IsTagged(slot, tag)`, `IsSpeakerPresent`, `CanCastSpell`, `IsSpellActive`, `IsMagicPocketsGold`,
`HasItemTemplate`, `HasItemTemplateInMagicPockets`, `HasItemTagInMagicPockets`,
`CheckNumberOfPartyPeanuts`, `IntegerCompare`; `%Name_uuid` arguments are dialog variables;
CHECK expressions are `c1` or ORs of conditions.

**Tags** (`Public/*/Tags`): `UUID`, `Name`, `Description`, `DisplayName`, `DisplayDescription`,
`Icon`, `Categories/Category {Name}` (e.g. `Dialog`, `DialogHidden`, `Race`, `Class`, `Story`),
`Properties/Property {Name}`.

## Runtime behaviour

What `bg3_dialog::DialogRunner` implements. Cited functions are in `LSDialogs::` unless noted.

### Start

`DefaultDialogNodeLogic::SelectStartNode` → `GetPossibleStartNodes`: the roots in `RootNodes`
order (without `RootNodes`: nodes with `Root`, in file order); each is checked with its
`IsValid` (below). Valid roots are collected; the first valid `exclusive` root replaces the
collection and only further exclusive roots are added. Invalid `optional` roots are a fallback.
No candidate → the instance stops. `PickNodeToStart`: if the first candidate needs input, all
candidates are offered as choices, else the first is started. There is no randomness. (World and
behaviour dialogs instead prefer the last `TagCinematic` with `BehaviourNodeType` 0, then 1;
not implemented.)

### Validity (`IsValid`)

- Text nodes (`DefaultDialogNode::IsValid`, `BaseTaggedNode::IsValid`): the speaker slot must be
  filled (narrator and cinematic nodes excepted), `checkflags` must hold, a `ShowOnce` node must
  not be in the instance's history, and at least one `TaggedText` must match (or none exist).
- `Alias`: its **own** check-flags and `ShowOnce`, the **source's** speaker and tag rules.
- `Jump`, `Visual State` (`DialogNode::IsValid`): only `ShowOnce`.
- `RollResult`: only "the last roll's outcome == `Success`". `FallibleQuestionResult`: only
  "the question's validation outcome == `Result`".

### Choosing what comes next

`DefaultDialogNodeLogic::StepInstance` → `SelectNextDialogNode`, after a node ends:

1. A node with `endnode` ends the dialog (or, inside a nested dialog, returns from it; with
   `endAllDialogs`, ends everything). Its children are ignored.
2. The children are expanded: a `Jump` is replaced by its target (`jumptargetpoint` 1) or the
   target's children (2), recursively, without duplicates (`ResolveReferenceNodes`,
   `JumpNode::GetReferencedNodes`). A jump's own flags are never evaluated or applied.
3. Each child, in order, is checked. **Player options win**: the first valid option (a
   `TagQuestion`, `ActiveRoll`, or an alias of one; `NeedsUserInput`) discards NPC candidates
   collected so far, and later NPC children are skipped. `exclusive` works as for roots.
   Invalid `optional` children are a fallback. In automated, world and behaviour dialogs nothing
   needs input, so questions are treated like NPC nodes.
4. Options: the current node is pushed onto the instance's **selection levels** (hubs), and all
   options are offered. Otherwise the first candidate starts.
5. Nothing valid: the first fallback node with children is passed through (made current without
   being played; its children are evaluated). Otherwise the most recent hub is popped and its
   choices are offered again; with no hub left the dialog ends.

So a leaf answer returns to the question hub it came from, where `ShowOnce` options have gone.
`Pop` (`esv::PopNode::Start` → `DialogInstance::PopNodeSelectionLevels`) drops `PopLevel` hubs
and passes through, so a leafless `Pop 1` returns to the hub *before* the latest one.

### Starting a node

`StartDialogNode`: the node is recorded in the history (`DialogInstance::SetCurrentNode`), then
its **set-flags are applied** (`DialogNode::Start` → `FlagCollection::PushState`), then the
node-specific `Start`. Set-flags therefore apply when a node starts, before its children are
evaluated; an option's set-flags apply only once it is picked. `RollResult` and
`FallibleQuestionResult` never apply theirs (all such nodes have none anyway).

- Text nodes play their line. The line is a `rand() % count` pick among the `TagText`s of the
  first `TaggedText` whose rule holds (`BaseTaggedNode::OnPresentedForSelection`,
  `GetRandom`); a node without a matching text passes through.
- A chosen `TagQuestion` evaluates its `ValidatedFlags` (true without `ValidatedHasValue`) for
  `FallibleQuestionResult` children, and passes through (the runner doesn't replay the option
  as a line).
- `PassiveRoll`: rolls when started (roller = node speaker, target = `RollTargetSpeaker`; the
  node's exclusion settings are ignored), waits for the result, then the matching `RollResult`
  branch runs (`esv::PassiveRollNode::Start`, `eoc::RollResultNode::IsValid`). Failure is not
  silent: the `Success=false` branch plays.
- `ActiveRoll` (an option): once chosen, runs the question start, then requests the roll with
  the node's exclusions and waits for the roll UI (`esv::ActiveRollNode::Start`).
- `Nested Dialog`: switches the instance to the nested resource (`DialogManager::SetInstanceDialog`)
  and starts it from its roots. **Speaker linking** rebuilds the actor list from a snapshot:
  `actors[Key] = old[Value]`; a slot that appears only as a `Value` is cleared
  (`DialogInstance::ApplySpeakerLinking`). When the nested dialog reaches an end node it returns
  (`PopPlayedNestedDialogs`, also popping past `Nested` nodes that are themselves end nodes) and
  continues with the `Nested Dialog` node's children; nothing valid there ends the dialog.
- `Pop`, `Visual State`, `Trade` (which opens trading), result nodes: pass through.

**Option availability.** An option whose `ValidatedFlags` (with `ValidatedHasValue`) fail is
shown but can't be picked, unless it branches on the outcome: exactly two
`FallibleQuestionResult` children with one `Result=false` (questions), or any `Result=false`
child (active rolls) (`TaggedQuestionNode::ValidateInput`, `eoc::ActiveRollNode::ValidateInput`).

**Timing** (`transitionmode`): 0 waits for an external continue, 1 is timed (game wait time or
`waittime` s), 2 continues on the next update. The runner leaves timing to the caller (a
`Step::Line` waits for `advance`).

### Tag rules

`TagRuleGroup::EvaluateState`: `TagCombineOp` 0 = all rules, anything else = any rule; empty =
true. `TagRule::EvaluateState`: the tags of speaker slot `speaker`, op 0 = all present, 1 = any,
2 = none; no tags = true; an empty or out-of-range slot fails a rule that has tags. Nested rule
groups are ignored (`TagRule::HasChildRules` always returns false). A speaker "has" a tag if it
is in its regular tags or its dialog tags (`esv::EoCDialogGameInterface::IsSpeakerTagged`).

### Flags

Checks (`FlagCollection::EvaluateState`) are an AND that stops at the first failure.

At load the engine **retypes** flags (`FlagCollection::Visit`): groups other than Tag, Script,
Local and Quest take the type from the flag resource's `Usage`; a `Global` group whose flag is
per-object gets `paramval = GetDefaultSpeakerIndex()`; a per-object flag without `paramval` is
dropped. Then (`esv::EoCDialogGameInterface::EvaluateFlag` / `SetFlag`):

| Type | Stored on | Check | Set raises |
|------|-----------|-------|-----------|
| Global (5) | the global store | value | `FlagSet(flag, NULL, inst)` |
| Local (1) | the global store, `paramval` ignored | value | nothing |
| Character (4, "Object") | the speaker | value | `FlagSet(flag, speaker, inst)` |
| Dialog (6) | the speaker's dialog-flag store | value | per speaker |
| DialogInstance (8) | the speaker's dialog-flag store; sets also reach nearby party members (distance `DialogInstanceFlagRange`) | value | per object |
| User (2) | every character of the speaker's player | set on any of them | one per character |
| UserShape (7) | as User, dialog-flag store; checks read the speaker only | value | one per character |
| Party (3) | every party member | set on any of them | one per member |

- A missing flag resource fails the check whatever `value` is. A per-object flag with an empty or
  out-of-range slot reads false.
- `Tag`: check = speaker has the tag; set adds/removes a **dialog tag** (a separate list,
  `esv::tags::DialogTagComponent`), no Osiris event.
- `Script`: evaluates the condition script; never set.
- `Quest`: check = the step is unlocked; set unlocks it for the speaker (even with
  `value=false`).
- Events (`esv::OsirisVariableHelper::SetIndividualFlag`): none if an existing value is
  unchanged; clearing a never-set flag is silent; otherwise `FlagSet`/`FlagCleared` with the
  dialog instance id as third argument.

#### Order of set-flags

A node's flags live in a 37-bucket hash map keyed by the **resolved type name** (a
`FixedString`); `PushState` walks the buckets in order, flags of one type in file order. The
bucket is `(string-table handle) % 37`, and the handle depends on string interning at run time
(`ls::gst::Add`), so the order of types **cannot be derived from the binary**. The one data
point we have (trace below) is Object before Global; the runner uses that and keeps other types
in order of first appearance (`FlagSlot::OBSERVED_ORDER`, configurable). Open: capture the
handles of `LSDFS::strGlobal`, `strObject`, … from a running game to get the full order.

### Events

In the order the game raises them (`esv::StoryDialogEventManager`; Osiris queues them):

1. `DialogStarted(dialog, inst)`, or `AutomatedDialogStarted` when the resource is `automated`,
   `isWorld` or `isBehaviour`. `dialog` is `Name_ID` of the resource.
2. `DialogActorJoined(dialog, inst, actor, slot)` per filled slot, in slot order.
3. During play: `FlagSet`/`FlagCleared` as nodes start; for rolls the roll system's
   `RollResult("<node uuid>_<inst>", roller, target, success, isActive, criticality)` and, for
   active rolls, `DialogRollResult(roller, success, dialog, isDetectThoughts, criticality)`;
   `InstanceDialogChanged(inst, old, new, oldStopping)` on nested switches.
4. `DialogActorLeft(dialog, inst, actor, instanceEnded)` per actor.
5. `NestedDialogPlayed(dialog, inst)` per nested dialog that played, then
   `DialogEnded(root dialog, inst)` / `AutomatedDialogEnded`.

`DialogEvent::osiris` renders runner events in the runtime-log format. Names of non-`NONE`
criticality values are not verified.

### Not modelled

Timelines and cinematics (`TimelineId`, `LookAts`, camera, `waittime`), voice, node grouping
(`GroupID`/`GroupIndex`, timeline-only), approval ratings, `DetectThoughts`, dialog variables,
trading, the script-flag interpreter (the world answers script flags), the behaviour/world
start-node preference, and actor joining/leaving mid-dialog.

## Verification

- `cargo test -p bg3-dialog --release -- --ignored` (needs the install):
  `parse_every_dialog_in_install` (9 386 files, 0 failures), `dialog_resources_resolve`,
  `flag_usage_matches_group_type`, and `runner_walks_every_dialog` (two walks per resource,
  first/last option, alternating roll outcomes: 18 776 walks, no panics; 13 hit the step budget
  by looping through non-`ShowOnce` hubs).
- `trace_dialogs_reproduce_logged_events` (needs `BG3_TRACE` pointing at a libOsiris runtime log,
  see [osiris-runtime-log.md](../research/osiris-runtime-log.md)). For each dialog instance in
  the log it rebuilds the flags and tags from the earlier events, fills the slots from
  `DialogActorJoined`, takes roll outcomes from `RollResult`, and searches the player's choices
  for a walk whose Osiris events equal the logged ones. On the Nautiloid log
  (`full-nautiloid.log`) all six complete instances reproduce exactly:
  `TUT_Start_OriginIntroduction_Default`, `TUT_Start_PAD_Start` (automated),
  `TUT_Start_Brinepool` (one active roll), `TUT_Lab_AD_DevourerVictim` ×2 (automated) and
  `TUT_Lab_TrappedDevourer` (28 events: 10 `FlagSet`, 2 passive and 3 active rolls, Object and
  Global flags, choice path 1,1,1,3,1). One instance is cut off by the end of the log.

What the log can and can't confirm: the start/actor/end events, which flags are set, on whom,
in which order relative to the rolls, and roll targets. It can't show the player's actual
choices (we find *a* consistent path), lines and texts (no events), checks that didn't change
the path, flags without Osiris events (`Local`, `Tag`), or the order of flag types beyond
Object-before-Global (that order was fitted to this same log).

## Prior art

Norbyte's LSLib reads and writes these resources and documents node constructors and flag
types. The runtime semantics above come from our own reading of the reference binary and the
runtime log; the code is an independent implementation.
