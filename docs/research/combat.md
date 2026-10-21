# Combat

Turn-based combat in `bg3-sim` (`crates/bg3-sim/src/combat/`): how a fight starts, initiative,
the turn order, action resources, attacks and spells, damage, death, the story events, and an
approximate enemy AI. Reference build 4.1.1.7631656 (macOS, arm64 slice). This note says, for
each rule, where it comes from and whether it is **verified** (by the reference trace or the
binary), **read** (from the binary, not yet seen in a trace) or **approximate** (a stand-in).

Sources:

- **Data**: stats (`Character`, `Weapon`, `Armor`, `SpellData`, `StatusData`, `Equipment.txt`,
  `SpellSet.txt`, `Data.txt` keys), `Factions.lsx`, `Relations.lsx`,
  `ActionResourceDefinitions.lsx`, `ClassDescriptions.lsx`, `Origins.lsx`, `Races.lsx`,
  `Progressions.lsx`, `Lists/SpellLists.lsx`, level/globals `Characters/_merged.lsf` and root
  templates (`Equipment`, `SpellSet`, `SkillList`, `CombatGroupID`). Loaded by
  `combat::data::CombatData::load` into `GameData::combat`.
- **Binary**: the symbol index plus disassembly (`objdump` on the thin slice) and Ghidra
  decompilation of single functions (headless, no auto-analysis: the binary is too large for
  REA's Ghidra start-up deadline; importing with `-noanalysis` and creating the function at the
  address works in seconds). Findings are written up here in prose.
- **Reference trace** `combat-20261021-183321` (not committed): a libOsiris runtime log of the
  Nautiloid imp fight on Balanced, recorded from a save with a cleric avatar and Lae'zel, plus
  an lldb probe log of `esv::combat::System::NewInitiativeRoll`,
  `esv::combat::SystemHelper::RollDie` and `esv::StatsSystem::ApplyDamage`, and screenshots of
  tooltips and the combat log (`tools/osiris-trace/enable_log.py osiris_trace_probes`).

## Entering combat

The engine starts fights itself (`esv::combat::System::ProcessStartCombat`, 0x1052835d0) when
hostile characters notice each other; in the trace the imp fight starts right after Lae'zel joins
the party, with no story call. The story can also start fights (`EnterCombat`,
`SetHostileAndEnterCombat`, …).

- **Hostility** (*read*): `eoc::relation::GetRelation` (0x101c4fcec) →
  `(anonymous)::GetRelation` (0x101c4f9a4) → `eoc::relation::GetFactionRelation`
  (0x101c4ed14). Relations are values 0 (hostile), 50 (neutral, the default) and 100 (allied).
  The faction relation walks the source faction and its ancestors (`ParentGuid`); for each it
  looks for a relation towards the target faction or one of the target's ancestors, nearest
  first; the first one found wins. The story's runtime relations (`SetRelation`) are consulted
  before the data. `IsEnemy` (`esv::OsirisGameFunctions::IsEnemy`, 0x1055646f0) is
  `relation == 0`. *Not modelled*: individual relations, attitudes, the extra party rule at the
  end of the inner `GetRelation`. The sim treats two party members as allied and two characters
  of the same faction as allied.
- **Who joins** (*approximate*): the two that start it, everyone sharing a `CombatGroupID` with
  a participant (level-object attribute; the three imps share one), and characters allied to a
  participant within `JoinAllyInCombatRadius` (Data.txt, 7 m; `ProcessStartCombat` reads it and
  the story's `GetEnterCombatRange` answered 7.0 in the trace), repeated until nothing changes.
  Off-stage characters (`SetOnStage(x, 0)`) don't join. The engine's sight checks,
  `CanJoinCombat`, dialogs that block joining and surprise are not modelled.
- **Off stage** (*verified by trace*): with one player the story keeps
  `S_FirstFight_Imp_Ranged_02` off stage (`TUT_LowerDeck` INIT), and the trace's fight has three
  imps.

### In a live game

The live path to the imp fight is the trace's: the avatar enters
`S_TUT_LowerDeck_GuideDialogTrigger`, the story runs `TUT_LowerDeck_AfterImpFight` with Lae'zel
(the root for a guide tagged `REALLY_LAEZEL`; origin characters carry their `Origins.lsx`
`ReallyTags`, *verified by trace*: `IsTagged(S_Player_Gale, REALLY_GALE)` is 1), the recruiting
answer sets `OriginAddToParty`, `PROC_GLO_PartyMembers_Add` calls `MakePlayer` and
`RegisterAsCompanion` (`crates/bg3-sim/src/party.rs`: `CharacterMadePlayer`,
`CharacterReservedUserIDChanged`, `CharacterJoinedParty`) and `SetFaction` (companion faction), the
story teleports the party into the imp room (`IsInTrigger`), and the imps notice the party.

- **Noticing** (*approximate*): `Sim::start_combat_on_sight` (`combat/notice.rs`), called once per
  frame by a live front end: a hostile character within the `Sight` (`Character` stats, cm;
  `_Base` 1600) of a party member, both on stage, alive, not in a fight or a dialog, starts a fight
  (`enter_combat(hostile, party member)`). The engine's sight system (field of view, light,
  stealth, hearing, `Saw` events) is not modelled; `start_combat_on_sight_with` takes a
  line-of-sight test, and the viewer passes one on the AI grid (walkable tiles near the straight
  line between the two), so other decks and walls don't count.
