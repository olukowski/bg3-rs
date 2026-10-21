# Combat AI

How the engine's combat AI chooses an action, and what `bg3-sim` implements of it
(`crates/bg3-sim/src/combat/ai/`). Reference build 4.1.1.7631656 (macOS, arm64 slice).
Labels as in `combat.md`: **read** (from the binary), **verified** (by a trace), **approximate**
(our stand-in).

Sources: the symbol index, `objdump` of the thin slice, and headless Ghidra decompilation of
single functions (the project under `/tmp/bg3-ghidra`, no auto-analysis, `-readOnly`). The
archetype files are read from the user's install at runtime; none of their values are in the
repository (tests use our own archetype text).

## The pipeline

The combat AI is the DOS2-lineage "AiHelpers" system (`CombatAISystem.cpp`), not the
`esv::ai::swarm` systems (those run swarm groups only).

1. **Request** (*read*): `esv::AiHelpers::PrepareRequest` (0x1051f5fc0) and
   `esv::AiHelpers::UpdateRequest` (0x1051f94e0), a per-frame state machine, plan one action
   for the active character: `StartCalculateAiScoreSession` (0x1051f39bc), gather spells
   (spell book, item uses; `esv::ai::combat::CanAICastSpell` 0x1051e2f9c,
   `esv::SpellHelpers::CanCast`), interesting tiles (`CalculateInterestingTiles`), per spell
   `esv::AiSpellState::CalculateSpellAi` (0x1050c41b4), positions and movement, then score and
   sort the `esv::CombatAIAction`s and execute the first through an Osiris task
   (`OsirisMoveInRangeTask`). After the action the next request plans again with what is left.
2. **Spell simulation** (*read*): per spell type a `esv::spell_state::<type>` namespace
   (`target`, `projectile`, `projectile_strike`, `shout`, `zone`, `rush`, `teleportation`,
   `throwing`, `multistrike`) with `CalculateSpellAi`, `CalculateSpellAiSimulate`,
   `CalculateSpellAiScoreCharacter/Item/Explosion`. Positions come from
   `esv::AiTileIteratorHelper::IterateTiles` / `CalculateTopScoresInternal`, which keeps the
   **5 best tiles** (`StaticArray<TileScoreData, 5>`).
3. **Effects** (*read*): `esv::AiHelpers::CalculateAiScoreSpellStatsFunctors` (0x105248600) →
   `CalculateAiScoreStatsFunctors` (0x105223ba0) dispatch on the functor type to
   `CalculateAiScoreDamageCharacter` (0x10522eb80), `CalculateAiScoreStatus` (0x105211624),
   `CalculateAiScoreHealCharacter`, surfaces, boosts, concentration, … Each adds to an
   `esv::AiScore`.
4. **Position and movement** (*read*): `esv::AiHelpers::CalculatePositionScore` (0x105215238)
   for the end tile, `CalculateMovementScore` (0x10521935c) for the walk (surfaces, auras,
   attacks of opportunity through `CalculateAiScoreAoO` 0x10524f0f8, `MULTIPLIER_POSITION_LEAVE`
   for jumps/teleports).
5. **Score** (*read*): `esv::CombatAIAction::CalculateScore` (0x1051e1c2c) →
   `esv::AiScoreInternal::CalculateScore` (0x10508365c).
6. **Choice** (*read*): `ls::LEGACY_Set<esv::CombatAIAction>::Sort` (0x105213e6c); the first
   action is executed. **No random draw** decides between actions: the roll streams
   (`esv::roll::stream::RollManager::GetGenericStream`) appear in the AI only inside
   `esv::AiStatus::CalculateAiScore`, `CalculateAiScoreStatsFunctors`,
   `CalculateAiScoreBreakConcentrationOnDamage` and `CalculateAiScoreBoost` (status and
   functor scoring), not in candidate generation or the sort. With equal state the choice is
   deterministic.
7. **Fallback** (*read*, details not): `esv::AiFallback::CalculateAiScore` /
   `GetFallbackPosition` and the `FALLBACK_*` modifiers; actions that can't be done this turn
   stay in the list at `MULTIPLIER_CANNOT_EXECUTE_THIS_TURN` and move the character towards
   them.

