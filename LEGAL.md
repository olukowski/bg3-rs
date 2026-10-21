# Legal notice

bg3-rs is an independent, non-commercial fan project. It is **not affiliated with, endorsed by, or
sponsored by** Larian Studios, Wizards of the Coast, or Hasbro.

*Baldur's Gate*, *Baldur's Gate 3*, *Dungeons & Dragons* and related names and marks belong to their
respective owners. They are used here only to describe what this software is compatible with.

## What this repository contains

- Original source code, written for this project, licensed MIT OR Apache-2.0 (see `LICENSE-MIT`
  and `LICENSE-APACHE`).
- Documentation in our own words: file-format write-ups and research notes. These cite function and
  symbol names from the original game for traceability, but contain no decompiled code.
- A few screenshots of **this project's own renderer** (`docs/media/`).

## What it does not contain

- No game files: no assets, models, textures, audio, text, scripts, story data or executables.
- No data extracted from the game, no decompiled or disassembled code, and no runtime traces.

## How it works

bg3-rs reads everything it needs from **your own, legally obtained installation** of Baldur's
Gate 3, at runtime, without modifying it. Without the original game it does nothing useful. The
rules the project follows are in
[`docs/adr/0001-provenance-and-extraction.md`](docs/adr/0001-provenance-and-extraction.md).

## Prior art

Format knowledge was informed by public community work, credited in the individual docs, notably
Norbyte's LSLib (MIT) and BG3 Script Extender, and libbg3/pybg3 (MIT) for the BitKnit2 codec.
Implementations here are written from scratch.

If you are a rights holder with a concern, please open an issue and we will respond promptly.
