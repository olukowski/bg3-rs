# Stats expression languages

Implemented in `crates/bg3-rules`. Many stats fields (see [stats.md](stats.md)) are small programs
rather than plain values: the effects of a spell, who it can target, what it rolls, which boosts an
item grants. This page describes those languages, the scripting language behind conditions
(Thoth), and the D&D roll mechanics, as far as we've worked them out for the reference build
(4.1.1.7631656).

How we know things: **[code]** means read in the arm64 binary (symbol and address given),
**[data]** means inferred from the shipped files, **[guess]** means not yet confirmed. Examples are
our own, not copied from the game.

## Overview

| Language | Field types (`Structure/Modifiers.txt`) | Example fields | Module |
|---|---|---|---|
| Functors | `StatsFunctors` | `SpellSuccess`, `SpellFail`, `SpellProperties`, `OnApplyFunctors`, `TickFunctors`, `StatsFunctors`, `Properties` (InterruptData), `WeaponFunctors` | `functor` |
| Conditions | `Conditions`, `TargetConditions` | `TargetConditions`, `Conditions`, `RequirementConditions`, `RemoveConditions`, `OnTickRoll`, `Roll` | `condition` |
| Roll conditions | `RollConditions` | `SpellRoll`, `ThrowableSpellRoll`, `OriginSpellRoll` | `condition` |
| Stats expressions | (inside the others), `Damage`, `VersatileDamage`, `AmountOfTargets` | `2d6+StrengthModifier` | `expr` |
| Boosts | `FixedString` | `Boosts`, `DefaultBoosts`, `BoostsOnEquipMainHand/OffHand` | `fields` |
| Requirements | `Requirements` | `Requirements` (`!Combat`) | `fields` |
| Functor contexts | `StatsFunctorContext` | `StatsFunctorContext`, `BoostContext`, `EnableContext` | `fields` |
| Costs | `FixedString` | `UseCosts`, `HitCosts`, `Cost`, `RitualCosts` | `fields` |
| Description params | `FixedString` | `DescriptionParams`, `TooltipDamageList`, `TooltipStatusApply` | `fields` |
| Thoth | `.khn` scripts | condition helpers, AI (Anubis), cinematic rules | `thoth` |

The schema type alone doesn't always decide the language: `Properties` is `PassiveFlags` in
`PassiveData` but `StatsFunctors` in `InterruptData`, so the survey looks up the type per
modifier list (`bg3_rules::survey::language_for`).

### Parse coverage (reference build, base game + Honour modules)

`cargo test -p bg3-rules -- --ignored parse_all_stats_expressions` parses every non-empty
expression field of every entry's own `data` lines. All parse:

| Language | Values | Notes |
|---|---:|---|
| Functors | 6,385 | 0 items the engine would drop |
| Conditions | 3,454 | |
| Roll conditions | 561 | |
| Boosts | 3,410 | 1 item the engine can't map to a boost (`AI_IGNORE:` prefix on a boost) |
| Requirements | 85 | |
| Functor contexts | 810 | |
| Costs | 2,553 | |
| Description params | 4,832 | 11 items are literal text (`50%`, `+4`, unclosed calls) |
| Stats expressions (own fields) | 243 | plus every expression inside functors and params |

`parse_all_khn_scripts` parses all 75 `.khn` files in the install (6,717 lines). Conditions call
about 350 distinct functions.

## Functors

A functor list is a sequence of effects, for example `DealDamage(1d6,Fire);ApplyStatus(X,100,2)`.

Parsing, as the engine does it [code]:

1. **Text-key groups.** The field is first split into groups keyed by an animation text event,
   with everything outside brackets in the `Default` group: `A();Cast2[B();C()];D()` gives
   `Default: A();D()` and `Cast2: B();C()`. Repeated keys merge with `;`. Nested `[`, a stray `]` or
   an unclosed group is an error. `(anonymous)::SplitStringPerTextKey` (0x102015fe8) does this.
2. **Items.** Each group is split on *every* `;`, without looking at parentheses. Each piece is
   trimmed and empty pieces are skipped. `eoc::StatsFunctorListManager::CreateStatsFunctorList`
   (0x101fab168) does this.
