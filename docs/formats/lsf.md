# LSF / LSX / LSJ resource files

Implemented in `crates/bg3-lsf`. All three formats are serialisations of the same thing: a
**resource** made of named **regions**, each holding a tree of **nodes**; a node has a name, an
ordered list of typed **attributes**, optional **map key**, and ordered children.

Verified against every `.lsf`, `.lsfx`, `.lsx` and `.lsj` in `Gustav`, `GustavX`, `Shared`,
`Engine` and `Game` paks of the reference build (4.1.1.7631656):
`cargo test -p bg3-lsf --release -- --ignored --nocapture`
(`parse_every_resource_in_install`, `dialog_lsf_ids_match_lsj`).

| Extension | Count | Format |
|-----------|------:|--------|
| `.lsf`  | 69 267 | binary, magic `LSOF` |
| `.lsfx` | 484 | **also plain LSF** (effect banks; v6/v7, engine version 1.0.x) |
| `.lsx`  | 23 049 | XML |
| `.lsj`  | 10 015 | JSON (dialog sources, voice barks) |

All parse with zero failures, and every file survives `to_lsx → parse_lsx` and
`to_lsf → parse_lsf` unchanged (~5 GiB of decompressed resources, ~20 s on 8+ cores, release).

## LSF

All integers little-endian.

### Header

| Offset | Type | Field |
|--------|------|-------|
| 0x00 | `[u8;4]` | magic `LSOF` |
| 0x04 | u32 | container version (1–7) |
| 0x08 | u32 (v < 5) / u64 (v ≥ 5) | engine version, packed (below) |
| … | `(u32 uncompressed, u32 on_disk)` × N | section sizes, in the order: strings, *keys (v ≥ 6 only)*, nodes, attributes, values |
| … | u8 | compression flags: low nibble method (0 none, 1 zlib, 2 LZ4, 3 zstd), high nibble level hint |
| … | u8, u16 | unknown, always zero in the reference build |
| … | u32 | metadata format (see below) |

So the header is 0x30 bytes for v < 5, 0x38 for v5, 0x40 for v6/v7.

Engine version packing:

- v < 5, 32-bit: `major = v>>28`, `minor = (v>>24)&0xF`, `revision = (v>>16)&0xFF`, `build = v&0xFFFF`.
- v ≥ 5, 64-bit: `major = v>>55`, `minor = (v>>47)&0xFF`, `revision = (v>>31)&0xFFFF`,
  `build = v&0x7FFF_FFFF`.

### Sections

Immediately after the header, in this on-disk order: **strings, nodes, attributes, values,
keys** (note: keys come last on disk although their sizes come second in the header).

Per section: if `on_disk == 0` the section is stored raw and is `uncompressed` bytes long (both
zero = empty/absent). Otherwise `on_disk` bytes are decompressed with the header's method.
For LZ4, the string table is a raw **LZ4 block**, while nodes/attributes/values/keys are
**LZ4 frames** (v ≥ 2, "chunked"). The reader also sniffs the LZ4 frame magic `04 22 4D 18`.
Zlib and zstd are supported but never occur in the reference build.

### String table

`u32 bucket_count`, then per bucket `u16 count` and `count × (u16 len, len bytes)` (no NUL).
Names are referenced by a u32 whose high 16 bits are the bucket and low 16 bits the index in the
bucket. Larian files use 0x200 buckets; the hash function is irrelevant for reading.

### Nodes and attributes: two record layouts

The **metadata format** word selects the record layout:

- `1` ("keys and adjacency"): 16-byte records.
  - node: `name u32, parent i32, next_sibling i32, first_attribute i32`
  - attribute: `name u32, type_and_length u32, next_attribute i32, value_offset u32`
- `0` and `2`: 12-byte records.
  - node: `name u32, first_attribute i32, parent i32`
  - attribute: `name u32, type_and_length u32, owner_node i32`; values are packed back-to-back in
    attribute-table order, so offsets are a running sum of lengths, and a node's attribute chain
    is "the attributes whose owner is this node, in table order".

`type_and_length`: low 6 bits are the type id, the upper 26 bits the value length in bytes.

Nodes with `parent == -1` are region roots; the region name is the root node's name. Children are
attached in node-table order; like LSLib, the reader does not follow the 16-byte format's
next-sibling links (not cross-checked against table order yet).

### Keys (v ≥ 6)

Pairs of `u32 node_index, u32 name_ref`: the node's map key is the attribute with that name
(LSX `<node id="…" key="MapKey">`). Seen only with metadata format 1.

### Attribute values

| Id | LSX name | Encoding |
|---:|----------|----------|
| 0 | `None` | empty |
| 1 / 27 | `uint8` / `int8` | 1 byte |
| 2 / 3 | `int16` / `uint16` | 2 bytes |
| 4 / 5 | `int32` / `uint32` | 4 bytes |
| 6 / 7 | `float` / `double` | IEEE 754 |
| 8–10 | `ivec2..4` | i32 × n |
| 11–13 | `fvec2..4` | f32 × n |
| 14 / 15 / 18 | `mat2x2` / `mat3x3` / `mat4x4` | f32 × 4 / 9 / 16, row-major (as LSX rows) |
| 16 / 17 | `mat3x4` / `mat4x3` | f32 × 12 (4 rows of 3 / 3 rows of 4; shape assumed, see below) |
| 19 | `bool` | 1 byte |
| 20, 21, 22, 23, 29, 30 | `string`, `path`, `FixedString`, `LSString`, `WString`, `LSWString` | UTF-8, NUL-terminated; length includes the NUL |
| 24 | `uint64` | 8 bytes |
| 25 | `ScratchBuffer` | raw bytes (base64 in LSX/LSJ) |
| 26 / 32 | `old_int64` / `int64` | 8 bytes |
| 28 | `TranslatedString` | v ≥ 4: `u16 version`, `i32 len`, handle (NUL-terminated). v < 4: `i32 len`, inline text, `i32 len`, handle |
| 31 | `guid` | 16 bytes, see below |
| 33 | `TranslatedFSString` | a TranslatedString, then `i32 argc`, then per argument: `i32 len` key, nested TranslatedFSString, `i32 len` value |

