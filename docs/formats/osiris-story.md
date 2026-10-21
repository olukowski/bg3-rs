# Osiris compiled story (`story.div.osi`)

Implemented in `crates/bg3-story`. The layout below was established by reading libOsiris
(`COsiris::Load` and the `COsiSmartBuf`-taking constructors it calls) in the reference build
4.1.1.7631656. Verified by loading all six shipped stories with zero leftover bytes **and**
re-serialising them byte-for-byte (`load_shipped_stories`, ignored test).

Prior art: Norbyte's [LSLib](https://github.com/Norbyte/lslib) (MIT) has a C# story
reader/decompiler. We used it to orient ourselves; field order and version gates were checked
against libOsiris, which differs from LSLib in a few places (noted below).

## Files in the reference build

| Pak | Path | Format | Story version |
|-----|------|--------|---------------|
| Gustav | `Mods/GustavDev/Story/story.div.osi` | 1.15 | 4.8.700.7143220 |
| Gustav | `Mods/Honour/Story/story.div.osi` | 1.15 | 1.0.0.7143220 |
| Gustav | `Mods/Gustav/Story/story.div.osi` | **1.13** | 1.0.235.2614518 |
| Shared | `Mods/SharedDev/Story/story.div.osi` | 1.15 | 1.0.0.7143220 |
| GustavX | `Mods/GustavX/Story/story.div.osi` | 1.15 | 4.8.400.7143220 |
| GustavX | `Mods/HonourX/Story/story.div.osi` | 1.15 | 1.0.0.7143220 |

Counts (GustavDev): 44 types, 14 enums, 0 objects, 20 065 functions, 151 332 nodes
(50 217 rules), 210 822 adaptors, 15 706 databases (0 initial facts), 935 goals, 0 global
actions. Shipped stories carry **no** facts: databases are filled by goal INIT sections at
runtime. Saves (`.lsv` under `Savegames/Story/`) embed the same format with live facts and goal
state; not yet examined.

## Primitives

- All integers little-endian. Byte 2 of the header says big-endian; we reject such files.
- **Strings** are NUL-terminated, and after the header every byte (including the terminator)
  is XORed with `0xAD` (from format 1.4). The header banner is plain.
- `list<T>` = `u32 count` + items; `list8<T>` = `u8 count` + items.
- **Ids** (nodes, adaptors, databases, goals) are 1-based `u32`; `0` means "none".
- The very first byte of the file belongs to the buffer (`COsiSmartBuf` constructor reads a
  mode byte; always 0). LSLib treats it as part of the header.

## Header

| Field | Type | Notes |
|-------|------|-------|
| buffer flag | u8 | 0 |
| banner | string (plain) | e.g. `Osiris save file dd. 09/21/26 13:32:45. Version 1.8.` |
| major, minor | u8, u8 | 1.15 / 1.13 seen; loader supports 1.13–1.15 |
| big endian | u8 | 0 |
| unused | u8 | |
| story version | `[u8;128]` | NUL-padded build string (format > 1.1) |
| debug flags | u32 | parser flags at compile time (> 1.2), `0x004810a0` seen |
| types | list of `{string name, u8 index, u8 alias}` | (> 1.4; alias byte > 1.8). Includes the built-ins 1..5 with alias 0 |
| enums | list of `{u16 type, list<{string label, u64 value}>}` | (> 1.12) |

Built-in type ids: 0 none, 1 INTEGER, 2 INTEGER64, 3 REAL, 4 STRING, 5 GUIDSTRING. Other types
alias a chain ending in a built-in (e.g. `CHARACTER → GUIDSTRING`, `ITEMROOT → ROOT →
GUIDSTRING`); a type with no alias and index ≥ 6 is a "DIV object" type; `0x7f` is reserved.
Enum types (e.g. `UNSHEATHSTATE`) alias INTEGER64; each enum also has an
`__OSIRIS_ENUM_FALLBACK` element.

## Sections (in order)

1. **DIV objects**: `list<{string name, u8 type, u32 key[4]}>` (empty in BG3).
2. **Functions**: `list<Function>`
   `u32 line, u32 condition_refs, u32 action_refs, u32 node, u8 kind, u32 key[4],`
   `string name, u32 mask_len, u8 out_mask[mask_len], list8<u16 type>`.
   Kind: 1 event, 2 query, 3 call, 4 database, 5 proc, 6 sysquery, 7 syscall, 8 user query.
   Parameter *i* is an out-parameter if `out_mask[i>>3] & (0x80 >> (i&7))`. `key` is the
   engine binding (`CFourKey`) used to resolve native handlers. User queries `QRY_Foo` come with
   a `QRY_Foo__DEF__` companion (see the VM notes).
