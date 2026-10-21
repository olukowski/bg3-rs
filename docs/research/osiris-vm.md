# Osiris VM: how a compiled story is evaluated

Implemented in `crates/bg3-osiris`. Source: static reading of `libOsiris.dylib` (arm64 slice,
reference build 4.1.1.7631656) with REA/Ghidra. Prose only; symbol names are cited so each
point can be checked again. Data model: `crates/bg3-story`; file format:
`docs/formats/osiris-story.md`.

Status: everything below is confirmed **statically** (read in the binary) unless marked
*inferred*. Nothing has been checked against a runtime trace of the game yet; see
[Open questions](#open-questions) and [Runtime trace plan](#runtime-trace-plan).

## Values and strings

- A value (`COsiTypedValueBase`, 16 bytes) is an 8-byte payload, a `u16` type id, a byte
  (variable index / column) and a flags byte (bit 3 = VALID; see the format doc). Operations
  first resolve the type's **root** by walking the alias table until a built-in id (< 6) or
  0x7f.
- **Strings are interned** (`COsiStringTable::AddStr`). The payload of a STRING/GUIDSTRING packs
  three 21-bit ids: the exact string, the ASCII-lowercased string, and — only for values whose
  root is GUIDSTRING — an id for the 128-bit UUID parsed from the **last 36 characters**
  (`8-4-4-4-12` hex, either case). If the suffix is not a well-formed UUID, or is all zeros, or
  the string is shorter than 36 characters, the UUID id is 0. The empty/null string is all-zero.
- `COsiTypedValueBase::operator==` / `operator<` require equal roots; otherwise they log
  "invalid comparison" and answer false. INTEGER/INTEGER64/REAL compare by value; **STRING
  compares the lowercased id (case-insensitive)**; **GUIDSTRING compares the UUID id**. So
  `"S_Player_Laezel_58a6…fb12" == "58a6…FB12"`, and every GUIDSTRING without a valid UUID
  (`""`, `NULL_00000000-0000-0000-0000-000000000000`, `"foo"`) equals every other such string.
  These operators drive database indexes, adaptor constant checks and join matching.
- `Compare(a, b, op)` (compare nodes) is different:
  - numbers mix: REAL on either side → `f32` comparison (the other side converted), else
    INTEGER64 on either side → `i64`, else `i32`;
  - STRING op STRING: `==`/`!=` by lowercased id, `< <= > >=` by `strcasecmp` of the full text;
  - GUIDSTRING op GUIDSTRING: `==`/`!=` by UUID id, ordering by `strcasecmp` of the last 36
    characters;
  - GUIDSTRING op STRING: everything (including `==`) by `strcasecmp` of the last 36 characters;
  - STRING op GUIDSTRING, number vs string, type 0 or DIV objects: "Invalid compare" → false.
- `COsiVariable::Assign` copies the source's type id too; the payload only if that type is
  non-zero and the source is VALID.

## Databases (`CReteDBase`)

- Storage is a `std::list<CTuple>`; `_insert` appends at the end. Each column has a
  `std::multimap<COsiTypedValue, list iterator>` index keyed by `operator<`.
- `Select(desc)` (`desc` = `(column, value)` pairs) first copies the whole tuple list, then for
  each constraint bumps a per-tuple hit counter through that column's index (`_TupleRefs`), and
  finally keeps, **in list order**, the tuples hit by every constraint. Result: **every lookup
  returns rows in insertion order**. A constraint whose value is not VALID asserts ("Value is
  not valid! Investigate!") and yields nothing. No constraints = all rows.
- `find(tuple)` intersects the per-column hits and expects at most one row. It fails for a
  zero-arity database, an empty tuple or an unset value.
- `insert` = `find`, then `_insert` if absent; returns "new". A zero-arity database stores
  nothing (`_insert` is guarded by the arity) yet reports every empty tuple as new.
- `erase(tuple)` (`COsiColumnIdxValuePairList::From` keeps only VALID columns): if some value
  is unset, **every** row matching the set columns is erased (Select); otherwise the exact row.
  Returns whether something was erased. Re-inserting a deleted row appends it at the end.
- `Valid(tuple)`: exact `find` if all values are set, else "any row matches the set columns".
- Join results are copied out (`CTupleList`) before propagation, so rules triggered while
  iterating cannot change the iteration.

## Tokens and adaptors

- Start nodes deal in physical tuples (one value per parameter); below them, *tokens*
  (`COsiVarIDValuePairList`): ordered `(variable, value)` lists.
- `CReteAdaptor` (on every edge, owned by the edge's *target* node: `Adaptor(entry)`):
  - `token(tuple)`: walks the columns; a column with a logical index becomes a binding; a
    column without one is checked against the next adaptor constant **if that constant is for
    this column** (the constants are walked in order); a mismatch drops the tuple. Afterwards a
    variable bound by two columns must have equal values (`DB(_X, _X)`).
  - `selectDesc(token)`: each token binding whose variable maps to a column (the
    logical→physical `std::map`), in token order, then all constants.
  - `InitTuple(token, tuple)`: writes mapped bindings and constants into a tuple of the target
    database's arity; other columns stay unset.
  - `undefToken()`: every mapped variable, unset, in variable order.
- `COsiVarIDValuePairList::Merge(out, a, b)`: all of `a`, then the entries of `b` whose
  variable is not in `a`.
- Compiled networks (checked in GustavDev): edges between inner nodes carry an identity
  adaptor (variable *i* ↔ column *i*); edges from start nodes map the condition's columns.

## Start nodes

- `CReteStartNode::Add(COsipParameterList*)`: with a database, the tuple is inserted unless
  `find` succeeds — **a known fact does not propagate** (no re-trigger). Events/PROCs have no
  database and always propagate. `Del` erases (see `erase`) and propagates only if something
  went.
- `ForwardAddToken`/`ForwardDelToken` visit the successors **in file order**. A successor
  whose connection goal has flag bit 1 (rules disabled) is skipped, reading the flag as each
  successor is reached. Otherwise the target's adaptor turns the tuple into a token and the
  target's `Add`/`Del` is called. Facts that arrive while a goal sleeps are **not** replayed
  when it wakes.
- `Add`/`Del` with a token on a start node, and `Add`/`Del` on query nodes, do nothing.

## Inner nodes

Node fields: a binary node has `next`, left/right parent, left/right adaptor and two
*generator links* (`DbLink`: the closest database whose tuples can regenerate the parent's
tokens, the first edge below it, and the distance; "no database" = indirection 0 with no node,
or 255 for query parents). `pTokenGenerator` picks the left link if its indirection ≤ the
right one, else the right link (so an event-rooted chain has no generator).

**AND** (`CReteBinaryNode::Join` → `JoinInternal`). A token arrives on `entry` from `from`. The
*other* parent is the right one if `from` is the left parent, else the left one (decided by node
id, not by entry); the other side's adaptor is `Adaptor(flip(entry))` (flip table at 0x8cf74:
0→0, 1→2, 2→1).
1. If this node is waiting for a generator replay (field +0x38 set), the incoming token is a
   replayed one: merge(waiting, incoming) goes straight to `next.Add` (not materialised).
2. Else, if the other parent has a database: `Select(selectDesc(token))` on it, adapt each
   copied row, merge(token, row token) and call `CReteChildNode::Add` per match.
3. Else, if the other parent has a token generator: set +0x38 = token,
   `CReteNode::GenerateAllSatisfying(generator, token, connection)`, then clear +0x38 (set to
   null, not restored). `GenerateAllSatisfying` selects the generator database's rows through
   the first edge's adaptor using the token, and for each row calls `child.Add(merge(token,
   row token))` on that first node; the tokens travel down (doing their own joins) and come back
   through path 1.
4. Else (queries, events): `out = merge(token, undefToken(other adaptor))`, and if
   `other.Valid(out, adaptor)` holds, `next.Add(out)`.

Re-entrancy quirk kept as is: while a node waits (+0x38), *any* token reaching it takes path 1.

**`CReteChildNode::Add`** (used by AND path 2, NOT-AND, compare): if the node has its own
database (a materialised join; 2 745 ANDs, 1 850 NOT-ANDs, 292 compares in GustavDev), the
token is written through the *successor edge's* adaptor into a tuple; a tuple already present
stops propagation, otherwise it is inserted. Then `next.Add(token)`.

**`CReteChildNode::Del`** (= `CReteAnd::Del`; compare nodes inherit it — *inferred*, there is
no `CReteRelCondition::Del`): only a node **with** a database propagates deletions: it erases
the tuple built from the token (with unset columns → every match) and, if something was erased,
calls `next.Del(token)`. Nodes without a database swallow deletions; rule nodes ignore them.

**NOT-AND** (`CReteNAnd`):
- `Add` from the left: if `right.Valid(token, right adaptor)` the token stops; otherwise
  `CReteChildNode::Add`. `Add` from the right (a fact appeared): `CReteChildNode::Del` — a
  retraction, only visible if the node is materialised.
- `Del` from the left: `CReteChildNode::Del`. `Del` from the right (a fact disappeared): if the
  right side still holds, stop; otherwise run `Join` (above) with the token — the matching left
  tokens are re-derived and **the rule fires again**.
- `Valid`: with a database, the database; else `left.Valid(merge(token, undef(left)))` and not
  `right.Valid(merge(token, undef(right)))`, binding the merged token.

**AND `Valid`**: with a database, `Valid` of the tuple built through the given adaptor; else
`left.Valid && right.Valid` on the same token. **Fact `Valid`**: the database. **Event
`Valid`**: always false (a fact arriving later never revives an event).

**Compare** (`CReteRelCondition::Add`): the left operand is the token value whose variable is
`left_var` (else the stored constant), the right likewise (`else if`, so `left_var ==
right_var` leaves the right as the constant); `Compare` true → `CReteChildNode::Add`. `Valid`:
the database if materialised, else `parent.Valid` then the comparison.

**Rule** (`CReteRuleActionPart::Add`): assigns each token binding into the rule's persistent
variable vector (`COsiVariable::Assign`), increments `g_OsiUserQueriesExecuted` if it is a QRY
rule, then fires the actions in order with `g_RuleNesting` incremented. Firing is synchronous
and depth-first.

## Actions (`COsiFunctionParseData::Fire`)

- `goal_or_hook > 0`: `COsiGoal::SetCompleted`. `< 0`: `ExecuteDebugHook` (logging only).
- Otherwise the function is looked up by name (case-insensitive, `strcasecmp` in the function
  search index) and dispatched on its kind:
  - **call** → engine callback with the handle and an argument list
    (`COsipParameterList::pParameterListDesc`: out-parameters become typed empty arguments).
    The result only matters for logging ("call failed!").
  - **database** → `NOT`: `CReteStartNode::Del`; otherwise `Add` wrapped in
    `SaveSharedParameters`/`RestoreSharedParameters`.
  - **proc** → `Add` on its start node, also wrapped in save/restore.
  - **syscall** → `InternalCall`.
  - event / queries → "should not get here" (events only in a special parser mode).
- **Shared variables.** At load, `CReteRuleActionPart`'s constructor replaces every action
  parameter that is a grounded variable (spec 3 + "adapted" flag) by a pointer to the rule's
  variable object; rule variables carry flag 0x20 ("shared"; LSLib's "IsAType"). Save/restore
  copies only the shared parameters **of that action** around the nested evaluation. A nested
  firing of the same rule overwrites all its variables, so later actions of the outer firing
  see the inner values for every variable the intervening action did not pass. Kept as is
  (test `recursion_clobbers_unsaved_rule_variables`).

## Queries

`CReteQuery::Valid(token, adaptor)`:
1. `CReteAdaptor::pParameterList`: per declared parameter — a mapped variable gives its token
   value if VALID, else an unset value of the declared type; a constant column gives the
   constant; an unmapped, constant-less column gives an unset value. Declared out-parameters
   are flagged and passed to the callee as typed empty arguments.
2. `_ExecuteQuery`:
   - engine query (`CReteDIVQuery`): callback with the handle; on failure the out arguments are
     cleared;
   - internal (`CReteInternalQuery` → `InternalQuery(id)`, id = `CFourKey` word 0);
   - user query (`CReteOsiQuery`): look up `<name>__DEF__` (an Event-type start node), `Add`
     the arguments, succeed iff `g_OsiUserQueriesExecuted` grew; the counter is restored
     afterwards. The QRY rules' actions run **during** the query. (No user query in BG3 has
     out-parameters.)
3. An out-parameter that was already bound must come back equal (`operator==`).
4. `CReteAdaptor::UpdateToken`: per returned column — constant columns must equal the
   constant; a variable whose token value has type 0 (from `undefToken`) takes the result,
   others must be equal.

## Built-ins (`InternalCall` / `InternalQuery`, ids from the function's key word 0)

| Id | Function | Effect |
|----|----------|--------|
| 1 | `SysCompleteGoal(goal)` | `SetCompleted` |
| 2 | `SysActivateGoal(goal)` | `EnableRules` (only if sleeping) |
| 3 | `SysSetGoalSleeping(goal)` | flags = flags & 0xf8 \| 2 |
| 4 | `SysLog(db, arity)` | logs the database |
| 5 | `SysClear(db, arity)` | erases all rows, **no propagation** |
| 100 | `SysStatus(goal, out int)` | 3 completed, 1 sleeping, 2 active |
| 101–103 | `SysIsCompleted/IsActive/IsSleeping` | bit 0 / neither bit / bit 1 without bit 0 |
| 104–107 | `SysWasCompleted/WasActive/WasSleeping/WasDefined` | merge-status bits 6/7/5 (0 in a fresh game) |
| 108 | `SysCount(db, arity, out int)` | row count |
| 109 | `SysStoryVersion(out ×4)` | bytes of the packed story version |
| 110 | `SysFactAtIndex(db, arity, n, out db)` | clear the output database, insert the n-th (1-based, insertion order) row; no propagation |

Goal names are matched case-insensitively (`COsiGoalMan::pGoal`); the name argument must have
type id exactly STRING.

## Goals

`COsiGoal` flags (byte 0x68): bit 0 completed, bit 1 rules disabled/sleeping, bit 2 completion
started, bits 5–7 merge status. Compiled stories: root goals 0x00 (OR), the rest 0x02 (AND).

- `SetCompleted`: if not completed: set bits 1|2, fire EXIT, set bit 0, then
  `CheckEnableRules` on every subgoal in order. The rule that ran `GoalCompleted` keeps firing
  its remaining actions.
- `CheckEnableRules`: return if bit 2 is set; with parents, AND needs all completed, OR any;
  then `EnableRules`. A goal without parents enables directly.
- `EnableRules`: only if bit 1 is set: clear bits 0 and 1, fire INIT.
- `COsiris::InitGame`: fire and free the global actions, then walk the goals in id order and
  fire the INIT section of each goal whose bit 1 is clear **at the moment the walk reaches
  it**. Consequence: a goal woken during this walk (its parent completed in its own INIT) that
  has a higher id runs INIT twice — once from `EnableRules`, once from the walk (test
  `and_combiner_waits_for_all_parents`).

## Entry point: `COsiris::Event(handle, args)`

Maps the handle to its function; if any argument has type 0 or 0x7f the event is ignored
("type of parameter unknown"); a function without a start node is ignored ("no rules defined
using it"); otherwise `CReteStartNode::Add(params)`. `bg3-osiris` identifies events by name and
arity instead of handle and converts arguments to the declared parameter types (same root).

## bg3-osiris specifics

- `Vm::new(&Story)`, `init_game(&mut host)`, `event(&mut host, name, args)`, `db(name,
  arity)`, `goal_state(name)`, `errors()`, `stats()`. The `Host` trait receives calls and
  queries by name; `RecordingHost` records them and fails unknown queries.
- Databases: slot vector in insertion order with hash indexes; equality = the Osiris
  equivalence above. Deviation: REAL keys use the bit pattern (NaN handling differs from
  `std::multimap` with `operator<`, which treats NaN as equivalent to everything); the last-36
  slice of a shorter GUID string is the whole string (UB in the original).
- A rule-nesting limit (default 2000) turns runaway recursion into an error instead of a stack
  overflow; libOsiris has no limit.
- `builder` compiles small rules into the same network shapes for tests.

### Boot of `Mods/GustavDev/Story/story.div.osi` (test `boot_gustav_dev`, mock host)

After `InitGame`: 92 goals active, 843 sleeping; 1 551 facts inserted (107 of 7 973 databases
non-empty, 1 544 rows), 401 rules fired, 1 engine call, 37 engine queries (all failing in the
mock), max nesting 1, ~90 ms, **0 VM errors**. Firing `GameModeStarted("Campaign", 0, 0)`,
`LevelLoaded("TUT_Avernus_C")`, `LevelGameplayStarted("TUT_Avernus_C", 0)` (order and arguments
unverified) then gives 242 active / 689 sleeping / 4 completed goals, 5 863 rows, 1 222 rules
fired, 130 calls (20 distinct), 53 user queries, max nesting 8, still 0 errors.

## Open questions

Need a runtime trace of the real game:

1. **Engine event scheduling.** Are events raised by the engine while Osiris is running (e.g.
   from inside a call) delivered synchronously (re-entering the VM) or queued until the current
   evaluation returns? Which events are queued per frame, and in what order?
2. **Startup sequence.** Which events follow `InitGame` for a new game (`GameModeStarted`,
   `LevelLoaded`, `LevelGameplayStarted`, `LevelGameplayReady`, `RegionStarted`, …), with which
   arguments, in which order, and how they interleave with the engine's own setup. What runs
   on loading a save (`SavegameLoaded`; is `InitGame` skipped?).
3. **Engine query answers** for the queries hit during boot (`IsCharacter`, `GetFlag`, …):
   needed to replay a trace faithfully.
4. **Unresolved engine functions.** What a call/query does when the engine has no handler
   (`COsiris::_GetDIVHandles` failure paths).
5. Whether `COsiFunctionMan::Key` includes the arity (we key by name *and* arity).
6. `CReteRelCondition::Del` resolving to `CReteChildNode::Del` (inferred; a vtable read would
   settle it statically).
7. Saved-game stories: live facts, goal flags and whether rule variables / waiting tokens are
   persisted; the `< 1.15` `DB_InRegion` load quirk.
8. How deep real stories nest at runtime (to size the nesting limit / stack).

## Runtime trace plan

Goal: a JSONL reference trace of Osiris activity in the original game, kept in
`.cache/traces/` (never committed), replayed by a `bg3-osiris` test with a replay host that
answers queries from the trace and compares rule firings, calls and database changes.

1. **Cheapest: Osiris' own log.** libOsiris writes a detailed runtime log (`### Run time log`,
   "fires. Rule variables:", `exec INIT section`, inserts/deletes, query results) to `_fh_Log`
   when `COsiris::m_LogEnabled` is set and the relevant `_ParserFlags` bits are on: bit 19
   (0x80000) rule firings, actions, calls, queries, INIT/EXIT; bit 20 (0x100000) database
   contents on insert/delete; bit 21 failed conditions; bit 24 rejected (duplicate) inserts.
   `COsiris::InitGame` keeps bits 16–24 when it resets the flags. With lldb attached to the game
   (`lldb -n "Baldur's Gate 3"`), set those globals before `InitGame` runs (break on
   `COsiris::InitGame`, `expr` the flags, continue), then start a new game and collect the log.
   Parse it into the JSONL schema below.
2. **Structured: Frida.** A script (to live in `tools/osiris-trace/`) attached to the running
   process hooks unstripped libOsiris symbols:
   - `COsiris::Event(unsigned int, COsiArgumentDesc*)`: handle → function name (walk
     `COsiFunctionMan`), arguments (`COsiArgumentDesc` linked list: next, payload, type);
   - `COsiFunctionParseData::Fire()` and the registered DIV call/query callbacks (pointers
     stored by `COsiris::RegisterDIVFunctions`): calls, queries with arguments and results;
   - `CReteRuleActionPart::Add` (node id = rule), `CReteStartNode::Add/Del(COsipParameterList*)`
     (node id + tuple), `COsiGoal::SetCompleted`/`EnableRules`, `COsiris::InitGame`.
   Each record gets a sequence number and the `g_RuleNesting` depth, which also answers
   question 1 (nesting of `Event` inside callbacks).
3. **Replay test** (ignored, needs install + trace): boot the same story, feed recorded events in
   order, answer queries from the trace, and diff the sequence of rule node ids, calls and
   fact changes against the trace.

The game is not run by agents; the user captures traces and points `BG3_OSIRIS_TRACE` at them.