## Archetypes

Files `Public/<Module>/AI/Archetypes/*.txt` (Gustav: `base`, `melee`, `ranged`, `mage`,
`imp_melee`, `imp_ranged`, …; GustavDev adds act-specific ones) plus folders named after the
`AI_LETHALITY` ruleset value (`AILETHALITY_FORGIVING`, `AILETHALITY_BALANCED` — the default
of `RulesetModifiers.lsx` — `AILETHALITY_BRUTAL`), and `CLASSIC`, `TACTICIAN` folders whose
selection we have not seen. A character's archetype is the `Archetype` attribute of its level
object or root template (the imps: `imp_melee`, `imp_ranged`); `AiArchetypeOverride` boosts
can replace it (not modelled).

Loader (*read*): `(anonymous)::LoadModifiersFromDisk` (0x1050807bc) and
`esv::ai::combat::AiModifiersSystem::Update` (0x10507c568):

- the archetype's name is the file stem; files of the same name from later modules and from
  the lethality folder (`<Module>/AI/Archetypes/<AI_LETHALITY value>/`, read from
  `eoc::ruleset::code::AILethality::s_ModifierGuid`) are **merged**: entries appended, a
  `USING` replaces the parent;
- per line every space and tab is removed, then `//` comments; a leading `+ - * / =` is the
  operation (ops 0 add, 1 subtract, 2 multiply, 3 divide, else set); `USING<name>` names the
  parent; otherwise the key runs to the first digit, `.` or `-` and the rest is the value;
- resolution: the parent's table (an archetype without `USING` builds on `base`, *inferred*:
  `melee.txt` subtracts from a value it never sets), then the entries in order.

The modifier table is a float array indexed by `esv::ai::combat::EAiModifier`; the enum order
is the order of the modifier names in the binary's string table (`SCORE_MOD` = 0,
`BASE_NEARBY_SCORE` = 1, …, 228 names), so offset = index × 4. That mapping is consistent with
every access we read (e.g. +0xfc `MULTIPLIER_ACTION_RESOURCE_COST`, +0x1c8
`MULTIPLIER_KILL_ENEMY`, +0x2f4 `MODIFIER_HIT_CHANCE_STUPIDITY`).

`bg3-sim`: `combat::ai::archetype` (`Archetypes::load`, `resolve`), loaded into
`CombatData::ai` with the Balanced lethality; `Sim::ai_modifiers(who)`. Without archetype
data (synthetic tests) a neutral table of our own is used.

## Scoring

### The score record

`esv::AiScore` holds a record per affected entity (*read*, `AiScoreInternal`): damage, heal,
damage over time, heal over time, control, boost, a raw part and an instakill chance, plus
flags.

- `AddDamage(entity, amount, turns)` (0x1050855c0): `turns` 0 → damage; > 0 → damage, and
  for more than one turn `((turns − 1) · 0.5 + 0.5) · amount` as damage over time; < 0 →
  over time only.
- `ApplyChance(p)` (0x105085768) scales every field; `Add(score, f)` (0x105083080) adds
  scaled fields, instakill chances combine as `a + b − ab`.
- `AddRaw` (0x105085b34): position and movement terms on the actor.

### Chances and expected damage

- The **believed chance** of a functor (*read*, `CalculateAiScoreStatsFunctors`,
  `CalculateAiScoreSpellStatsFunctors`): `clamp(S + (1 − S) · p)` with
  `S = MODIFIER_HIT_CHANCE_STUPIDITY`. `S = 1` means the AI assumes every roll succeeds;
  smaller is more realistic. Success functors are weighted by it, fail functors by its
  complement. Advantage/disadvantage enter a chance as `1 − (1 − p)²` / `p²` (the
  concentration-save chance in `CalculateAiScoreDamageCharacter`, from
  `eoc::roll::GetChanceToPass`). The spell roll's chance comes from
  `eoc::spell::CalculateModifiableSpellRollChance`.
