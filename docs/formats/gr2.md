# Granny 3D `.GR2` (BG3 flavour)

Implemented in `crates/bg3-gr2`. Every one of the **285,055** `.GR2` files in the reference
install (4.1.1.7631656) parses: header, all sections, relocations and the typed scene
(`cargo test --release -p bg3-gr2 -- --ignored`, ~30 s).

Granny 3D is RAD Game Tools' model/animation middleware. A `.GR2` file is a serialised object
graph that carries its own type description, so a reader can walk it generically and then pick
out the members it understands by name.

## Survey of the reference install

| Pak | GR2 files | Notes |
|-----|-----------|-------|
| Models | 131,204 | meshes, skeletons, character/creature animation |
| English_Animations | 130,783 | dialogue animation (`MC_…` facial/body tracks) |
| Gustav | 19,038 | level/HLOD meshes |
| GustavX | 3,972 | |
| Effects / Engine / Shared / Game | 46 / 9 / 2 / 1 | |

By content: 232,477 animation files, 50,564 files with meshes (143,649 meshes), 2,014
skeleton-only files.

* **Signature**: all files are little-endian 64-bit. 285,016 use the well-known LE64 magic
  `E5 9B 49 5E 6F 63 1F 14 1E 13 EB A9 90 BE ED C4`; 39 files in `Models.pak` (mostly
  `Proxy_*_FullBody` character proxies and a few large props) use a second signature
  `E5 2F 4A E1 6F C2 8A EE 1E D2 B4 4C 90 D7 55 AF` with an identical layout.
* **File format revision**: always 7. Header-format word 0, type tag `0x80000039`, extra tags 0.
* **Sections**: 5–31 per file (5 is the common case); alignment 4, except section 1 is
  occasionally 32.
* **Compression**: every non-empty section is **BitKnit 2** (type 4). No `None`, `Oodle0`,
  `Oodle1` or `BitKnit1` sections exist. Relocation tables of BitKnit sections are themselves
  BitKnit-compressed.
* **Mixed-marshalling** records are present (~370k sections) but only matter for byte-swapping
  big-endian files; they are ignored.
* The Oodle "stop" fields (`first_16bit`/`first_8bit`) are set but irrelevant to BitKnit.

## Layout (little-endian)

### Magic block (0x20 bytes)

| Off | Type | Field |
|-----|------|-------|
| 0x00 | `[u8;16]` | signature (pointer size + endianness) |
| 0x10 | u32 | header size (magic + info + section table) |
| 0x14 | u32 | header format (0) |
| 0x18 | `[u32;2]` | reserved |

### File info (0x48 bytes, at 0x20)

| Off | Type | Field |
|-----|------|-------|
| 0x00 | u32 | format revision (7) |
| 0x04 | u32 | total file size |
| 0x08 | u32 | CRC32 |
| 0x0C | u32 | section table offset, relative to 0x20 (= 0x48) |
| 0x10 | u32 | section count |
| 0x14 | (u32,u32) | root **type** (section, offset) |
| 0x1C | (u32,u32) | root **object** (section, offset) |
| 0x24 | u32 | type tag |
| 0x28 | `[u32;4]` | extra tags |
| 0x38 | u32 | string table CRC |
| 0x3C | `[u32;3]` | reserved |

### Section header (44 bytes each)

`compression, data_offset, data_size (on disk), expanded_size, alignment, first_16bit,
first_8bit, relocations_offset, relocations_count, marshalling_offset, marshalling_count`
(all u32; offsets are absolute file offsets).

### Relocations

`relocations_count` records of 12 bytes: `u32 offset_in_this_section, u32 target_section,
u32 target_offset`. Each names an 8-byte pointer slot to patch. For a BitKnit section the table
on disk is `u32 compressed_size` followed by a BitKnit stream that expands to
`count × 12` bytes. Unpatched pointer slots are zero (null).

The reader does not build a real address space: each slot is rewritten to an opaque
`(section + 1) << 32 | offset` value (`bg3_gr2::Ptr`), with 0 staying null.

## BitKnit 2

An LZ77 + rANS codec from RAD. The stream is a sequence of LE 16-bit words.

* Starts with the magic word `0x75B1`.
* Output is produced in **quanta** that end on 64 KiB boundaries of the output (the last one
  ends at the output size). A copy may run past a boundary; the next quantum still ends at the
  next boundary.
* A quantum whose first word is 0 is **stored**: raw bytes follow, up to the quantum end.
* Otherwise the quantum is coded by **two interleaved 32-bit rANS states** (L = 2^16, 16-bit
  word renormalisation, 15-bit probabilities). Every symbol decode or raw-bits extraction uses
  the current state, refills it from the word stream if it fell below 2^16, then the two states
  swap. At the end of a coded quantum both states must equal 2^16 (a strong integrity check).
