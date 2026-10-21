# libOsiris runtime log

libOsiris can write a plain-text log of everything the rule engine does at runtime. We switch
it on in the running game with `tools/osiris-trace/enable_log.py` (lldb: break on
`COsiris::InitGame` or `COsiris::Load`, open the log with `COsiris::OpenLogFile`, set
`_ParserFlags`), start a new game or load a save, and keep the log in `.cache/traces/`
(gitignored, never committed). When loading a save, the flags are set again after `Load`
returns. `osiris_trace_probes` additionally stays attached and logs a few engine functions
(initiative rolls, `RollDie`, `ApplyDamage`) to a second file until `.cache/traces/STOP`
exists. The parser lives in
`crates/bg3-osiris/src/trace.rs`, the replay/compare in `crates/bg3-osiris/src/replay.rs`.
This page describes the format in our own words; the format strings were checked against the
strings of `libOsiris.dylib` (4.1.1.7631656, arm64 slice).

## Switching it on

- `COsiris::m_LogEnabled` plus `_ParserFlags` bit 19 (0x80000): rule firings, actions,
  calls, queries, goal INIT/EXIT, events. Bit 24 (0x1000000): rejected inserts with a dump of
  the database. The game's own default flags are 0x004810a0 (the value stored in the story
  header as "debug flags"); our captures use 0x014810a0.
- Bit 19 also brings join diagnostics (`*** Multiple match ***`, `*** Match n: …`) that we
  record but do not compare.

## Captures so far

| File | Content | Size |
|------|---------|------|
| `snapshot-early.log` | InitGame → `GameModeStarted` → character creation (`SYS_CC_I`), cut while the game ran | 64 k lines, 8 508 events |
| `full-nautiloid.log` | the same session to the end: `CharacterCreationFinished`, `LevelUnloading("SYS_CC_I")`, `LevelLoaded`/`LevelGameplayStarted`/`LevelGameplayReady("TUT_Avernus_C")`, first Nautiloid dialogs | 135 k lines, 12 381 events |
| `combat-<timestamp>.log` + `combat-probes-<timestamp>.log` | loaded save on the Nautiloid → the first imp fight (cleric avatar + Lae'zel vs 3 imps); probes for initiative, dice and damage; UI screenshots alongside | 3.4 MB |

**Story.** The goal numbers in the log (`ModWrapper_Gustav (#904)`, `ZZZ_Tutorials (#941)`)
only fit `Mods/GustavX/Story/story.div.osi` from `GustavX.pak` (941 goals, story version
4.8.400.7143220), not `Mods/GustavDev/Story/story.div.osi` (935 goals, where #904 is
`QRY_Characters`). A new campaign game on the reference build runs the GustavX story;
`DB_BG3_Released_Versions` and `GetModuleVersion("GustavX", …)` in the log agree.

## Line kinds

Blank lines and lines made only of `-` are layout. Everything else is one record; the only
multi-line record is a database dump.

**Nesting prefix.** Lines written while rules fire start with the value of `g_RuleNesting` and
an arrow: `0>`, `1-->`, `2---->`, … (two dashes per level). The prefix is the depth *before*
the line's own effect: a rule fires at `N>`, its actions print at `N+1`, and its
`[end of rule action part]` is back at `N>`. Records without a prefix (events, queries, goal
sections, diagnostics) carry no depth.

| Record | Example | Written by |
|--------|---------|-----------|
| banner | `### Run time log:` | log open |
| global INIT | `exec global INIT section:` | `COsiris::InitGame` |
| INIT walk | `exec INIT section of goal Name (#12):` (unquoted, no END) | `InitGame`, once per goal whose rules are enabled when the walk reaches it |
| woken INIT | `exec INIT section of goal "Name" (#2):` … `… (#2) END` | `COsiGoal::EnableRules` |
| EXIT | `exec EXIT section of goal "Name" (#11):` … `… (#11) END` | `COsiGoal::SetCompleted` |
| GoalCompleted | `1--> GoalCompleted; ("Name") (GoalID == 11)` | the action |
| action | `1--> DB_Foo( 1, "x" ) [add fact]` | `COsiFunctionParseData::Fire`, before it runs |
| | kinds: `[add fact]`, `[delete fact]` (`NOT DB`), `[Osiris procedure call]`, `[DIV call]` (engine call), `[Osiris internal call]` (`Sys*`) | |
| call failed | `*** call failed! ***`, sometimes prefixed `line 1: error: ` (it goes through the error channel) | after a `[DIV call]` the engine refused |
| rejected insert | `## insert (add fact) failed: tuple: 1, "x"` + database dump | a database start node already holds the tuple (bit 24) |
| rejected join insert | `## insert failed: tuple: …` (no dump) | a materialised join node already holds the tuple |
| database dump | `DBase contents of DB_Foo( (INTEGER)_, (STRING)_ )`, a dashed rule, one row per line, blank line | after a rejected insert, after `*** Multiple match ***` |
| event | `>>> call COsiris::Event` then `>>> event Name(a, b)` | `COsiris::Event` |
| ignored event | `>>> call COsiris::Event` then `event Name/2 ignored (no rules defined using it)` | event without a start node |
| rejected event | `<<< event ignored: …` | argument without type (not seen yet) |
| rule fires | `0> RuleActionPart of THEN at line 50976 fires. Rule variables: 1, 2` | `CReteRuleActionPart::Add`, after binding |
| rule end | `0> [end of rule action part]` | same, after the last action |
| query | `exec [DIV query] Name( a, [out] (INTEGER) )`; kinds `DIV query`, `Osi user query`, `internal query` | `CReteQuery::Valid`, before running it |
| query failed | `*** Query failed! ***` | the query returned false |
| query result | `     Query returns: Name( a, 2 )` (five spaces) | after the query, whether it failed or not; **not written for queries without parameters** |
| diagnostics | `*** Multiple match ***`, `*** Left node: …`, `*** Match 3: (var 1 = …)`, `## (node n) …` | joins/valid tests; the number of `*` grows with join depth |

`RuleActionPart of THEN at line N` names a rule by the source line of its THEN part; the
compiled story stores that line (`RuleNode::line`), and in GustavX it identifies the rule.
Rule variables are the rule's whole persistent variable vector after binding the token, so
variables the current token does not bind show the value from the rule's previous firing (or
`(unknown)`).

