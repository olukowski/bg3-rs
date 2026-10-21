# Granite virtual textures (`.gts` / `.gtp`)

Implemented in `crates/bg3-vt`. Verified against the reference install by
`cargo test -p bg3-vt --release -- --ignored`: all 16 tile sets parse, every flat tile resolves to an
existing chunk, a sample of tiles from each of the 12,976 page files decodes, and all 81 virtual
textures used by Nautiloid (`TUT_Avernus_C`) materials extract. With `BG3_VT_SAMPLES=all` the test
decodes all 4,623,209 tiles (about 30 s) with zero failures.

Many BG3 environment materials (shader graphs named `*_VT`) take their albedo, normal and physical
maps from a Granite (Graphine) virtual texture rather than from DDS files. All data lives in
`VirtualTextures.pak` (20 parts, every entry stored uncompressed) under
`Generated/Public/VirtualTextures/`.

## From a material to a texture

1. **Material** (`MaterialBank` → `Resource`) has a child `VirtualTextureParameters`
   `{ParameterName: "virtualtexture", ID, Index, Enabled, …}`.
2. **`ID`** names a `VirtualTextureBank` → `Resource` (usually in the same bank file):
   `{ID, Name, GTexFileName, VirtualTextureLayerConfig, ReferencedColorSpaces, Prefetch,
   PrefetchMipLevel}`. `GTexFileName` is a 32-digit hex name such as
   `20fa472cdf6744af0a005eec12f40072`.
3. **Tile set.** `VirtualTextureLayerConfig` is always 3 in the reference build, meaning the
   "Albedo_Normal_Physical" layer layout. The tile set file is
   `Generated/Public/VirtualTextures/Albedo_Normal_Physical_<c>.gts`, where `<c>` is the first
   character of the GTex name (`0`–`f`, so 16 tile sets). The game builds this path in
   `ls::VirtualTextureLayerConfig::GetPath(Type, Optional<char>, bool)`; the install test checks
   that every texture sits in the tile set named after its first character.
4. **Texture rectangle.** The tile set's metadata lists each GTex name with its rectangle in the
   tile set's virtual image (see FourCC metadata below).
5. **Page file.** Builds use the "GTP per GTex" paging strategy, so a texture's tiles mostly live
   in `Albedo_Normal_Physical_<c>_<GTex>.gtp`. Tiles that are identical to existing ones are
   shared, so other page files can be referenced too. The tile tables are authoritative.

Example: material `WALL_MF_Nautiloid_Interior_Ground_A` (shader `ENV_DetailMap_VT`) →
VT resource `b90eb801-…` → GTex `20fa472c…` → `Albedo_Normal_Physical_2.gts`, rectangle
(28672, 24576) with size 4096×2048.

## Tile set (`.gts`), little-endian

### Header (0xC0 bytes)

| Offset | Type | Field |
|--------|------|-------|
| 0x00 | `[u8;4]` | magic `GRPG` |
| 0x04 | u32 | version = 5 |
| 0x08 | u32 | unused |
| 0x0C | `[u8;16]` | GUID |
| 0x1C | u32 | layer count |
| 0x20 | u64 | layer table offset |
| 0x28 | u32 | level count |
| 0x2C | u64 | level table offset |
| 0x34 | i32 ×3 | tile width, tile height, tile border |
| 0x40 | u32 | unknown (a tile count) |
| 0x44 | u32 | flat tile count |
| 0x48 | u64 | flat tile table offset |
| 0x50 | u32 ×2 | unknown (0) |
| 0x58 | u32 | packed tile ID count |
| 0x5C | u64 | packed tile ID table offset |
| 0x64 | u32 ×7 | unknown (0) |
| 0x80 | u32 | page size (bytes) |
| 0x84 | u32 | page file count |
| 0x88 | u64 | page file table offset |
| 0x90 | u32 | FourCC metadata size |
| 0x94 | u64 | FourCC metadata offset |
| 0x9C | u32 | parameter block count |
| 0xA0 | u64 | parameter block header table offset |
| 0xA8 | u64 | thumbnail table offset (not read) |
| 0xB0 | u32 ×4 | unknown (0) |