- **The avatar** (*approximate*): character creation is skipped, so the placeholder avatar gets the
  reference combat trace's class (Cleric) and its template's race (`start::TRACE_AVATAR_CLASS`,
  `CombatState::created_origins`): class hit points and the race's movement.
- **Spell slots** (*approximate*): `SpellSlotsGroup:n:…:level` costs are paid from `SpellSlot`,
  slot levels not told apart.
- Characters the story took off stage (`SetOnStage(x, 0)`, live) don't join fights; test
  `live_imp_fight` checks the trace's line-up (avatar and Lae'zel sharing the first turn, three
  imps).

## Initiative

- **Roll** (*verified by trace*): `InitiativeDie` (Data.txt: 4) + the initiative bonus. The
  die value is mapped from faces to `EDiceType` in `ProcessStartCombat`;
  `esv::combat::System::NewInitiativeRoll` (0x10528e328) calls
  `esv::combat::SystemHelper::RollDie` (0x1052684b0) with that die and
  `StatsComponent::InitiativeBonus`. The probes show RollDie with dice type 0 (d4) for all five
  participants, and results 3, 2, -17, -19, -18.
- **Bonus** (*verified by trace* for the imps; *read*): `esv::StatsSystem::CalculateInitiative`
  (0x1057bd3ac) adds the base initiative (`esv::BaseStatsSystem::Update` stores the `Character`
  entry's `Initiative`, clamped at 0), the `Initiative(n)` value of every active boost and
  `floor((Dexterity - 10) / 2)`. The imps' bonus in the probes is -21: Dexterity 8 (-1) and the
  story's `TUT_LOWERDECK_INITIATIVEBOOST` status (`Initiative(-20)`, applied to every imp of the
  `TUT_LowerDeck_ImpsDefeated` counter so the party goes first). The sim computes the same -21
  (test `combat_nautiloid`).
- **Not modelled**: a swarm member reuses its swarm group's roll (`NewInitiativeRoll` looks the
  group up first); a character that can't take part gets -20; the engine draws from a roll
  stream per entity (the sim uses its one seeded stream).
- **Open**: the second party member's probe shows *two* RollDie calls (bonus 1 each) with one
  result. Advantage, a reroll, or a probe artefact? The probe log can't tell which character
  that was.

## Turn order and shared turns