Query records nest: a user query's QRY rules (and the queries in their conditions) are logged
between its `exec` and its `Query returns`. For a failing user query, `*** Query failed! ***`
comes right before its `Query returns`. A DIV query that "succeeds" but returns a value that
does not match a bound out-parameter (`IsItem(X, [out] 1)` → `IsItem(X, 0)`) is *not* marked
failed: the mismatch is decided afterwards in `UpdateToken` and is silent.

## Values

| Printed | Meaning |
|---------|---------|
| `12`, `-40` | INTEGER (and INTEGER64 printed through an alias) |
| `(INTEGER64)7143220` | INTEGER64 (format `(INTEGER64)%-lld`; not seen in our captures) |
| `100.0`, `-1.0`, `2.5` | REAL, `%.1f` |
| `"text"` | STRING (no escaping) |
| `S_Player_Hero_9f8e…`, `NULL_0000…` | GUIDSTRING, bare |
| `(CHARACTER)S_…` | a value whose type id is an alias type: `(%s)%s` |
| `DEATHTYPE.DOT` | enumeration value, `%s.%s`; when two labels share a value the real label wins over `__OSIRIS_ENUM_FALLBACK` |
| `(unknown)` | unset value of type 0 |
| `(INTEGER)`, `(GUIDSTRING)` | unset value with a type (e.g. an unbound out-parameter) |
| `(undef value)` | any unset value in a `Query returns` line |
| `(STRING)_` | a column in a `DBase contents` header |

The `(TYPE)` prefix shows the value's **runtime type id**, not the declared one, which makes
the log a check on type propagation (see below). `Query returns` lines and event lines print
the engine argument list, which carries root types only, so they never show a prefix.

## What the captures show (VM semantics)

Replaying both captures through `bg3-osiris` (`cargo test -p bg3-osiris --test replay --
--ignored`, `bg3x osiris-replay`) matches every record: facts, rule firings, engine calls,
queries, PROCs and goal sections, in order, including the printed type annotations.

- **Events are queued, never nested.** None of the 12 381 `>>> call COsiris::Event` lines
  falls inside a rule firing, a query or a goal section; every event starts at depth 0 after
  the previous one is fully evaluated. The engine collects events raised by its calls and
  delivers them later.
