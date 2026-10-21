# Osiris VM: how a compiled story is evaluated

Notes for the future `bg3-osiris` crate. Source: static reading of `libOsiris.dylib` (arm64
slice, reference build 4.1.1.7631656) with REA/Ghidra. Prose only; symbol names are cited so
each point can be checked again. Data model: `crates/bg3-story`; file format:
`docs/formats/osiris-story.md`. Nothing here has been checked against runtime traces yet.

## Pieces

- **Tuples / tokens.** A start node works with *physical* tuples (one value per function
  parameter). Everything below the first adaptor works with *logical* tokens
  (`COsiVarIDValuePairList`): values keyed by rule-variable index.
- **Adaptors** (`CReteAdaptor`) sit on edges. `CReteAdaptor::token(CTuple)` turns a physical
  tuple into a logical token: columns with a logical index become variables, constant columns
  are checked against the adaptor's constants (mismatch → the tuple is dropped). The reverse,
  `selectDesc`/`InitTuple`, builds a column filter or tuple from a token for DB lookups and
  inserts.
- **Databases** (`CReteDBase`) are sets of tuples with per-column indexes
  (`CReteDBase::Select`, `find`, `insert`, `erase`). A node's `database` field links it to one:
  start nodes of `DB_*` functions, but also inner join nodes, which materialise their output
  (join memory).
- **Goals** gate which rules can fire.

## Entry points

- **Event from the engine**: `COsiris::Event(handle, args)` maps the function handle to its
  function, then calls `Add(params)` on the function's start node. Events with no node are
  ignored ("no rules defined using it").
- **Start node add**: `CReteStartNode::Add`. If the node has a database, the tuple is inserted
  only if it is not already there, so a duplicate fact does **not** re-trigger rules. New tuples
  go to `ForwardAddToken`, which visits the successor connections in file order. A connection
  whose goal has flag bit 1 (rules disabled) is skipped. Otherwise the successor's adaptor for
  that entry point converts the tuple and, if the constants match, calls `successor.Add(token,
  from, entry)`. Deletion (`Del`, `ForwardDelToken`) mirrors this.
- **Init**: `COsiris::InitGame` first fires the global action list (then frees it), then fires
  the INIT section of every goal whose bit 1 is clear, in goal-id order. In shipped stories those
  are the 92 root goals (flags 0x00).

## Inner nodes

- **And** (`CReteBinaryNode::Join` / `JoinInternal`). A token arrives from one parent; the
  *other* parent decides what happens:
  1. If the other parent has a database: build a select descriptor from the other side's adaptor
     and the token's bound variables (`selectDesc`), `CReteDBase::Select`, adapt each matching
     tuple, merge it with the incoming token, and `CReteChildNode::Add` it downstream (one
     propagation per match, in DB order).
  2. Else, if the other side has a generator (the `DbLink` on that side: its node and
     connection), `CReteNode::GenerateAllSatisfying` replays the stored tuples through that
     sub-network to produce matches. A re-entrancy slot (`this+0x38`) holds the pending token.
  3. Else the other parent is a query: the token is merged with the other adaptor's
     "undefined" token and handed to the query node's `Valid(token)`, which fills out-parameters
     and returns success. On success `Join` passes the merged token on.
- **NotAnd** (`CReteNAnd::Add/Del`). Only left-side tokens pass. If the right side is `Valid`
  for the token, the token stops. Otherwise, if the node has its own database, the tuple is
  inserted (a duplicate stops it), then it propagates. A token arriving on the **right** turns
  into a `Del` downstream (retraction); right-side deletes re-add.
- **Compare** (`CReteRelCondition::Add`, `ConditionValid`). Picks the token values whose
  variable index equals `left_var` / `right_var`, or the stored constants, and calls
  `Compare(a, b, op)`:
  - INTEGER/INTEGER64/REAL mix freely; if either side is REAL the comparison is done in `f32`.
  - STRING `==`/`!=` compare interned string ids; `<` etc. use `strcasecmp`.
  - GUIDSTRING compares **only the last 36 characters** (the UUID) case-insensitively; `==`
    uses the interned id bits. A GUIDSTRING may be compared with a STRING the same way.
  - DIV object types: assertion "encountered DIV object" (not supported).