- **Damage** (*read*, `CalculateAiScoreDamageCharacter`): after resistances `r` of raw `d`:
  `r < 1` → `d · MULTIPLIER_IMMUNITY_STUPIDITY`; else `r + (d − r) · MULTIPLIER_RESISTANCE_STUPIDITY`
  (base: 1, i.e. the AI ignores resistances unless an archetype says otherwise).
- **Unit of damage** (*read*): `GetAverageDamageForCharacterNoCache` (0x10523a384) is the
  maximum over the character's spell book (one spell type skipped; we read it as `Throw`) of
  the summed damage of `esv::LEGACY_GetSpellDamage`, divided by the `Attack` resource cost
  amount; `GetNormalizedDamageScore` (0x10524e9f8) divides by it. With `SCORE_MOD` (base 100)
  a hit of the character's average damage on an enemy scores 100.

### Per target (`AiScoreInternal::CalculateScore`, *read*)

For each entity in the record, with `hp` = hit points + temporary hit points:

1. Target multiplier `t` starts at 1 (`MULTIPLIER_TARGET_PREFERRED` for `AI_PREFERRED_TARGET`
   tags, × `MULTIPLIER_TARGET_UNPREFERRED`, × a health bias term).
2. Allies' boosts fall off with their distance to the closest enemy
   (`BUFF_DIST_MIN..MAX`, factor 1 → 0.1).
3. **Damage**: for enemies control is scaled by `min(average damage / damage, 1)`. If
   `(int) damage < hp` and no instakill: on allies damage is divided by `max(hp / max hp, 0.1)`.
   Otherwise it is a **kill**: killing oneself invalidates the action; damage gets
   `MULTIPLIER_KILL_TARGET_HEALTH_BIAS · hp` (+ `INSTAKILL_BASE_SCORE …`); the kill factor
   `k` is `MULTIPLIER_KILL_ENEMY[_SUMMON]` / `MULTIPLIER_KILL_ALLY[_SUMMON]` (neutral 1), times
   the instakill chance if the damage alone does not kill; boost and control drop to 0.
4. **Heals** count negative: `−heal`; overheal `−missing² / heal`; below half health divided
   by `max(2 · ratio, 1 − MAX_HEAL[_SELF]_MULTIPLIER)`; the same for heal over time after the
   action's damage and heal.
5. Neutral targets: every part clamped to ≥ 0.
6. Relation: the actor itself `k = −k`; allies `k ·= MULTIPLIER_SCORE_ON_ALLY`; neutrals
   `k ·= MULTIPLIER_SCORE_ON_NEUTRAL`; enemies `t ·= MULTIPLIER_TARGET_MY_ENEMY` /
   `MY_HOSTILE` (current enemy relations). For non-self targets also
   `MULTIPLIER_SCORE_OUT_OF_COMBAT`, `MULTIPLIER_TARGET_SUMMON`, `…_AGGRO_MARKED`,
   `…_{ENEMY,ALLY,NEUTRAL}_DOWNED`, `…_HOSTILE_COUNT_ONE / _TWO_OR_MORE` (how many fight the
   target), `MULTIPLIER_TARGET_IN_SIGHT` — applied when `esv::CheckIsInSight` **fails**, as the
   code reads — and, without control, `…_KNOCKED_DOWN` / `…_INCAPACITATED`.
7. `k ·= SCORE_MOD`, `t ·= k`; damage, heal, DoT, HoT, control and boost are multiplied by
   `t` and by `MULTIPLIER_<KIND>_<SELF|ENEMY|ALLY|NEUTRAL>_<POS|NEG>` by their sign; damage,
   heals and over-time parts are divided by the average damage — **damage to oneself by one's
   own maximum hit points instead**. The raw part is multiplied by `k` only (so position terms
   on the actor count negated and scaled by `SCORE_MOD`).
8. Sum over entities (+ a base value); `×1000` and other flags for special cases;
   `MULTIPLIER_SELF_ONLY_THROW`, `MULTIPLIER_PLANNED_ACTION_WITH_MOVE_SPELL`; a minimum score
   flag.

### The action (`CombatAIAction::CalculateScore`, *read*)