3. **Calls.** `eoc::ParseFunctorString` (0x1012b29d0) splits `name(args)rest`:
   - The name is the text before the first `(`.
   - Arguments split at commas at depth 1 and are trimmed. Empty middle arguments are kept; an
     empty last argument is dropped.
   - Whatever follows the closing `)` is the *remainder*, stripped of whitespace, `,`, `:` and `;`.
   - Quotes and brackets are not special.
4. **Contexts.** `StatsFunctorBase::ParseContext` (0x101fa30bc) splits the name at `:`. Every
   piece before the last is a context flag, matched case-insensitively; unknown pieces are
   ignored. The last piece is the functor name. With no targeting flag (only `AI_ONLY` or
   `AI_IGNORE`, or nothing at all), the context is `TARGET|AOE`. The flags, in bit order:
   `TARGET AOE CAST EQUIP GROUND LEAVE_ATTACK_RANGE ATTACKED_IN_MELEE_RANGE
   ATTACKING_IN_MELEE_RANGE PROFICIENCY_CHANGED STATUS_APPLY STATUS_APPLIED STATUS_REMOVE
   MOVED_DISTANCE AI_ONLY AI_IGNORE ATTACK ATTACKED DAMAGE HEAL STATUS_REMOVED OBSCURITY_CHANGED
   SHORT_REST DAMAGED HEALED ABILITY_CHECK (bit 25 unused) LONG_REST CREATE PUSH PUSHED
   INVENTORY_CHANGED ENTER_ATTACK_RANGE PROJECTILE COMBAT_ENDED TURN ACTION_RESOURCES_CHANGED
   SURFACE_ENTER LOCKPICKING_SUCCEEDED INTERRUPT_USED DAMAGE_PREVENTED DAMAGED_PREVENTED ROUND
   COMBAT_STARTED CONSUMED`.
5. **`IF`.** If the first call has a remainder and at least one argument, its first argument
   becomes the condition and the remainder is parsed again as the real functor. The engine never
   checks that the first name is literally `IF`. Because the remainder is stripped of `:`, all of
   `IF(c):F()`, `IF(c)F()` and `IF(c)::F()` work, and all three occur in the data. Contexts must
   come before the `IF`: the second name is not split at `:`.
6. **Name.** There are 62 functor names, matched case-sensitively (`GetStatsFunctorStrings`
   0x101fa54ec). An unknown name, or parameters that fail to parse, makes the engine silently
   drop that functor. We keep such items as `Entry::Dropped`.
7. **Target keyword.** A leading argument `SELF`, `OWNER`, `SWAP`, `OBSERVER_TARGET`,
   `OBSERVER_SOURCE` or `OBSERVER_OBSERVER` selects whom the functor acts on, and is consumed
   (`StatsFunctorBase::Parse` 0x101fa2a20). So `ApplyStatus(SELF,X,100,1)` and
   `ApplyStatus(X,100,1)` differ only by that keyword.

Typed parameters [code]:

- **`DealDamage(amount, type, Magical, Nonlethal, int, -, bool×4, roll)`**
  (`StatsFunctorDealDamage::Parse` 0x101fb0cfc).
  - *amount*: a weapon keyword (`MainMeleeWeapon`, `OffhandRangedWeapon`, `SourceWeapon`,
    `ThrownWeapon`, …) is replaced by `Placeholder0`, and the result is parsed as a stats
    expression.
  - *type*: a damage type, or `<WeaponKeyword>DamageType`; an invalid type drops the functor.
  - When there are 5 or more arguments, the fifth must be an integer.
- **`ApplyStatus(status, chance, duration, param, int, int, condition, bool)`**
  (`StatsFunctorStatus::ParseStatus` 0x101fac62c).
  - *chance* must be a number, or the functor is dropped. We haven't seen the engine store it.
  - *duration* is a stats expression; `-1` means permanent.
  - The 7th argument is a condition, e.g. `not SavingThrow(...)` for "unless the target saves".
  - With 9 or more arguments, or in an `EQUIP`-only context, only the status id is read.

