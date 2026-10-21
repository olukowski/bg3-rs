# Architecture

## Layers

```
 game / viewer (Bevy)      bg3-viewer          (planned: bg3-game, UI, audio, animation plugins)
                                │
 simulation (headless)     bg3-osiris · bg3-dialog · bg3-stats · bg3-world        (planned: bg3-sim, bg3-rules, nav)
                                │
 file formats (no Bevy)    bg3-pak · bg3-vfs · bg3-lsf · bg3-gr2 · bg3-vt · bg3-loca · bg3-story · bg3-material
                                │
 install                   bg3-install

 tools                     bg3x (command-line inspector) · tools/osiris-trace (runtime log capture, macOS)
```

## Principles

- **Format crates are pure Rust with no Bevy dependency.** They're tested with self-authored
  fixtures, plus `--ignored` tests that run against a real install. Most of them could be published
  standalone.
- **Simulation is headless.** Rules, AI and story logic run without a renderer, so they can be
  tested quickly and deterministically against traces of the real game.
- **Presentation is Bevy plugins** that read data and simulation state. Every conversion from the
  game's left-handed space into Bevy's goes through `viewer/src/space.rs`.
- **Mirror the original where it helps traceability**: e.g. its `eoc` (shared rules), `esv`
  (server simulation) and `ecl` (client) split. Whether to keep a real client/server boundary,
  which multiplayer parity would need, is still undecided.
- **The VFS mirrors the game's pak and module layering**, so mods can eventually work like they do
  in the original.

## Crates

| Crate | Responsibility | State |
|-------|----------------|-------|
| `bg3-install` | Find the install (macOS, Windows, Linux), read its version, list paks | done |
| `bg3-pak` | LSPK v18 `.pak` reader (file table, compression, multi-part archives) | done, verified on all paks |
| `bg3-vfs` | One case-insensitive read-only view over all paks; module load order | done |
| `bg3-lsf` | LSF / LSX / LSJ resource trees: readers and writers | done, verified on all files |
| `bg3-loca` | Localization: text handle → string | done |
| `bg3-stats` | Stats text files with `using` inheritance across modules | parsing done; expressions not yet evaluated |
| `bg3-story` | Compiled Osiris story loader and pseudo-source dump | done |
| `bg3-osiris` | Osiris VM, plus trace parser and replay | matches the real game on the recorded opening |
| `bg3-dialog` | Dialog model, resources, headless runner; game state via the `DialogWorld` trait | reproduces the recorded Nautiloid dialogs |
| `bg3-gr2` | Granny GR2 reader with BitKnit2 decompression | meshes and skeletons done; animations not yet |
| `bg3-vt` | Granite virtual textures | done |
| `bg3-material` | Shader-graph parameters, material instances, texture slot/channel mapping | first approximation |
| `bg3-world` | Resource banks, root templates, levels, terrain, lights | done for rendering |
| `bg3x` | Command-line inspector over all of the above | |
| `bg3-viewer` | Bevy level viewer | |

Planned: `bg3-sim` (headless world simulation and the engine side of Osiris), `bg3-rules` (stats
expressions, rolls, combat), navigation, dialogue, animation, and UI. See the
[roadmap](roadmap.md).