`final = m · score − cost`:

- `cost = GetResourceCostScore · MULTIPLIER_ACTION_RESOURCE_COST`, or
  `MULTIPLIER_FREE_ACTION` when that is 0. `GetResourceCostScore` (0x10521cc50) sums
  `amount / resource maximum × MULTIPLIER_RESOURCE_REPLENISH_TYPE_<type>`.
- `m` = 1, or `MULTIPLIER_CANNOT_EXECUTE_THIS_TURN` for an action that can't be done this
  turn (if such an action would not move the character: −1000); times a cooldown factor
  `clamp(1 − cooldown · MULTIPLIER_COOLDOWN_MULTIPLIER / 6, 0.5, 1)` and, for item uses, a
  lerp between `MULTIPLIER_LOW/HIGH_ITEM_AMOUNT_MULTIPLIER` by `ITEM_HIGH_COUNT`.
- Invalid scores (flags) are −1000.

Sort keys (*read*): final score, raw score, a flag bit (`+0x1d8` bit 4), whether the action has
a target, then a float at `+0x1e0` compared in hundredths; what that float holds is unread.

### Position (`CalculatePositionScore`, *read*)

Raw terms on the actor (scored negated, see above):

- enemies within `max(ALLIES_MAX, ENEMIES_MAX)` (horizontal): each `BASE_NEARBY_SCORE` within
  `ENDPOS_ENEMIES_NEARBY_MIN_DISTANCE`, falling linearly to 0 at the max (× `MY_HOSTILE` for
  the current enemy); with a positive `MULTIPLIER_ENDPOS_ENEMIES_NEARBY` the **largest** value
  counts, with a negative one the **sum**; raw `−value · multiplier`. So base (0.02) likes a
  little closeness, `ranged` (−0.5) avoids enemies;
- allies the same with the `ALLIES` settings (sum);
- closest enemy farther than `MAX_DISTANCE_TO_CLOSEST_ENEMY`: raw
  `(d − max) · MULTIPLIER_NO_ENEMIES_IN_MAX_DISTANCE`;
- the fallback move uses the `FALLBACK_*` settings instead;
- plus height, flanking, AI hints, smoke, dangerous surfaces and items, darkness, auras,
  invisibility, ledges.

Movement (`CalculateMovementScore`): surfaces on the path (`MULTIPLIER_MOVEMENT_SURFACE`),
dangerous auras, and for each enemy whose reach the path leaves an attack of opportunity
simulated as the enemy's attack on the mover with the mover's modifiers
(`CalculateAiScoreAoO`), gated by `ENABLE_MOVEMENT_AVOID_AOO`.

## What `bg3-sim` implements

| Part | Status |
|---|---|
| Archetype files, merge, operators, `USING`, lethality folder | faithful (*read*); implicit `base` parent inferred; `CLASSIC`/`TACTICIAN`/`modifiers` files not applied |
| Score record, `AddDamage`, `ApplyChance`, `Add` | faithful |
| Believed chance, fail functors at the complement | faithful |
| Spell roll chance | exact: the roll condition evaluated by the rules engine for every d20 (pair) |
| Damage: dice averages (`Mode::Average`), resistance blend | approximate: integer averages; crits, functor `IF` conditions and status/surface functors not scored |
| Average damage unit | faithful structure; damage from our `SpellSuccess` `DealDamage` averages |
| Per-target formula (kill, heals, relation, target multipliers, POS/NEG, normalisation) | faithful for the modelled inputs; preferred tags, summons, aggro, current-enemy relations, hostile counts, sight, knocked down/incapacitated inputs are not tracked by the sim (always off) |
| Final score, resource cost, free action, can't-execute multiplier, −1000 | faithful; cooldown and item factors not modelled |
| Position: enemies/allies nearby, far from enemies, fallback settings | faithful; other position terms not modelled |
| Movement: attacks of opportunity | approximate (centre-to-centre reach, first melee attack, reaction available); surfaces/auras not modelled |
| Candidates | approximate: single-target `Target`/`Projectile` spells only; tiles = standing still, the route point where each spell's range is reached, 16 points around (not the engine's tile flood and top-5 tiles) |
| Fallback | approximate: "future" actions on enemies walk the budget towards them with the `FALLBACK_*` position terms |
| Sort | final score, raw score (faithful); then shortest walk, spell-book order, target id (stand-ins) |
| Turn | plan → execute the best if > 0 → plan again; a move-only action ends the turn |