Offsets are absolute and often unaligned.

In BG3 all 16 tile sets use 144×144 tiles with an 8-pixel border, giving 128×128 pixels of
content per tile. Pages are 1 MiB. There are 9–10 levels, and level 0 is 512–1024 × 256–512 tiles,
so a virtual image is up to 131072×65536 pixels.

### Layers: `{data_type: u32, default_color: u32}`

Data types follow Granite's enum: 0 `R8G8B8_SRGB`, 1 `R8G8B8A8_SRGB`, 2 `X8Y8Z0_TANGENT`,
3 `R8G8B8_LINEAR`, 4 `R8G8B8A8_LINEAR`, 5 `X8`, 6 `X8Y8`, 7 `X8Y8Z8`, 8 `X8Y8Z8W8`, and others.
In BG3 every tile set has layer 0 = `R8G8B8A8_SRGB` (albedo) and layers 1 and 2 = `X8Y8Z8W8`
(normal and physical, linear). The layer *names* are only present in the embedded build project XML
(see `PROJ` below).

### Levels: `{width: u32, height: u32, indices_offset: u64}`

Level 0 is full resolution, and each further level halves it. At `indices_offset` there are
`width × height × layers` u32 entries, index `layer + layers × (x + y × width)`:

- **High bit clear:** an index into the flat tile table. The tile is stored for this level.
- **High bit set:** the tile isn't stored at this level. The low 31 bits are the flat tile of the
  *nearest coarser level whose covering tile is stored*, i.e. the stored tile at
  `(x >> k, y >> k)` of level `level + k` for the smallest such `k`. Sampling it upsampled is what
  the GPU would show. The install test confirms this for all 1.6 million such entries. In BG3 this
  mostly happens for the normal and physical layers of some textures, which are authored at half
  resolution or lower.

Entries outside every texture point at arbitrary placeholder tiles.

### Flat tiles (12 bytes): `{page_file: u16, page: u16, chunk: u16, unknown: u16 (1), packed_tile: u32}`

`packed_tile` indexes the packed tile ID table: a u32 with `layer` in bits 0–3, `level` in bits
4–7, `y` in bits 8–19 and `x` in bits 20–31. Identical tiles are **deduplicated**: about 374k
level-index entries share a flat tile with another position or level. The packed ID then names
only one of the places the tile is used, so don't derive positions from it.

### Parameter blocks

Header (20 bytes): `{id: u32, codec: u32, size: u32, offset: u64}`. Codecs: 0 = Uniform,
9 = BC (Granite also defines Color420, Normal, RawColor, Binary, Half, MultiChannel, ASTC, …, but
none are used by BG3).

- **BC** (0x38 bytes): `u16 version (0x238E)`, `char[16]` compression (`"lz77"`), `char[16]`
  compressor (`"fastlz0.1.0"`), u32 0, 3 × u8 0, `u8 data_type`, u16 0, `[u8;4] FourCC`
  (`"BC3 "`), u8 0, `u8 embedded_mip` (1), u16 0, u32 0. LSLib also handles `"lz4"` /
  `"lz40.1.0"` and `"raw"`, and so does `bg3-vt`, but BG3 ships only FastLZ.
- **Uniform** (0x10 bytes): `u16 version (0x42)`, u16 0, `u32 width (4)`, `u32 height (1)`,
  `u32 data_type`.

Survey of the reference build: 46,644 BC blocks (all `BC3`, FastLZ, embedded mip; 15,600 sRGB and
31,044 linear) and 2,882 uniform blocks.

### Page file table (536 bytes each)

`{name: UTF-16LE[256] (NUL-padded), num_pages: u32, guid: [u8;16], flags: u32 (2)}`. Names are
relative to the tile set's directory. `guid` must equal the page file's header GUID.

### FourCC metadata

This is a tree of tagged elements. Each element has an 8-byte header
`{fourcc: [u8;4], kind: u8, extended: u8, length: u16}`. When `extended == 1`, a u32 follows that
holds bits 16 and up of the length. Then comes the value, padded to 4 bytes relative to the start
of the file. Kinds are 1 = node (the value is a list of children), 2 = UTF-16LE string with NUL,
3 = u32, 8 = binary, 13 = binary GUID.

