# LSPK `.pak` (version 18)

Implemented in `crates/bg3-pak`. Verified against every pak in the reference install with
`bg3x verify`.

## Header (0x28 bytes, little-endian)

| Offset | Type | Field |
|--------|------|-------|
| 0x00 | `[u8;4]` | magic `LSPK` |
| 0x04 | u32 | version = 18 |
| 0x08 | u64 | file list offset |
| 0x10 | u32 | file list size (bytes, including its 8-byte prefix) |
| 0x14 | u8 | flags (seen: `0x00`, `0x02`, `0x10`, `0x12`) |
| 0x15 | u8 | priority (always 0 in the reference build) |
| 0x16 | `[u8;16]` | MD5 (zero in the reference build) |
| 0x26 | u16 | number of parts |

Flag meanings are not yet confirmed. Prior art names `0x02` "allow memory mapping" and `0x04`
"solid"; `0x10` is unknown. No solid archives are present.

## File list

At `file list offset`: `u32 count`, `u32 compressed_size`, then an **LZ4 block** (no frame) that
decompresses to `count × 272` bytes of entries:

| Offset | Type | Field |
|--------|------|-------|
| 0 | `[u8;256]` | path, NUL-terminated, `/`-separated |
| 256 | u32 | data offset, low 32 bits |
| 260 | u16 | data offset, high 16 bits |
| 262 | u8 | archive part |
| 263 | u8 | flags: low nibble = compression (0 none, 1 zlib, 2 LZ4 block, 3 Zstd), high nibble = level hint |
| 264 | u32 | size on disk |
| 268 | u32 | uncompressed size (0 when stored) |

## Multi-part archives

Part 0 is the `.pak` itself. Part N is `<stem>_N.pak` next to it, **headerless raw data**: for example,
`Textures_1.pak` starts with DDS data and `VirtualTextures_N.pak` starts with Granite `GRAP` pages.
`Textures.pak` has 4 parts and `VirtualTextures.pak` has 20.

## Credits

Prior art: Norbyte's LSLib (MIT), whose package reader documents this layout.
