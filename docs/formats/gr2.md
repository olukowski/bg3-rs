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

## Animations

Implemented in `crates/bg3-gr2/src/anim.rs` (`Animation::from_bytes`, `read_animations`). All
**228,541** animations in `Models.pak` (97,758) and `English_Animations.pak` (130,783) decode,
25.9 M transform tracks, zero failures (`cargo test --release -p bg3-gr2 --test animations --
--ignored`, ~13 s).

### Layout

* Root `Animations[]` → **animation**: `Name` (the source `.ma` path), `Duration` (seconds),
  `TimeStep` (seconds per source frame, 1/60 typical), `Oversampling` (the exporter's
  samples-per-frame factor; informational, knots are already in seconds), `TrackGroups[]`,
  `DefaultLoopCount`, `Flags`, `ExtendedData`. BG3 files hold one animation with one track
  group, which also appears in the root's `TrackGroups`.
* **Track group**: `Name` (the skeleton's root, e.g. `Dummy_Root`), `VectorTracks[]`,
  `TransformTracks[]`, `TransformLODErrors`, `TextTracks`, `InitialPlacement`,
  `AccumulationFlags`, `LoopTranslation`, `PeriodicLoop`, `ExtendedData`.
* **Transform track**: `Name` (bone name; bind by name, not index), `Flags`, and three inline
  `curve2` structs `OrientationCurve` (quaternion `x y z w`), `PositionCurve` (3) and
  `ScaleShearCurve` (3×3, row-major like `transform::ScaleShear`). All three are the bone's
  **parent-local** transform, in the same space as the skeleton's `Bone::Transform`; Granny
  composes them as translation · rotation · scale/shear.
* **Vector track**: `Name`, `TrackKey`, `Dimension`, `ValueCurve`. Seen: `Textkeys` (46k files,
  mostly dialogue), `IKFK`, `inverseScaleX/Y/Z`, `visibility`, `lockInfluenceWeights`,
  `blendParent1`; all 1-dimensional.
* A `curve2` is one `VariantReference` `CurveData`. The variant type's first member is an inline
  `CurveDataHeader_<Format>` struct `{UInt8 Format, UInt8 Degree}`; the **member name** gives the
  format and the `Format` byte its index.

Time: knots are seconds from 0. Across 14.2 M animated curves the first knot is always 0 and the
last equals `Duration` (to within quantisation: p99 6 ms).

### Curve formats

A curve is a B-spline with paired knots and controls: knot *i* is a time, control *i* a value of
`dimension` floats. Formats differ in storage only:

| # | Format | Members after the header | Decoded value |
|---|--------|--------------------------|---------------|
| 0 | `DaKeyframes32f` | `Dimension`, `Controls[]` | one control per frame (not seen) |
| 1 | `DaK32fC32f` | `Padding`, `Knots[]`, `Controls[]` | as stored |
| 2 | `DaIdentity` | `Dimension` | identity: 0 / `(0,0,0,1)` / 3×3 identity |
| 3 | `DaConstant32f` | `Padding`, `Controls[]` | one control |
| 4 | `D3Constant32f` | `Padding`, `Controls[3]` | one control |
| 5 | `D4Constant32f` | `Padding`, `Controls[4]` | one control |
| 6/7 | `DaK16uC16u` / `DaK8uC8u` | `OneOverKnotScaleTrunc`, `ControlScaleOffsets[]`, `KnotsControls[]` | `c·scale[i] + offset[i]` |
| 8/9 | `D4nK16uC15u` / `D4nK8uC7u` | `ScaleOffsetTableEntries`, `OneOverKnotScale`, `KnotsControls[]` | quaternion, below |
| 10/11 | `D3K16uC16u` / `D3K8uC8u` | `OneOverKnotScaleTrunc`, `ControlScales[3]`, `ControlOffsets[3]`, `KnotsControls[]` | `c[i]·scale[i] + offset[i]` |
| 12/14 | `D9I1K16uC16u` / `D9I1K8uC8u` | `OneOverKnotScaleTrunc`, `ControlScale`, `ControlOffset`, `KnotsControls[]` | uniform-scale diagonal `c·s + o` |
| 13/15 | `D9I3K16uC16u` / `D9I3K8uC8u` | as D3K, 3 controls per knot | diagonal `(c[i]·s[i] + o[i])` |
| 16 | `D3I1K32fC32f` | `Padding`, `ControlScales[3]`, `ControlOffsets[3]`, `KnotsControls[]` (f32) | `c·scale[i] + offset[i]` |
| 17/18 | `D3I1K16uC16u` / `D3I1K8uC8u` | as D3K, 1 control per knot | `c·scale[i] + offset[i]` |

* `KnotsControls` stores all *n* knots first, then all controls; *n* = length / (1 + controls per
  knot). Integers are taken as exact values (no normalisation).
* Quantised knot `k` → time `k / OneOverKnotScale`. `OneOverKnotScaleTrunc` is the top 16 bits of
  an f32: `f32::from_bits(trunc << 16)`.
* `ControlScaleOffsets` (`Da…` formats) holds all `dimension` scales, then all offsets.
* D9 formats expand to a 3×3 matrix with zeros off the diagonal.

Survey (`examples/anim_survey.rs`, curves per role; Models + English_Animations; every curve of
degree 0 is a constant/identity or a `DaK32fC32f` step curve, every other one is degree 2):

| Role | Format (count) |
|------|----------------|
| Orientation | `D4nK16uC15u` 10.94 M, `D4Constant32f` 9.45 M, `DaIdentity` 3.81 M, `D4nK8uC7u` 1.61 M, `DaK32fC32f` 96k (5.7k of degree 0) |
| Position | `D3Constant32f` 21.71 M, `DaIdentity` 2.65 M, `D3K16uC16u` 727k, `D3K8uC8u` 476k, `D3I1K8uC8u` 241k, `D3I1K16uC16u` 62k, `DaK32fC32f` 27k (1.2k of degree 0), `D3I1K32fC32f` 277 |
| Scale/shear | `DaIdentity` 25.82 M, `DaConstant32f` 73k, `D9I1K8uC8u` 2.4k, `D9I1K16uC16u` 1.3k, `DaK32fC32f` 81, `D9I3K8uC8u` 45, `DaK16uC16u` 39, `DaK8uC8u` 33, `D9I3K16uC16u` 16 |
| Vector | `DaK16uC16u` 37k, `DaK8uC8u` 18k, `DaK32fC32f` 937 |

`DaKeyframes32f` does not occur; its implicit knots (frame indices) are a guess.

### Quantised quaternions (`D4nK16uC15u`, `D4nK8uC7u`)

Each control is three words (u16 or u8) storing the three smallest components of a unit
quaternion; the largest is rebuilt.

* With *top* the word's high bit (`0x8000` / `0x80`): the omitted component's index is
  `(word1.top << 1) | word2.top`; its sign is negative if `word0.top` is set.
* The stored components are the ones after the omitted index, cyclically: words 0, 1, 2 hold
  components `(m+1)&3`, `(m+2)&3`, `(m+3)&3`.
* Component *c* = `(word & 0x7fff) · scale[c] / 32767 + offset[c]` (8-bit: `& 0x7f`, `/ 127`),
  where nibble *c* of `ScaleOffsetTableEntries` (bits `4c..4c+3`) picks a (scale, offset) range
  from a fixed 16-entry table. Entries 0–7, in units of `1/√2`: (2, −1), (1, −½), (½, −¾), (½, −¼),
  (½, ¼), (¼, −¼), (¼, −⅛), (¼, 0) — nested sub-ranges of `[−1/√2, 1/√2]`, the range of every
  component but the largest. Entries 8–15 are entries 0–7 with scale and offset negated. The
  table is `granny::QuaternionCurveScaleOffsetTable` in the game binary; the decoding steps follow
  `granny::CurveExtractKnotValuesD4nK16uC15u` (both read with objdump, 4.1.1.7631656).
* Omitted component = `±sqrt(1 − Σ stored²)` (clamped at 0).

Checks over all 689 M controls: the stored three never exceed unit length, and 99.994% of
consecutive controls lie in the same hemisphere (positive dot product), which only holds with the
right component order and sign bit; the median step between controls is 0.7° (16-bit) / 0.5°
(8-bit). The first frame of every `HUM_M_Rig` animation sits at the same distribution of angles
from the `HUM_M_Base` bind pose for the quantised curves (median 18–19°) as for the float
`DaK32fC32f` curves (19°).

### Evaluation

Granny evaluates with non-uniform B-splines over the stored knots, without an explicit knot
vector (`granny::FindKnot`, `granny::ConstructBSplineBuffers`, `granny::SampleBSpline*`):

1. `k` = index of the first knot **greater** than `t`, clamped to `n − 1`.
2. For degree *d*, take knots `k−d … k+d−1` and controls `k−d … k`. Indices past the start
   repeat knot 0 / control 0, past the end repeat the last knot / control (a clamped curve, so
   the ends interpolate the first and last controls). When playing **looped** (Granny's
   underflow/overflow loop flags), the last knot (a copy of the first one period later) is
   dropped and the window instead wraps to the other end, shifting knots by `±Duration`.
3. Evaluate with de Boor's algorithm on the interval `[knot k−1, knot k)`.

Degree 0 therefore returns control `k` (the control of the *next* knot), degree 1 interpolates
linearly. Granny normalises sampled quaternions. An identity curve gives the identity of its
component (zero translation, identity rotation/scale), not the bind pose.

## Tools

* `examples/gr2_survey.rs`: header/codec census across all paks.
* `examples/gr2_scan.rs`: vertex format census and QTangent convention check.
* `examples/gr2_dump.rs`: dump the generic object tree of a file.
* `examples/anim_survey.rs`: curve format census and the quaternion/rest-pose checks above.

## Credits

* Norbyte's **LSLib** (MIT): GR2 header/section/member-type layout and the vertex component
  names/QTangent convention.
* Mackenzie Straight's **libbg3 / pybg3** (MIT), especially `docs/bitknit2.txt`: the BitKnit 2
  bitstream description that the decoder here was written from.
* Fabian Giesen's public blog posts on BitKnit, rANS and interleaved entropy coders.
* LSLib's curve readers (MIT) document the `CurveDataHeader_*` member layouts; the
  dequantisation and evaluation rules here were checked against the game binary's Granny
  routines (symbol names above) and the survey, and written fresh.

The code here is a fresh Rust implementation. No Granny or game code was copied.