## Conditions

A condition is a Thoth (Lua 5.3) expression written with `and`, `or` and `not`. The engine
compiles each distinct condition string once. It runs the shipped script
`Scripts/thoth/generateConditionString.khn` on the text and compiles the result
(`ls::thoth::Machine::CreateConditionCall` 0x1020c8e44). The script:

- prepends `context = ... \nreturn `;
- textually replaces ` and ` with `&`, `)and ` with `)&`, ` or ` with `|`, `)or ` with `)|`, and
  `not ` with `~`.

So `not Dead() and Enemy()` runs as `return ~Dead()&Enemy()`, using the `ConditionResult`
operators. Consequences, all reproduced:

- **No short-circuit.** Both sides are always evaluated, so a saving throw on the right is rolled
  even when the left side is already false.
- **Precedence.** Lua's bitwise operators bind tighter than comparisons, so `X == 1 and Y()`
  means `X == (1 & Y())`.
- **The rewrite is purely textual.** It also applies inside string literals.

`bg3_rules::condition::condition_to_lua` reimplements the rewrite. The ignored test
`condition_rewrite_matches_shipped_script` runs the shipped script in our interpreter on all
2,039 distinct condition strings and gets identical output.

`convertToChanceString.khn` does the same replacements without the `return` prefix. Its output
is compiled a second time for hit-chance calculation (`CalculateConditionChance` 0x1020cb9d4),
where the roll functions come from `ls::thoth::shared::chance` and return probabilities.

**`ConditionResult`** (`ls::thoth::shared`) [code]:

- **Fields:** `Result`, `Chance`, `ActiveErrors` (errors shown when false), `InactiveErrors` (the
  ones that apply if negated), `Priority`, and surface preferences.
- **Constructor:** `ConditionResult(b, active, inactive, chance)`. Chance defaults to 1 or 0.
- **`a & b`** (0x102020b0c): the result is `a and b` and the chances multiply. If the operands
  disagree, the false one's errors are kept; otherwise the left one's.
- **`a | b`** (0x1020215a0): chance `1-(1-a)(1-b)`. If the operands disagree, the true one's
  errors are kept; otherwise the left one's. The engine's "1 - x" helper leaves values outside
  [-1, 1] unchanged.
- **`~a`:** flips the result, swaps the error lists, and sets chance to `1 - chance`.
- **Mixing types:** `&` and `|` accept a boolean (`{b, 1|0}`) or a number (`{f > 0, f}`) on
  either side. Anything else is an error.
- **Truthiness:** a `ConditionResult` is userdata, so Lua's own `not`/`if` treat it as true even
  when its result is false. Scripts test `.Result`.

**Evaluation** (`ls::thoth::Machine::CheckCondition` 0x1020ca9c0):

- An absent condition is true.
- The chunk runs with the context data as its argument.
- A script error makes the result false. Our `ConditionEngine` reports it as an error instead,
  and the cast executor treats it as false.

The context (`ContextData`) has `Source`, `Target`, `Observer`, their proxies and positions,
`HitDescription`, `AttackDescription`, `StatusId`, `SpellId`, `Passive`, … We provide `Source`,
`Target`, `Observer`, `SpellId`, `StatusId` and `PreferredCastingAbility`.

**Functions** come from two places:

- **Helpers:** Thoth scripts in `Mods/<Module>/Scripts/thoth/helpers/*.khn`, loaded into the
  condition environment module by module (`Machine::CreateEnvironment` 0x1020cd834). `Shared`
  has 382 helper functions (`Self`, `SourceSpellDC`, `Unarmed`, …) and `Honour` overrides one.
- **Natives** exported from C++, in the namespaces `ls::thoth::shared` (about 100, e.g.
  `Tagged`, `Dead`, `HasPassive`, `CalculateSpellDC`), `ls::thoth::shared::condition`
  (`SavingThrow`, `Attack`, `SkillCheck`, …) and `ls::thoth::{server,client}::condition`
  (`Character`, `Ally`, `Enemy`, `HasStatus`, …). The names are in the symbol index as
  `…::_khonsu::function::_Function_<Name>_<hash>`.