Layout in BG3: `META` → `ATLS` → `TXTS` → one `TXTR` per texture with `NAME` (GTex name), `WDTH`,
`HGHT`, `XXXX`, `YYYY` (the rectangle in level-0 pixels), `ADDR` (address mode, `"None"`), `SRGB`
and `THMB` (both binary). `META` also holds `PROJ`, the Granite build project XML, whose
`<LayerDescription Name="Albedo|Normal|Physical" CompressionFormat="BC3" DataType=…>` names the
layers.

Textures are placed on tile boundaries. At level `L` a texture's rectangle is simply the level-0
rectangle shifted right by `L`.

## Page file (`.gtp`)

The file is a sequence of `page_size` (1 MiB) pages. The first page starts with a 24-byte header
`{magic "GRAP", version: u32 = 4, guid: [u8;16]}`. Each page then has `u32 count, u32
offsets[count]` (relative to the page start). At each offset there is a chunk
`{codec: u32, parameter_block: u32, size: u32}` followed by `size` bytes:

- **Uniform:** 4 bytes RGBA. The whole tile is that colour.
- **BC:** the payload is compressed as the parameter block says. It decompresses to the tile's
  BCn blocks (144×144 px, i.e. 36×36 BC3 blocks = 20,736 bytes), followed by the embedded
  half-resolution copy of the tile (72×72 px = 5,184 bytes) when `embedded_mip` is set.
  Stripping the 8-pixel (2-block) border on each side leaves the 128×128 content.

Of all 4,623,209 tiles, 830 are uniform and the rest are BC3.

### FastLZ

FastLZ is Ariya Hidayat's LZ77 variant (MIT). The top 3 bits of the first byte select level 1
or 2. Each instruction starts with a control byte:

- `< 32`: a literal run of `ctrl + 1` bytes.
- Otherwise: a match of length `(ctrl >> 5) + 2` from distance `((ctrl & 31) << 8 | next) + 1`.
  A length field of 7 means "more length follows": one byte in level 1, or bytes up to and
  including the first one below 255 in level 2.
- Level 2 only: a low byte of 255 with high bits 31 escapes to a 16-bit big-endian distance plus
  8191.

## Extraction

To assemble the layer image of a texture at level `L` (`TileSet::extract`):

1. Shift the texture's rectangle right by `L` and cover it with tiles of 128 content pixels.
2. For each tile, look up the level index. Decode the stored tile, then copy the part of its
   content (offset by the border) that lies inside the rectangle into the output.
3. BC output copies 4×4 blocks and needs the rectangle on a 4-pixel grid. Uniform tiles are
   written as solid BC3 blocks. RGBA output decodes the blocks.
4. If a tile isn't stored at `L` (fallback entry), RGBA output upsamples the covering tile of the
   coarser level (nearest neighbour). BC output fails instead; use
   `TileSet::finest_stored_level` (`VirtualTextures::extract` clamps to it automatically).

Extracting the Nautiloid ground (4096×2048, 3 layers, 10 mip levels) takes well under a second
from the memory-mapped pak.

## Open questions

- The meaning of the remaining header fields (0x40, 0x50, 0x64.., 0xB0..) and of
  `FlatTileInfo.unknown` (always 1). Also the `SRGB`/`THMB` blobs and the thumbnail table.
- `ReferencedColorSpaces` (6 in all banks seen) and the `Index` of material VT parameters.
- How the shader samples the layers, e.g. which channels of "Physical" hold what. This needs the
  `ENV_*_VT` shader graphs.

## Credits

- Norbyte's [LSLib](https://github.com/Norbyte/lslib) (MIT). Its `VirtualTextures` reader and
  writer document the tile set header, tables, FourCC encoding and chunk layout. Field names here
  follow it where it names them.
- [FastLZ](https://github.com/ariya/FastLZ) by Ariya Hidayat (MIT), for the compression format.

`bg3-vt` is a fresh Rust implementation written from these format descriptions and from
inspection of the install.