API: `Sim::ai_plan(who) -> AiPlan` (all candidates with breakdowns, nothing executed),
`Sim::ai_turn`, `ai_turn_traced`, `combat_run_ai[_traced]`, `Game::combat_auto[_traced]`,
`Sim::ai_modifiers`. Debug view: `bg3x combat … --ai-debug [--ai-top N]` prints, per decision,
the actor's archetype and average damage and the best candidates with chance (raw and
believed), walk, final score = score − cost, position terms, attack-of-opportunity damage and
the per-target parts.

## Against the reference trace

Test `bg3-sim-nav` `ai_trace` (*ignored*, needs the install) reaches the fight the live way,
applies the trace's party actions with their outcomes and asks the AI for each of the trace's
three imp decisions:

| # | State (trace) | Trace | Model |
|---|---|---|---|
| 1 | Melee_02 dead, party unhurt | Ranged_01: `Projectile_Generic_Imp` → Lae'zel | projectile; Lae'zel and the avatar tied best |
| 2 | Lae'zel 10/12 | Melee_01: `Target_MainHandAttack` → avatar | main-hand attack; both tied best |
| 3 | Ranged_01 dead, Melee_01 3/6 | Melee_01: `Target_MainHandAttack` → Lae'zel | main-hand attack; both tied best |

The **spell** matches 3/3. The **target** is among the best-scored candidates 3/3, but our
tie-break (shortest walk) picks the other target in all three. The ties are real in the
engine's terms: `imp_melee` / `imp_ranged` set `MODIFIER_HIT_CHANCE_STUPIDITY` to 1 (every
attack believed to hit) and neutralise the target multipliers (`MY_ENEMY`, `MY_HOSTILE`,
hostile counts at 1), nobody can be killed (1 expected damage against 9–12 HP), so damage
scores are equal for both party members and only position terms and the sort's later keys
separate them. Two caveats: the trace has no positions after the teleport (the test places
the party next to the imp it attacked, and in decision 3 Lae'zel about 6 m from the imp, as
screenshot 18.40.28 suggests), and the trace's choices went to the farther target each time
under our placement — consistent with the unread `+0x1e0` sort key favouring a longer walk, or
with position terms we don't model (flanking: `imp_melee` sets `MULTIPLIER_ENDPOS_FLANKED`
−0.01). Dice don't enter the decisions (they are expectations), so with exact positions the
comparison would be exact.

## What a future trace should capture

lldb probes (`tools/osiris-trace`), per AI decision:

- `esv::CombatAIAction::CalculateScore` (0x1051e1c2c): on return read the action's final score
  `+0x178`, raw `+0x17c`, cost `+0x1e8`, multiplier `+0x1ec`, flags `+0x1d8`, the float at
  `+0x1e0`, the target entity and the spell; `esv::AiScoreInternal::CalculateScore`
  (0x10508365c) returns the summed score in the low 32 bits and flags in the high 32.
- After `ls::LEGACY_Set<esv::CombatAIAction>::Sort` (0x105213e6c) the first N actions in order
  (settles the tie-break keys).
- `esv::AiHelpers::GetAverageDamageForCharacterNoCache` (0x10523a384) return value (s0).
- `esv::AiHelpers::CalculatePositionScore` / `CalculateMovementScore` /
  `CalculateAiScoreAoO` entries with the tile position, to compare position terms.
- Positions and hit points of every participant at each decision (Osiris `GetPosition` /
  `GetHitpoints` answers, or the transform component), which the current trace lacks.
- A fight with real hit-chance reasoning (an archetype with `MODIFIER_HIT_CHANCE_STUPIDITY`
  below 1, e.g. `melee` humanoids) and with possible kills, where scores differ by target.