- **Startup sequence (new game).** `InitGame` (global INIT, then the INIT walk over the 92
  root goals) runs before any event. Then: `FlagSet(Debug_CAMP_IgnoreCampTypeRestrictions…,
  NULL_…, 0)`, `GameModeStarted("Campaign", 0, 0)` (which completes `__Shared_Campaign`,
  `__Start`, `ModWrapper_Gustav` and `Z_Shared_Campaign_At_End`, waking most of the story), a
  long stream of entity events (`HitpointsChanged`, `TagSet`, `AddedTo`/`Equipped` for every
  character's gear, `EnteredLevel`, …), `LevelLoaded("SYS_CC_I")`,
  `CharacterCreationStarted()`, `LevelGameplayStarted("SYS_CC_I", 0)`,
  `LevelGameplayReady("SYS_CC_I", 0)`, `MovieFinished("GUS_CGI01_Part1")`, the player's
  choices (`CharacterMadePlayer`, `CharacterSelectedForUser`, …),
  `CharacterCreationFinished()`, `LevelUnloading("SYS_CC_I")`, `LevelLoaded("TUT_Avernus_C")`,
  `LevelGameplayStarted("TUT_Avernus_C", 0)`, `LevelGameplayReady("TUT_Avernus_C", 0)`.
  No `RegionStarted` in this range. Goal completions: `GameModeStarted` completes
  `__Shared_Campaign`, `__Start`, `ModWrapper_Gustav`, `Z_Shared_Campaign_At_End`;
  `CharacterCreationStarted` completes `__AAB_CharacterCreation_At_Start`;
  `CharacterCreationFinished` completes `Z_Shared_CharacterCreation`, `Start`,
  `Z_Shared_TutorialCharacterCreation`; `LevelLoaded("TUT_Avernus_C")` completes
  `_CharacterCreation_Monk`, `Tutorial`, `Z_Shared_CharacterCreation_AfterEvents`;
  `LevelGameplayStarted("TUT_Avernus_C", 0)` completes `TUT_WakeUp`.
- **Nesting.** `g_RuleNesting` reaches 100 (a `PROC_DoNTimes`-style recursion counting to 100
  through `IntegerSum`); typical chains stay below 10.
- **Functions are keyed by name and arity.** `SetFlag/3` is a story PROC that calls the engine
  call `SetFlag/4`; both appear in the log under the same name.
- **Function names in actions** print as written in the action (`DB_Noop`), while the lookup
  is case-insensitive (the database is `DB_NOOP`).
- **Engine values keep their own type.** An event argument for a `CHARACTER` parameter stays
  a GUIDSTRING (rule variables and the facts made from it print without prefix); so does a
  GUIDSTRING answer for a `TAG` out-parameter.
- **Unbound out-parameters** are passed typed with the declared type's *root*
  (`[out] (GUIDSTRING)` for a `TAG` parameter), or type 0 (`[out] (unknown)`) where the
  adaptor has no variable for the column.
- **User-query arguments** reach the QRY rules as root types (`(TAG)X` in the caller,
  bare `X` in the QRY rule's variables): they travel as an engine argument list.
- **EXIT END ordering.** `SetCompleted` writes the EXIT `END` line after the subgoals have
  been checked and woken, so their INIT sections appear inside the EXIT block.
- **Engine-query answers** for the whole session are in the log (13 543 DIV queries); the
  only call that failed was one `StopFollow`.
- A DIV query can succeed and leave its out-parameter unset (`GetHostCharacter` before a host
  character exists): `Query returns: GetHostCharacter( (undef value) )` without a failure,
  and the rule fires with that variable `(unknown)`.

## Replay

`replay::replay(story, log)` cuts the log into segments (InitGame, then one per event), runs
`Vm::init_game` and each logged event (arguments converted from the printed values: numbers
by the declared root, strings as STRING, bare text as GUIDSTRING, enums by label) with a
`ReplayHost` that answers DIV queries from the log (matched by name and input arguments,
preferring answers from the same event) and fails exactly the calls the log marks failed. The
VM writes the same records (`Vm::set_logging`, `Vm::take_log`; `trace::render_log` prints
them in libOsiris' format) and the two record streams are compared per segment, by a
normalised key that drops type prefixes; a second pass compares the full text including the
prefixes. Results: first divergence with context, match rates per category (in-order, LCS per
segment), per-database fact histories, unanswered queries.

`bg3_sim::replay::sim_replay` (`bg3x sim-replay`) runs the same log with `bg3-sim` answering
the queries it implements and raising the events it should, and grades it against the log; see
[osiris-engine-api.md](osiris-engine-api.md).

Limits of the method: queries without parameters print no result line (their answer is taken
from the absence of `Query failed`); diagnostics and database dumps are not compared; a trace
that starts mid-game (after a save load) has no InitGame segment and would need the save's
story state.