3. **Rete nodes**: `list<{u8 type, u32 id, Node}>`. Every node starts with
   `u32 database, string name, [u8 arity if name non-empty]`, then by type:

   | Type | Class | Extra fields |
   |------|-------|--------------|
   | 1 | `CReteFact` (database) | StartNode |
   | 2 | `CReteEvent` (event/proc) | StartNode |
   | 3 | `CReteDIVQuery` | — |
   | 4 | `CReteAnd` | Join |
   | 5 | `CReteNAnd` | Join |
   | 6 | `CReteRelCondition` | Unary, `i8 left_var, i8 right_var, Value left, Value right, u32 op` |
   | 7 | `CReteRuleActionPart` | Unary, `list<Action>`, `list8<Value> variables`, `u32 line`, `u8 is_query` (> 1.5) |
   | 8 | `CReteInternalQuery` | — |
   | 9 | `CReteOsiQuery` | — |

   - *Connection* = `u32 node, u32 entry (0 none, 1 left, 2 right), u32 goal`.
   - *StartNode* = `list<Connection>` successors. (LSLib also expects this list on query nodes;
     in libOsiris query nodes are plain `CReteNode`s and have nothing after the base.)
   - *DbLink* = `u32 db_node, Connection, u8 indirection`.
   - *Join* = `Connection next, u32 left_parent, u32 right_parent, u32 left_adaptor,
     u32 right_adaptor, DbLink left, DbLink right`.
   - *Unary* = `Connection next, u32 parent, u32 adaptor, DbLink`.
   - Compare op (`TOsiRelOp`, checked against libOsiris `Compare`): 0 `<`, 1 `<=`, 2 `>`,
     3 `>=`, 4 `==`, 5 `!=`.
4. **Adaptors**: `list<{u32 id, list8<Value> constants, list8<i8> logical_indices,
   list8<{u8 logical, u8 physical}> map}>`. A constant's value index is its physical column.
   `logical_indices[col]` is the rule variable bound to physical column `col` (or −1).
5. **Databases**: `list<{u32 id, list8<u16 type>, list<list8<Value>> facts}>`.
6. **Goals**: `list<{u32 id, string name, u8 combiner, list<u32> parents, list<u32> subgoals,
   u8 flags, list<Action> init, list<Action> exit}>`. Combiner 0 = OR, 1 = AND (of parents).
   Flags: bit 0 completed, bit 1 rules disabled ("sleeping"), bit 2 completing/completed.
   Compiled stories: root goals 0x00/OR, all others 0x02/AND.
7. **Global actions**: `list<Action>` (empty in BG3).

No bytes follow.

### Action (`COsiFunctionParseData`)

`string name`; if the name is non-empty: `u8 has_params`, then (if non-zero)
`list8<Value> params`, then `u8 negate`. Always followed by `i32 goal_or_hook`: 0 = normal call,
> 0 = with an empty name, "GoalCompleted" for that goal id, < 0 = debug hook.

### Value (`COsiTypedValueBase`)

Format ≥ 1.14:

```
i8 index           // variable index / adaptor column / -1
u8 flags           // bits 0-2 specialisation (1 value, 2 typed param, 3 variable),
                   // 3 VALID, 4 out-param, 5 "IsAType", 6 unused (`_`), 7 adapted
if flags & VALID:
  u8 marker; u16 type
  '0': payload by root type of `type`: none | i32 | i64 | f32 |
       STRING/GUIDSTRING: u8 present, [string] | object: string
  'e': string enum label (resolved through the enum's symbol table)
  '1': i32 raw
```

Format ≤ 1.13 has no index/flags word. The constant (marker, type, payload) always comes first;
the specialisation comes from the caller, and the flags follow as whole bytes: for
specialisation 2/3 three bytes (valid, out, IsAType), for 3 additionally `i8 index`, `u8 unused`,
`u8 adapted`. Callers add a byte before some values: action parameters carry a kind byte
(0 → typed, 1 → variable, other → value), rule variables a byte (always 1), adaptor constants
their column.

GUID strings: Osiris strips spaces when loading (`COsiSmartBuf::readGuidString`); the crate keeps
the stored string.

## Quirks

- In formats < 1.15, `DB_InRegion` and `DB_WasInRegion` get special handling at load time
  (facts containing `Crowd_Citizen_A_Patterned` are dropped from `DB_InRegion`). Applies to the
  1.13 `Gustav` story; behavioural, not a format difference.
- Ids are assigned by factories that pad gaps with null; the crate's `Table` supports gaps but
  none occur in shipped files.
- The rule `line` is the source line of the THEN part.