Natives that take an entity default to `context.Target` when it's omitted [code: `Dead`
0x102026e54]. `Ally`, `Enemy` and `HasStatus` take a second entity that defaults to
`context.Source` [guess]. In real mode, `SavingThrow` and `Attack` roll dice
(`condition::SavingThrow` 0x102036a9c, `condition::Attack` 0x102036574).

We implement `ConditionResult`, `ConditionError`, `Dead`, `Character`, `Item`, `Player`,
`Party`, `Combat`, `IsDowned`, `Ally`, `Enemy`, `Tagged`, `HasStatus`, `HasAnyStatus`,
`HasPassive`, `Attack`, `SavingThrow`, `CalculateSpellDC` and `RollDieAgainstDC`. Others are a
script error. Enum tables (`Ability`, `AttackType`, `DamageType`, …) are exported as globals;
their labels are not validated.

### Roll conditions

`SpellRoll` and friends hold one condition per text key: `Attack(…);Cast2[Attack(…)]`
(`TextKeyParsingHelpers::SplitStringPerTextKeyNoMerge` 0x102016a7c, then `RPGStats::AddCondition`
per key). A spell whose roll condition is true runs `SpellSuccess`, otherwise `SpellFail`.
`not SavingThrow(...)` therefore means "succeeds when the target fails the save".

## Thoth (`.khn`)

Thoth is the engine's embedded **Lua 5.3**: the binary carries `Lua 5.3`, the stock `llex.c`
token names and the stock `luaB_*` library. It adds one statement, `try block catch Name then
block end`; `catch` sits in the reserved-word table. C++ functions and types are bound through
Larian's "khonsu" layer (`ls::khonsu::CallApi`, `ClassApi`, …).

`.khn` files in the reference build:

| Where | What |
|---|---|
| `Game.pak` `Scripts/thoth/` | `generateConditionString`, `convertToChanceString`, `chanceEnvironment` (a comment), 52 cinematic camera rules under `cinematic/conditions/`, `picking/SelectionPriority` |
| `Game.pak` `Scripts/larian/`, `Scripts/anubis.khn` | `util`, and the Anubis (AI behaviour) framework |
| `Shared.pak` `Mods/Shared/Scripts/thoth/helpers/` | `CommonConditions`, `CommonConditionsDev` |
| `Gustav.pak` `Mods/Honour/Scripts/thoth/helpers/` | Honour-mode override |

Our `thoth` module has two parts:

- **Parser:** the full Lua 5.3 grammar with Lua's operator priorities, plus `try`/`catch`. It
  parses all 75 files.
- **Interpreter:** a tree-walker covering what conditions and their helpers need: closures,
  varargs, multiple returns, tables with `__index`/`__call`, `try`/`catch`, `pcall`/`error`,
  `math.floor/max/min/abs`, `string.gsub/find/format/sub/rep` (Lua patterns without captures),
  `table.insert/concat/remove/unpack`, `pairs`/`ipairs`/`next`/`select`/`setmetatable`/`require`.
  Not supported: `goto` at run time, coroutines, the `debug`/`io`/`os` libraries.

## Stats expressions

Damage, healing, durations and description values use a small formula language
(`eoc::StatsExpressionObject::Parse` 0x101f84b70) [code]. It has no tokenizer:

1. Trim (space, tab, CR, LF only).
2. While the text starts with `(` and ends with `)`, strip one pair. The two are not checked
   against each other, so `(1)+(2)` becomes `1)+(2` and fails.