* The quantum starts with an **init block** packing both encoder end states: two words form a
  32-bit value whose low 4 bits are a split count *k*; the rest (refilled to ≥ 2^16 if needed)
  is shifted right by *k* to give state 1 (again refilled if needed); its low *k* bits plus one
  more word become the low `16 + k` bits of state 2, whose bit `16 + k` is implicitly set.
* The very first output byte is 8 raw bits. After that each step decodes a **command word**
  (alphabet of 300) with one of four models selected by `output_position % 4`:
  * `0..256`: a literal, stored as a **delta** against the byte at `pos − last_copy_offset`
    (initially 1, i.e. the previous byte).
  * `256..288`: a copy of length `v − 254` (2..33).
  * `288..300`: a copy of length `2^b + bits(b) + 32` with `b = v − 287`.
* A copy then decodes a **cache reference** (alphabet of 40, also four models by position):
  * `0..8`: reuse entry *i* of an 8-entry most-recently-used offset cache and move it to the
    front. The cache is stored as fixed slots plus a nibble-packed permutation word
    (initially `0x76543210`, all slots = 1), so a hit is a rotate of the low `4(i+1)` bits.
  * `8..40`: a new offset. Decode a bit count *b* (alphabet of 21, single model), read
    `b mod 16` raw bits (plus one whole raw stream word if *b* ≥ 16) as *L*; the offset is
    `2^(5+b) + 32·L + v − 39`. It is inserted into the cache in the second-to-last position
    (the old occupant moves to last).
* The copy sets `last_copy_offset` and copies byte-by-byte (overlap allowed).
* **Adaptive models**: start with a uniform CDF over the first `n − m` symbols and minimum
  frequency for the last *m* (the command model has 36 min-probability symbols — the long copy
  lengths; the others have none). Each decoded symbol adds `(2^15 − n) / 1024` to its counter;
  every 1024 symbols the 1024th also gets the remainder, the CDF moves halfway to the counted
  CDF (difference taken in unsigned 32-bit, then halved and truncated to 16 bits), and counters
  reset to 1. A 1024-entry table over the top 10 code bits speeds up symbol lookup.

The game's decoder is `granny::BitKnitDecompress` / `granny::DecompressIncremental`
(statically linked; symbols in `.cache/symbols/bg3.tsv`). Granny feeds it in 0x2000-byte
chunks; we decode the whole buffer at once.

## Type system ("member definitions")

A type is an array of 44-byte member records terminated by a record of type 0:

| Off | Type | Field |
|-----|------|-------|
| 0 | u32 | member type |
| 4 | ptr | name (C string) |
| 12 | ptr | referenced type |
| 20 | i32 | array size (0 = scalar) |
| 24 | `[u32;3]` | extra |
| 36 | u64 | reserved |

Objects are **packed** (no padding) in member order. Member types and their 64-bit sizes:

| # | Type | Size |
|---|------|------|
| 1 | Inline (embedded struct) | size of referenced type |
| 2 | Reference | 8 |
| 3 | ReferenceToArray (`i32 count, ptr`) | 12 |
| 4 | ArrayOfReferences (`i32 count, ptr → ptr[]`) | 12 |
| 5 | VariantReference (`ptr type, ptr object`) | 16 |
| 7 | ReferenceToVariantArray (`ptr type, i32 count, ptr`) | 20 |
| 8 | String (`ptr → char[]`) | 8 |
| 9 | Transform (`u32 flags, f32 pos[3], f32 quat[4], f32 scale_shear[9]`) | 68 |
| 10 | Real32 | 4 |
| 11/12 | Int8 / UInt8 | 1 |
| 13/14 | BinormalInt8 (snorm) / NormalUInt8 (unorm) | 1 |
| 15/16 | Int16 / UInt16 | 2 |
| 17/18 | BinormalInt16 (snorm) / NormalUInt16 (unorm) | 2 |
| 19/20 | Int32 / UInt32 | 4 |
| 21 | Real16 (IEEE half) | 2 |
| 22 | EmptyReference | 8 |

Sizes are multiplied by the array size when it is non-zero.

## Root object (`granny_file_info`)

`ArtToolInfo, ExporterInfo, FromFileName, Textures, Materials, Skeletons, VertexDatas,
TriTopologies, Meshes, Models, TrackGroups, Animations, ExtendedData`. In BG3 the art tool is
"FBX" 7.5, `UnitsPerMeter` 1, Y up, `BackVector` (0,0,−1). Materials/Textures are empty (BG3
binds materials elsewhere); `MaterialBindings[].Material` is null.