- **Sort** (*read*): the comparator of `eoc::TurnOrderSort` (0x1020e1fd8): higher initiative
  first; non-swarm before swarm members, swarm groups by name; higher Dexterity score; player
  characters (`eoc::PlayerComponent`) first; lower UUID (compared as the engine's two 64-bit
  halves; which half the sim's `u128` puts first is unverified); lower entity handle.
- **Groups** (*read*; *verified by trace* for two party members):
  `esv::GetShareInitiativeParameters` (0x10588e1d0) gives each entity a share key: a player
  character without `LoseControlComponent` shares with adjacent player characters; an AI swarm
  member with adjacent members of the same swarm group (when swarms are enabled); anyone else
  acts alone. Consecutive entries with the same key form one `TurnGroup`; its members act in any
  order and the next group starts when all of them ended their turn.
  `esv::SplitSharedInitiativeGroups` splits groups whose members stop qualifying (not
  modelled). The trace: avatar and Lae'zel act together, then each imp alone.
- **Rounds**: after the last group a new round starts (`CombatRoundStarted(combat, n)`) and the
  order is rebuilt from the kept initiative values; dead characters drop out.
- **Late joiners** (*approximate*): roll initiative and enter the order at the next round.

## Turns and resources

- **Action resources** (*data*): a character's `ActionResources` (`ActionPoint:1;
  BonusActionPoint:1;Movement:9;ReactionActionPoint:1` in `_Base`) plus, for origin characters,
  the `ActionResource(Name, n, level)` boosts of their race and class progressions up to their
  level (player stats entries have no Movement; the race grants it: Githyanki 9 m). Resources
  whose `ReplenishType` is `Turn` refill at the start of the owner's turn and on entering combat
  (*approximate*: the refill point was not read in the binary).
- **Movement** (*approximate*): the caller passes the path length (`bg3-nav` finds paths); it is
  paid 1:1 from `Movement` in metres. AI movement with a pathfinder pays the walked 3D polyline
  length, including partial final segments. No difficult terrain, no jumping.
- **Spell costs** (*data*): `UseCosts` are checked and paid before the cast
  (`ActionPoint:1` for weapon attacks).

### Presentation names

`Sim::display_name_handle` shares the story's `GetDisplayName` source: runtime story handle,
then the placed object's `DisplayName` **TranslatedString**, then the nearest root-template
`DisplayName` along `ParentTemplateId`. The ranged imp's placed object has no name handle;
its root template provides it. `Sim::display_name` resolves that handle through install-loaded
`bg3_loca::Loca` (English by default). Missing translations return `None`, allowing the viewer
to keep its object-name fallback. The viewer uses this for turn order, combat log and live or
standalone dialog speakers. Live dialog names are refreshed from the sim to honor story overrides.
The story query still returns a **handle**, not translated prose, and retains its conservative
replay boundary for names that may have been changed in a save.

Synthetic template/LOCA tests cover inherited, placed and story names and missing translation;
the ignored install test checks every first-fight imp resolves to a non-empty localized name
that differs from its object identifier. No extracted names, tables or localization are saved
in tracked files.

## Attacks, spells, damage

A weapon attack is a spell (`Target_MainHandAttack`, `Projectile_MainHandAttack`,
`Target_UnarmedAttack`, creature attacks like `Target_Claws_Imp`). The sim runs it through
`bg3_rules::exec::Executor::cast` (target conditions, `SpellRoll`, `SpellSuccess`/`SpellFail`,
`SpellProperties`; see `docs/formats/stats-expressions.md`), which now also reports structured
events (`ExecEvent`: rolls with their breakdown, damage, statuses, healing).

- **Equipment** (*approximate*): the object's or nearest template's `Equipment` set
  (`Equipment.txt`); from each `equipmentgroup` the first entry with `Weapon`/`Armor` stats.
  Slots from `Slot`. The imps' `EQP_Imp_Handaxe` etc. match what the trace shows them equip.
- **Known spells** (*approximate*): weapon attacks for the equipped weapons (unarmed without a
  melee weapon), the `SkillList`, the `SpellSet`, and for origins the fixed spell lists of their
  progressions (`AddSpells`). Choices (`SelectSpells`: cantrips, prepared spells) are not
  modelled; tests add spells to `Combatant::spells` directly.
- **Range** (*approximate*): `TargetRadius` as metres, or the weapon range keywords
  (`MeleeMainWeaponRange` = the melee weapon's `WeaponRange` / 100, 1.5 m unarmed;
  `RangedMainWeaponRange`), measured centre to centre.
- **Weapon proficiency** (*data*): the weapon's `Proficiency Group` against the character's
  `Proficiency Group`; characters without any (player stats entries; their proficiencies come
  from class progressions) count as proficient.
- **Attack ability**: as in `bg3_rules` (finesse, ranged → Dexterity); unarmed and creature
  attacks use the `Character` entry's `UnarmedAttackAbility` (the imps': Dexterity). New
  `RulesContext::attack_weapon` / `unarmed_attack_ability` hooks.
- **Armour class** (*read*; *verified by trace* for the imp): `esv::StatsSystem::
  CalculateArmorClass` (0x1057bdbc8). With body armour: its `ArmorClass` plus the modifier of
  its `Armor Class Ability`, capped by `Ability Modifier Cap` when positive (a boost can override
  the cap per armour type). Without: the `Character` entry's `Armor` plus the Dexterity
  modifier (plus the ruleset's `GlobalBaseACModifier`, 0 on Balanced as far as seen). Then a
  shield's `ArmorClass`, `AC(n)` boosts, and `ACOverrideFormula` boosts (Unarmoured Defence; not
  modelled). The imp: `Armor` 8, Dexterity 8 → **7**, as the trace's tooltip shows. Lae'zel:
  half plate 15 + Dex 1 (cap 2) = 16.
- **Hit points** (*verified by trace*): monsters' `Vitality` (imp 6/6 in the tooltip). Origins'
  stats entries have `Vitality` 1; their maximum is their class's `BaseHp` + Constitution
  modifier at level 1 and `HpPerLevel` + Constitution modifier per further level
  (`ClassDescriptions.lsx`, class from `Origins.lsx`): Lae'zel 10 + 2 = 12, the trace's cleric
  8 + 1 = 9.
- **Damage and resistances**: as `bg3_rules` (`apply_resistance`), resistances from the
  `Character` entry's `<Type>Resistance` fields.
- **Seen in the trace's combat log** (consistent with the sim's rules): the cleric's mace
  attack 12 + 2 (proficiency) = 14 vs AC 7, 3 bludgeoning; Lae'zel's main-hand attack 7
  slashing; Sacred Flame (1d8 radiant, Dexterity save, 18 m), failed save, 3 radiant.

## Death

- **NPCs** (*verified by trace*): at 0 HP, in this order around the killing cast:
  `HitpointsChanged(x, 0.0)`, `CastSpell`, `Dying(x)`, `KilledBy(x, killer owner, killer,
  action)`, `CastedSpell`, `StatusAttempt(x, "DYING", null, 0)`, `LeftCombat(x, combat)`,
  `AttackedBy(x, …)`, `StatusApplied(x, "DYING", null, 0)`, the other statuses removed,
  `Died(x)`. The trace also has tag, sight, loot and capability events the sim does not raise,
  and removes each status with the story action id it was applied with (the sim reports 0).
  The story's defeat counter reacts to `Dying` (`DB_Dead` → `TUT_LowerDeck_ImpsDefeated` →
  flag `TUT_LowerDeck_State_ImpsDead`, checked by `combat_nautiloid`).
- **Party members** (*approximate*): downed (`DOWNED` status, `DownedChanged(x, 1)`), death
  saving throws on their turn by the 5e rule (d20 ≥ 10, 1 = two failures, 20 = up with 1 HP,
  three of a kind decide). The engine's downed/death systems (`esv::death`) were not read and
  the trace has no downed party member.

## Story events

Order *verified by trace* unless noted. Arguments: objects as `Name_<uuid>`, the combat as a
bare UUID (the sim generates one per fight), story action ids from a counter (the engine's ids
are larger because other actions use the same counter).

| When | Events |
|---|---|
| fight starts | `EnteredCombat(x, combat)` per participant, `CombatStarted(combat)`, `CombatRoundStarted(combat, 1)`, then the first group's turns |
| a group's turn starts | `TurnStarted(x)` per member; in a shared group `ShareInitiative(x)` after each member's `TurnStarted` except after the last member in round 1 (*observed*, cause unknown); then `TurnEnded` for the members of the previous group |
| a new round | `CombatRoundStarted(combat, n)` before the first group's turns |
| a cast | `UsingSpell(caster, spell, type, school, action)`, `UsingSpellOnTarget(caster, target, spell, type, school, action)`, `StartAttack(target, caster, caster, action)` for attacks, the rules (`HitpointsChanged`, statuses), `CastSpell(…)`, `AttackedBy(target, attacker owner, attacker, damage type, amount, "Attack", action)`, `CastedSpell(…)`; spell type in lower case (`"target"`, `"projectile"`), school from `SpellSchool` (`"Evocation"`, empty for weapon attacks); a miss raises `AttackedBy` with `""` and 0 |
| a kill | see Death |
| fight ends | `TurnEnded` for the acting group, `LeftCombat(x, combat)` for every remaining participant, `CombatEnded(combat)`. The trace also shows a `CombatRoundStarted(combat, 4)` right before `CombatEnded` that the sim does not raise |

Story functions: `EnterCombat`, `EndTurn`, `IsInCombat`, and `SetOnStage` (off-stage set only).
`sim-replay` of the opening still agrees with the game on every graded answer.

## AI (approximate)

`combat::ai`: approach the nearest living hostile and use the first affordable damaging
spell that reaches (melee first when adjacent), else close in. Not from the binary.

With `Sim::set_pathfinder(Some(...))`, AI movement follows a walkable route on the level's AI
grid. It stops at attack range (retaining the 5 cm safety margin when affordable), or at the
movement budget, charging the walked polyline length. An unreachable target causes no movement;
an attack already in range still works. Without a pathfinder, the original straight-line
placeholder behavior is preserved for existing headless consumers and reference replays.
`AiAction::path()` exposes the walked waypoints, including both endpoints, for animation.

The shared `bg3-sim-nav::GridPathfinder` adapter depends on both core crates, keeping `bg3-sim`
independent of `bg3-nav` and Bevy and keeping `bg3-nav` independent of the sim. It wraps an
`Arc<NavGrid>` and is used by `bg3-viewer --play` and `bg3x combat --nav`. The viewer animates
all walked waypoints, starting at the recorded origin before applying the final sim position.
The adapter rejects a blocked start column rather than connecting it to a sideways-snapped
start through geometry; valid starts may need a short vertical approach to the grid surface.
The destination uses the grid's existing standable projection; disconnected goals return no
path. This does not search for a best partial route in a disconnected component.

Still approximate: target/spell selection (no tactical scoring or attack line of sight),
centre-to-centre ranges, a fixed **0.5 m** radius (the sim does not load AI bound radii), default
humanoid step height and collision mask, the grid search/smoothing limitations documented in
`docs/formats/navcloud.md`, and interpolation between smoothed floor waypoints. This is a fix
to our placeholder AI, not newly verified original-engine AI behavior.

Validation: synthetic wall-detour tests in `bg3-sim` check route truncation, path-length costs,
range stops and unreachable targets. The ignored `bg3-sim-nav` test
`nautiloid_nav_endpoints_are_standable_and_fight_finishes` checks each move endpoint's actual
column (no sideways standability snap, within 0.6 m of the floor) and fight completion.
With seed 0, Lae'zel and Shadowheart against the lower deck imps, the CLI fight with and
without `--nav` both finished in **9 turns**, with the party winning and the story defeat flag
set. All **5** nav move endpoints were standable; routes and movement costs differ.

The real AI scores actions, targets and positions: `esv::ai::…`, `esv::AiHelpers` (e.g.
`esv::AiHelpers::CalculateMovementScore`), archetypes (`Public/<Module>/AI/Archetypes`), the
objects' `Archetype` attribute (`imp_melee`), swarm groups (`esv::ai::swarm`). The trace's
imps: the ranged imp used `Projectile_Generic_Imp` on Lae'zel, the melee imp
`Target_MainHandAttack` on the avatar and Lae'zel.

## API

On `Sim` (state) and `Game` (the same, with the story events delivered after each step):

- set-up and queries: `ensure_combatant`, `combatant` (resources, equipment, spells,
  initiative, downed/dead), `combat_of` / `combat_by_id` (`Combat`: participants, round,
  `order` of `TurnGroup`s, `current_group`, `is_active`, `turn_order`), `in_combat`,
  `relation`, `is_hostile`, `spell_range`, `initiative_bonus`
- actions: `enter_combat(a, b)`, `combat_move(who, metres, Option<position>)`,
  `combat_cast(who, spell, target)` → `CastReport` (outcome, text log, `ExecEvent`s),
  `combat_end_turn(who)`, `combat_run_ai(combat, max)`, `Game::combat_auto(combat, max)`
- notifications: `take_combat_events()` → `CombatEvent` (combat start/end, initiative rolls,
  rounds, turns, moves, casts, rolls with breakdown, damage, statuses, healing, downed, death
  saves, deaths, rules log lines)
- scenario helpers: `Game::start_level(level)` (the new-game events up to the level start),
  `Game::set_companion_factions(party)` (`Companion1`, `Companion2`, … as the story assigns
  on recruitment; the trace: Lae'zel → `Companion1`)

`bg3x combat [LEVEL] --party A,B [--target X] [--seed N] [--events] [--nav]` runs a fight on
the AI and prints the log. `--nav` loads the level's AI grid and prints endpoint standability
and walked path length for each AI move.
`cargo test -p bg3-sim --test combat_nautiloid -- --ignored` runs the original headless imp
fight test; `cargo test --release -p bg3-sim-nav --test combat_nautiloid -- --ignored` runs
the nav endpoint/completion test.

## What a further trace should capture

- The same fight with **probes on the party's initiative** that identify the entity (the
  `x3` argument of `NewInitiativeRoll` → `ls::uuid::Component`), to settle the double RollDie.
- **Attack rolls**: `eoc::stats::GetAttackRollResults` (0x101fc88e4) and the roll manager
  (`eoc::RollManager::InternalRollDiceWithBoosts`, 0x101c62184) with d20, modifiers, target AC;
  damage rolls (`eoc::stats::CalculateDamage`, 0x101fd7408).
- **Movement**: positions per turn and the Movement resource (`eoc::ActionResourcesComponent`)
  at turn start and after moving, for the path-length rule.
- **A downed party member** and death saves; **a late joiner**; **a swarm** (many identical
  enemies) for shared AI turns; the AI's choices (`esv::ai` scoring) for a rules-faithful AI.
- `CombatGetGuidFor`, `IsEnemy`, `GetHitpoints` answers during combat, to grade the sim's
  queries in `sim-replay` (the combat trace starts from a save, which the replay can't load
  yet).