3. Try each node kind in order. "Not mine" moves on to the next kind; any other error stops.
   The order:
   - **Roll:** `NdM`, `dM`, `NdM+B`, `NdM-B` or a non-negative integer. M is one of 4, 6, 8, 10,
     12, 20 or 100; N is at most 255 (`eoc::ParseRoll` 0x101c53668). The word `ImprovisedWeapon`
     is also a roll.
   - **`+`, then `-`, then `/`, then `*`.** Each splits at the *first* such operator at bracket
     depth 0 (`StatsExpressionObjectOperator::ParseLeftAndRight` 0x101f8bd8c), and refuses an
     operator at position 0. So `+` binds loosest and chains are right-associative:
     `10-4-3 = 10-(4-3) = 9`, `12/2*3 = 12/(2*3) = 2`.
   - **`max(a, b, …)`:** lowercase `max(` only.
   - **`foreach(count, body)`.**
   - **`PlaceholderN`.**
   - **`ResourceRoll(Name, level)`.**
   - **Variable:**
     - Suffixes `Modifier`, `Flat`, `SavingThrow`, `DialogueCheck`, `Advantage`,
       `Disadvantage`.
     - Prefixes `Target.`, `Owner.`, `Cause.`.
     - `LevelMapValue(X)` and `ClassLevel(X)`.
     - Built-ins, numbered 0–10: `SpellCastingAbility`, `ProficiencyBonus`, `Level`, `SpellDC`,
       `WeaponActionDC`, `UnarmedMeleeAbility`, `CurrentHP`, `MaxHP`, `SpellPowerLevel`,
       `TadpolePowersCount`, `DamageDone`. These are matched by a compiled `switch`, not a
       string table.
     - Ability names, skill names, and otherwise the raw name.
   - **Negate:** a leading `-`. Hence `-1d4` works, and `-a-b` is `-(a-b)`.

   Our raw-name fallback only accepts identifier-like text, so that `Negate` can be reached.

Evaluation [code] (resolver table at 0x1086efeb0):

- **Arithmetic:** 32-bit integers. Division truncates toward zero, and **division by zero gives
  0**.
- **Average mode** (tooltips): a roll is `count*(faces+1)/2 + bonus`, so 1d6 = 3 and 2d6 = 7.
- **`max`:** compares every argument by its *average*. The largest wins, a tie goes to the
  non-dice argument, and then **only the winner is rolled**. So `max(1, 1d4-2)` keeps 1, while
  `max(1, 2d6)` rolls 2d6.
- **`foreach`:** rolls the body `count` times. In average mode it multiplies.
- **Damage context:** `+` merges damage entries and `/` divides each, so `(8d6)/2` halves the
  rolled total.
- **`LevelMapValue(X)`:** looks up a level-map series (`Public/<Module>/Levelmaps/
  LevelMapValues.lsx`; values are rolls such as `1d10` or numbers). The result is rolled like
  dice, so it is also doubled on a critical hit. We pick the level as: exact `LevelN`, else the
  highest defined level below, else `FallbackValue` [guess, beyond the exact case]. Which level
  is used (character or class level, `PreferredClassUUID`) is [guess].
- **Weapon placeholder:** at run time `Placeholder0` is the weapon's damage roll
  (`stats::GetWeaponDamage` 0x101fbf724, only partly read). We use weapon dice plus the attack
  ability modifier, with only negative modifiers for off-hand attacks [guess, the 5e rule].

## Smaller languages

- **Boosts:** `;`-separated at depth 0, because arguments may contain `;` as in
  `BlockRegainHP(A;B)`. Each item is `[IF(cond):]Name(args)`. `(anonymous)::ParseBoosts`
  (0x10118d188) calls `ParseFunctorString` once, plus once more after an `IF`, and recurses on
  leftover text. So `A(1):B(2)` and `A(1)B(2)` are read as two boosts (both occur in the data)
  [code, skimmed]. Boost arguments are kept raw; `eoc::ParseBoostData` (0x101180124) interprets
  them per boost type. The data uses 125 boost types.
- **Requirements:** `;`-separated `[!]Name[ param]` (`ParseRequirements` 0x1020fd4fc) [code].
- **Functor contexts:** `;`-separated event names (`OnDamage;OnCast`) [data].
- **Costs:** `Resource:amount[:group:level]`. The amount is a number, or a formula such as
  `Distance*0.5` [data].