* **Skeleton**: `Name, Bones[], LODType`. **Bone**: `Name, ParentIndex (−1 = root), Transform`
  (local rest pose), `InverseWorldTransform` (16 × f32, row-major with translation in the last
  row), `LODError, ExtendedData`.
* **Model**: `Name, Skeleton (ref), InitialPlacement, MeshBindings[{Mesh}]`.
* **Mesh**: `Name, PrimaryVertexData {Vertices (variant array), …}, MorphTargets,
  PrimaryTopology {Groups[{MaterialIndex, TriFirst, TriCount}], Indices (i32) | Indices16
  (u16), …}, MaterialBindings, BoneBindings[{BoneName, OBBMin, OBBMax, TriangleIndices}],
  ExtendedData`.
* **Mesh ExtendedData** (Larian): `UserMeshProperties` (ref to `Flags[4], Lod, FormatDescs[],
  ExtendedData, LodDistance, IsImpostor`), `LSMVersion` (3), and optional `Rigid`, `Cloth`,
  `Spring`, `Occluder`, `MeshProxy`, `LOD` ints.

Almost every mesh uses 16-bit indices (143,450 vs 157 32-bit) and a single topology group;
32 meshes have morph targets.

### Vertex formats seen

| Meshes | Components |
|-------:|------------|
| 64,362 | Position f32×3, QTangent snorm16×4, UV0 f16×2 |
| 42,173 | Position, BoneWeights unorm8×4, BoneIndices u8×4, QTangent, DiffuseColor0 unorm8×4, UV0 f16×2 |
| 20,230 | Position, QTangent, DiffuseColor0, UV0 |
| 7,679 | Position, BoneWeights, BoneIndices, QTangent, UV0 |
| 3,373 | skinned + DiffuseColor0 + UV0 + UV1 |
| 2,185 | Position only (proxies / collision) |
| ~3,600 | other combinations: up to 4 UV sets, UVs as f32×2 in a few hundred meshes |

No plain `Normal`/`Tangent`/`Binormal` components occur in Models, Gustav, GustavX or Shared (the reader
still supports them). Bone indices index the mesh's `BoneBindings`, not the skeleton; weights
are u8 summing to 255.

### QTangent

The tangent frame is a quaternion `(x, y, z, w)` stored as 4 × snorm16. With `X, Y, Z` the
rotated basis axes and `s = sign(w)`:

* normal = `s · Z`
* tangent = `X` (along +U)
* bitangent (along +V of the UVs as stored) = `−Y`
* so a glTF/Bevy `tangent.w` (bitangent = `cross(N, T) · w`) is `−s`.

Determined empirically with `examples/gr2_scan.rs` over ~80 M triangles of `Models.pak`: `s·Z`
matches the triangle winding normal on 97.8% (unsigned `Z` only 28%), `X` matches dP/du on 99%,
and `−Y` matches dP/dv on 98.7%. The convention agrees with Norbyte's LSLib reader. Caveat: the
auto-generated HLOD meshes in `Gustav.pak` (`Generated/Public/GustavDev/Assets/HLOD`) come out
with the opposite bitangent sign on ~90% of meshes — presumably a different generator
convention (or they are not tangent-space normal mapped); not yet confirmed against the shaders.

## Animations (not decoded yet)

`TrackGroups[{Name, VectorTracks, TransformTracks[{Name, Flags, OrientationCurve,
PositionCurve, ScaleShearCurve}], …}]` with `CurveData` as variant references whose type names
give the Granny curve format (`CurveDataHeader_D4nK16uC15u`, `D3Constant32f`, `DaIdentity`,
`DaK16uC16u`, …). Dialogue files carry a `VectorTrack` named `Textkeys`. The generic walker
already exposes all of this.

## Tools

* `examples/gr2_survey.rs`: header/codec census across all paks.
* `examples/gr2_scan.rs`: vertex format census and QTangent convention check.
* `examples/gr2_dump.rs`: dump the generic object tree of a file.

## Credits

* Norbyte's **LSLib** (MIT): GR2 header/section/member-type layout and the vertex component
  names/QTangent convention.
* Mackenzie Straight's **libbg3 / pybg3** (MIT), especially `docs/bitknit2.txt`: the BitKnit 2
  bitstream description that the decoder here was written from.
* Fabian Giesen's public blog posts on BitKnit, rANS and interleaved entropy coders.

The code here is a fresh Rust implementation. No Granny or game code was copied.