- **Rule** (`CReteRuleActionPart::Add`). Copies the token into the rule's variable vector by
  index, then fires each action in order (`COsiFunctionParseData::Fire`). Firing is synchronous
  and depth-first: an action that inserts a fact or calls a PROC runs all affected rules
  before the next action (`g_RuleNesting` tracks the depth). If `is_query` is set the rule
  increments `g_OsiUserQueriesExecuted`.

## Actions (`COsiFunctionParseData::Fire`)

- `goal_or_hook > 0` → `COsiGoal::SetCompleted` on that goal (the `GoalCompleted;` statement).
  `< 0` → `ExecuteDebugHook`.
- Otherwise look the function up by name and dispatch on its kind:
  - 3 call → engine callback with the function's handle and the bound arguments.
  - 4 database → `Add` (insert) on the DB start node, or `Del` if `negate`. Shared parameters
    are saved and restored around the call (`SaveSharedParameters`), because the nested
    evaluation reuses the same value objects.
  - 5 proc → `Add` on the proc's start node (same mechanism as an event).
  - 7 syscall → `InternalCall` (e.g. `SysCompleteGoal`, `SysSetGoalSleeping`).
  - 1 event → only in a special parser mode. 2/6/8 (queries) cannot be actions.

## Queries

- **Engine queries** (`CReteDIVQuery::_ExecuteQuery`) call into the game.
- **Internal queries** (`CReteInternalQuery`) are Osiris built-ins (`SysWasDefined`, ...).
- **User queries** (`CReteOsiQuery::_ExecuteQuery`): calling `QRY_Foo(args)` looks up the
  user-query function `QRY_Foo__DEF__` and `Add`s the arguments to its start node, which runs
  every `QRY` rule for it. The query **succeeds iff at least one QRY rule fired** (compares
  `g_OsiUserQueriesExecuted` before and after). Out-parameters come back through the shared
  parameter objects.

## Goals

`COsiGoal` flags (byte 0x68): bit 0 completed, bit 1 rules disabled/sleeping, bit 2 completion
started.

- `COsiGoal::SetCompleted`: if not completed, sets bits 1|2 (rules off), fires the EXIT
  actions, sets bit 0, then calls `CheckEnableRules` on every **subgoal**.
- `COsiGoal::CheckEnableRules`: returns if bit 2 is set. With the AND combiner, every parent
  must be completed; with OR, any one. Then `EnableRules`.
- `COsiGoal::EnableRules`: only if bit 1 is set: clears bits 0 and 1 and fires the INIT
  actions.

So the goal tree is a dependency graph: a child's rules wake up and its INIT runs when its
parents complete.

## Design notes for `bg3-osiris`

- Keep `bg3-story` ids as arena indices: `NodeId`/`AdapterId`/`DatabaseId`/`GoalId` index
  dense tables; side tables can use `Table::id_bound()`.
- Evaluation is recursive and order-sensitive (successor order, DB iteration order, action
  order). For 1:1 we need Osiris' DB ordering. `CReteDBase` keeps a tuple list plus per-column
  `std::map` indexes; `Select` order still has to be pinned down, ideally with a runtime trace.
- Value identity: strings are interned (`COsiStringTable`), and GUIDSTRING equality ignores the
  name prefix.

## Open questions

- Exact `Select` iteration order and how `GenerateAllSatisfying` walks a `DbLink` with
  `indirection > 1`.
- What flag bit 5 (`IsAType`) means. It is set on every rule variable in BG3 stories.
- How the engine fills function `key`s (handles) at load: `COsiris::_GetDIVHandles` and
  `RegisterDIVFunctions`.
- Save-game stories: same format with live facts and goal flags. Need a test against real
  `.lsv` saves.