- **Description params:** `;`-separated values that fill `[1]`, `[2]`, … in loca strings. Each
  value is a GUID, a stats expression, a call like `DealDamage(1d6,Fire)` or `Distance(3)`, or
  literal text. Text-key groups occur here too [data; the engine's parser was not traced].

## Rolls

**Random numbers** [code]:

- **Generator:** a 64-bit Mersenne Twister per *roll stream*. Every draw re-seeds an engine from
  the stream seed, discards as many outputs as the stream's draw counter, takes one, and
  increments the counter (`RollManager::InternalRollDice` 0x101c6139c,
  `RNGEngine::GetNextInteger` 0x101c5efb0). That equals reading one MT19937-64 sequence in order,
  which is what `bg3_rules::dice::RollStream` does.
- **Die value:** `min + raw % (max - min + 1)`.
- **Not modelled yet:** a "bell curve" mode and a success/failure *debt* system for some stream
  types (`RollDiceAgainstDC` 0x101c61668 → `CheckStreamAgainstDC` 0x101c617f8). The debt system
  is probably the Karmic Dice option [guess]. When it applies, it can force a roll to pass or
  fail. Inputs: `GetChanceToPass` 0x101c538f4 and the `RollStream*DebtRange`/`…ConsumeMultiplier`
  keys in `Data.txt`.

**d20 with advantage** [code]: advantage and disadvantage are two flags; both set cancel,
however many sources each has. d20 rolls draw two dice per die and keep the higher or lower
(`InternalRollDiceWithBoosts` 0x101c62184). Which die counts on a straight roll is [guess]; we
keep the first.

**Attack rolls** [code: `stats::GetAttackRollResults` 0x101fc88e4]:

- **Total:** d20 + ability modifier + proficiency (if proficient) + boosts. Summing the bonuses
  was not traced; the ability is Strength, Dexterity for ranged weapons, and the better of the
  two for finesse weapons [guess, 5e].
- **Natural 1:** critical miss. **Natural ≥ 20 − reduction:** critical hit. Both decide the
  outcome regardless of AC. Otherwise the attack hits on total ≥ AC.
- **Not modelled:** the `CriticalHit(..., Never)` boosts that cancel this, and the distance
  condition on `CriticalHit` boosts.

**Saving throws** [code: `condition::SavingThrow` 0x102036a9c]: d20 + modifier + proficiency
(if proficient in that save) against the DC; pass on total ≥ DC. **Natural 1 and 20 have no
special effect.**

**Ability checks** (dialog rolls; `bg3_rules::rules::roll_ability_check`): d20 (with
advantage/disadvantage) + ability modifier + proficiency bonus when proficient in the skill
(twice with expertise) + flat `Skill(X, n)` boosts; for `RollType` `SavingThrow` the
saving-throw proficiency counts instead, for `RawAbility` nothing. Pass on total ≥ DC. The
modifier sources are the ones `eoc::active_roll::DetermineStaticModifiers` gathers
(`CheckProficiencyBoost`, `CheckExpertiseBoost`, `CheckSkillBoost`) [code, only the sources;
other boosts like roll bonuses and Guidance are not modelled]. A natural 20 always passes and a
natural 1 always fails, reported as critical success/failure [guess: how the game presents dialog
checks, branch not located]. The DC is the `Difficulties` value of the roll's
`DifficultyClasses` entry.

**Spell save DC** [code: `ls::thoth::shared::CalculateSpellDC` 0x10202c118]:
`BaseSpellDifficultyCheck` (8 in `Data.txt`) + proficiency + DC boosts +
`floor((score-10)/2)`. With no stats, the modifier is −5.

**Damage** [code]:

- **Critical hits** double each roll's dice count, not the flat bonus (`stats::CalculateDamage`
  0x101fd7408).
- **Resistance** (`stats::ApplyResistance` 0x101fe0b5c):
  - Damage is clamped to ≥ 0, then flat `DamageReduction` boosts are subtracted.
  - Halving computes `d − round(d/2)`, which is `floor(d/2)`.
  - The result is clamped to 0..46340.
  - Vulnerability (×2) and immunity (0) follow the D&D rules; their branches were not located.

Ability modifier: `floor((score − 10) / 2)`. The proficiency bonus is stored per entity, coming
from the `ProficiencyBonusScaling` level map, so it's a query, not a formula.

## Evaluation API (`bg3-rules`)