**GUID byte order.** The 16 stored bytes are a little-endian u32, two little-endian u16s, and then
**four more little-endian u16s** — i.e. the text form `aabbccdd-eeff-gghh-iijj-kkllmmnnoopp` is
stored as `dd cc bb aa ff ee hh gg jj ii ll kk nn mm pp oo`. (A .NET `Guid` would keep the last
8 bytes in order; LSLib calls the extra swap "bswap_guids".) Verified by
`dialog_lsf_ids_match_lsj`: all 404 037 GUIDs/handles in 9 371 compiled dialogs
(`Story/DialogsBinary/*.lsf`) are found in the corresponding editor `.lsj`.

Every value in the reference build consumes exactly its declared length (the reader enforces this).

### Versions and variants in the reference build

`(container version, compression flags, metadata format) → files` over .lsf + .lsfx:

| Version | Flags | Format | Files | Notes |
|--------:|------:|-------:|------:|-------|
| 2 | 0x22 | 0 | 7 | old `Public/Game/Content`, engine 2.0.x |
| 3 | 0x00 / 0x22 | 1 | 293 / 216 | e.g. `Public/Game`; engine 3.x / 4.0.x |
| 4 | 0x00 / 0x22 | 1 | 353 / 39 | |
| 5 | 0x00 / 0x22 | 1 | 999 / 29 | |
| 6 | 0x00 | 0 | 20 | levels; one `.lsfx` |
| 6 | 0x00 / 0x22 | 1 | 36 746 / 277 | bulk of Gustav (sound banks, story, …) |
| 7 | 0x00 | 0 | 498 | 483 are `.lsfx` (engine 1.0.x) |
| 7 | 0x00 / 0x22 | 1 | 24 112 / 27 | dialogs, assets |
| 7 | 0x00 | 2 | 6 135 | e.g. under `Public/Engine/Content`; engine version 0.0.x; 12-byte records |

Only LZ4 (0x22 = LZ4, level hint 2) or no section compression occur; most LSF files are stored
uncompressed inside an LZ4-compressed pak entry. Metadata format 1 appears with v3–v5 too, where
there is no keys section. 2.37 M of 45.7 M nodes carry a map key.

## LSX

```xml
<?xml version="1.0" encoding="UTF-8"?>
<save>
    <header version="1" time="0"/>                       <!-- rare, optional -->
    <version major="4" minor="0" revision="9" build="331"/>
    <region id="Config">
        <node id="root" key="MapKey">                    <!-- key optional -->
            <attribute id="Name" type="FixedString" value="…"/>
            <attribute id="Pos" type="fvec3"><float3 x="1" y="0" z="2"/></attribute>
            <attribute id="Rot" type="mat3x3"><mat3><float3 …/>×3</mat3></attribute>
            <attribute id="Label" type="TranslatedString" handle="h…" version="1"/>
            <children> <node …/> … </children>
        </node>
    </region>
</save>
```

Quirks seen in the reference build:

- **Vectors/matrices are child elements** (`<float2/3/4 x y z w>`, `<mat3>`/`<mat4>` wrapping rows),
  not a `value` string. LSLib-style space-separated `value="x y z"` is also accepted.
  `mat3x4`/`mat4x3` never occur in LSX; the writer uses `<mat3x4>` with 4 `float3` rows and
  `<mat4x3>` with 3 `float4` rows (LSLib's column/row convention), unverified.
- A few old files use **numeric type ids** (`type="22"`) and put `value` before `type`.
- **Literal newlines inside attribute values** (e.g. dialog `Script` conditions). A conforming XML
  parser would normalise them to spaces; we unescape entities without normalising. The writer
  emits `&#10;` instead.
- Bools are `true`/`false`; floats use C-style formatting including `1.1996228e+17`, `-0`, and
  (in LSF-derived data) NaN.
- A region's id and its root node's id differ (`region id="Config"` / `node id="root"`), unlike
  LSF where they are one name. `to_lsf` writes the *region* name as the root node name.
- Multiple regions per file occur.

## LSJ

```json
{"save": {"header": {"version": "4.6.0.900", "time": 0},
          "regions": {"dialog": { "Flag": {"type": "bool", "value": true},
                                  "Child": [ { … }, { … } ] }}}}
```

A node is an object whose members are either attributes (objects with `type`) or child lists
(arrays of node objects, all named after the member). The region's root node takes the region
name. `TranslatedString` attributes carry `handle` and `version` (no `value`);
`TranslatedFSString` adds `arguments: [{key, value, string}]`. A handful of voice-bark files use
numeric type ids (`"type": 22`). Node keys are not represented, and children with different names
lose their relative order (they're grouped per name), so LSJ→LSX is not order-exact.

Compiled LSF and source LSJ of the same dialog are **not** identical trees — the compiled form is
restructured (e.g. extra `speakerlist` node, different child grouping) — so only ids/handles are
compared between them.

## Credits

The LSF layout (header fields, section order, string-table addressing, node/attribute record
variants, type ids and their names, TranslatedString/TranslatedFSString encodings, GUID swap)
was first documented by Norbyte's [LSLib](https://github.com/Norbyte/lslib) (`LSFReader`,
`LSFCommon`, `NodeAttribute`), which this write-up and the implementation follow in substance.
Everything here was re-derived or checked against the reference install's own files; the code is
an independent Rust implementation.
