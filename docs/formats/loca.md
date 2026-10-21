# `.loca` localization tables

Implemented in `crates/bg3-loca`. Verified against `Localization/English.pak` in the reference build
(4.1.1.7631656) by the ignored test `bg3_loca::tests::load_english`.

## Layout (little-endian)

| Offset | Type | Field |
|--------|------|-------|
| 0x00 | `[u8;4]` | magic `LOCA` |
| 0x04 | u32 | entry count `n` |
| 0x08 | u32 | offset of the text area (always `12 + n × 70`) |
| 0x0C | `n × 70` bytes | entry table |
| text area | | texts back to back, in entry order |

Entry (70 bytes):

| Offset | Type | Field |
|--------|------|-------|
| 0 | `[u8;64]` | handle, ASCII, NUL-padded (e.g. `h` + a GUID with `-` replaced by `g`) |
| 64 | u16 | version |
| 66 | u32 | text length in bytes **including** the trailing NUL |

Texts are UTF-8 and NUL-terminated. There is no per-entry offset: the text position is the running
sum of the preceding lengths. Texts contain markup such as `<br>`, `<LSTag …>` and `[1]`
placeholders; the reader returns them verbatim.

## Observations (reference build)

- `Localization/English/english.loca`: 237,597 entries, no duplicate handles.
- `English.pak` also contains eight 12-byte stub files (`english_to_F.loca`,
  `Gender/Female/english.loca`, `Gender/Neutral/english_X_to_M.loca`, …). They are a bare header with
  count 0 and text offset 12. Presumably the slots for gendered variants (text spoken *to* / *by* a
  given gender); empty in English.
- References elsewhere in the data (stats `TranslatedString` fields, LSX `TranslatedString`
  attributes) use `handle;version`, e.g. `h1234…;2`. `Loca::get` accepts both forms and ignores the
  version for lookup. Whether the game ever selects a text by version is not yet investigated.

## Merging

When several `.loca` files are loaded, a later file's entry replaces an earlier one with the same
handle (`Loca::merge`). Which order the game loads language files/mod overrides in is not yet
verified.

## Prior art

LSLib (Norbyte) documents the same layout (`LocaReader`). We confirmed it independently against the
shipped file (header arithmetic, length-includes-NUL, zero parse failures).