- **`context::RulesContext`:** the game-state interface the simulation implements:
  - Required: identity, ability scores, level, proficiency, AC, HP, and `set_hp`.
  - Defaulted: save and skill proficiencies, expertise, skill bonuses, spellcasting ability,
    weapons (`weapon` by hand, `attack_weapon` by attack type for separate melee and ranged
    slots), the unarmed attack ability (`UnarmedAttackAbility`, used by unarmed attacks and
    `UnarmedMeleeAbilityModifier`), resistances, attack and save advantage, critical
    threshold, level maps, relations,
    tags, statuses, passives, generic `property` lookup for scripts, and the status mutations.
  - `testworld::TestWorld` is a small in-memory implementation; `bg3_sim::Sim` implements it
    over the game's entities (`crates/bg3-sim/src/rules.rs`, characters' stats from their
    `Character` stats entries).
- **`dice`:** `RollDefinition`, `DiceRng`, `RollStream` (seeded MT19937-64), and `ScriptedDice`
  for tests.
- **`rules`:** `roll_attack`, `roll_saving_throw`, `roll_ability_check`, `spell_dc`,
  `apply_resistance`, `weapon_damage_roll`.
- **`condition::ConditionEngine`:** a Thoth machine with the natives registered. Load the
  shipped helpers with `install::load_helpers`; `evaluate` returns the `ConditionResult` and any
  rolls it made.
- **`exec::Executor`:** runs functors and single-target spell casts. The cast does
  `TargetConditions`, then the default `SpellRoll` group, then `SpellSuccess`/`SpellFail`, then
  `SpellProperties`, in the `TARGET`/`AOE` pass. It writes a readable log, and reports what it
  skips (ground pass, other text-key groups, unimplemented functors). `events` has the same as
  data (`ExecEvent`: rolls with their breakdown, damage before and after resistances, statuses,
  healing); `bg3-sim`'s combat turns them into notifications and story events
  (`docs/research/combat.md`).
- **`bg3x rules eval "<text>" --kind condition|functor|roll|expr`** (alias `e`) evaluates against
  two sample combatants. `bg3x rules cast <Spell>` runs a spell between them; `bg3x rules survey`
  prints the coverage table.

## Verification

- **Unit tests** (self-authored fixtures): the parser quirks above, `ConditionResult` algebra,
  attack, save and damage rules, the MT19937-64 reference value, and casts with scripted dice.
- **Ignored install tests:** parse every expression field and every `.khn`; check the condition
  rewrite against the shipped script; run a melee mini-encounter with `Target_MainHandAttack`'s
  real fields, the shipped helpers and a fixed seed.
- **Evaluation coverage:** `evaluate_install_target_conditions` evaluates every distinct
  `TargetConditions` value with the shipped helpers and two synthetic entities. 443 of 697
  complete. The rest stop at natives we don't have yet; the most common is `CanStand` (needs world
  geometry), then `IsImmuneToStatus`, `ConditionErrorData.MakeFromNumber`, `Grounded` and
  `WieldingWeapon`.
- **Not yet compared against runtime traces** of the original game. Roll outcomes need a
  captured roll stream (seed + counter) to verify.

## Open questions

- Which d20 counts on a straight roll (both dice are drawn), and how stream seeds are chosen per
  roll type.
- The karmic-dice and bell-curve modes: when are they active, and for which streams?
- Raw expression names the evaluator doesn't resolve: `UnarmedDamage` and
  `MartialArtsUnarmedDamage` in `DealDamage` (the tooltip code rewrites the latter to a level map
  plus Dexterity), status references like `X.Duration`, and damage-type words used as
  description params.
- How level maps choose the level, and what they do above the highest defined level.
- Weapon damage composition (`GetWeaponDamage`), off-hand modifier rules, and how unarmed damage
  is computed.
- Attack bonus composition (`GetWeaponAttackRoll` 0x101fcc6b4), the `CriticalHit` boosts,
  vulnerability and immunity.
- The boost parser in full, the 125 boost types' arguments, and the description-param parser.
- Whether `ApplyStatus`'s chance argument is used at all.
- `Size.value` ordinals for `Size.None` (we use −1).
