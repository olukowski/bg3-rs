# Architecture

## Layers

```
 game / viewer (Bevy)      bg3-viewer          (planned: bg3-game, UI, audio, animation plugins)
                                │
 simulation (headless)     bg3-osiris · bg3-sim · bg3-dialog · bg3-rules · bg3-stats · bg3-world   (planned: nav)
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
| `bg3-dialog` | Dialog model, resources, headless runner; game state via the `DialogWorld` trait | reproduces the recorded Nautiloid dialogs (on a memory world and on `bg3-sim`) |
| `bg3-sim` | Headless world state and the engine side of Osiris: entities, flags, tags, statuses, timers, character sheets, the dialog manager; `Host` impl; `DialogWorld` and `RulesContext` impls; `Game` (VM + sim, ticked, player input, notifications); trace validation (`sim-replay`) | answers 88% of the opening's engine queries, all graded answers equal the game's; runs the opening's six dialogs itself, event for event |
| `bg3-gr2` | Granny GR2 reader with BitKnit2 decompression | meshes, skeletons and animations (all 228,541 decode) |
| `bg3-vt` | Granite virtual textures | done |
| `bg3-material` | Shader-graph parameters, material instances, texture slot/channel mapping | first approximation |
| `bg3-world` | Resource banks, root templates, levels, terrain, lights | done for rendering |
| `bg3-rules` | Stats expression languages (functors, conditions, formulas, boosts), Thoth (Lua 5.3) parser and interpreter, rolls (attacks, saves, ability checks), `RulesContext` | all fields and `.khn` parse; single-target casts evaluate; runs on `bg3-sim` entities |
| `bg3x` | Command-line inspector over all of the above | |
| `bg3-viewer` | Bevy level viewer | |

Planned: navigation, combat flow, and UI. See the [roadmap](roadmap.md).

## The simulation core: who depends on whom

```
 bg3-osiris (VM)   bg3-dialog (runner, DialogWorld)   bg3-rules (rolls, conditions, RulesContext)
        \                       |                               /
         └────────────── bg3-sim (world state) ────────────────┘
                                 │
                   bg3x · tests · (next) the Bevy game layer
```

`bg3-sim` implements the traits of the other three (`bg3_osiris::Host`,
`bg3_dialog::DialogWorld`, `bg3_rules::RulesContext`) and owns the state they work on; they
don't know about it. That mirrors the original, where the server simulation (`esv`) owns the
dialog manager and evaluates the shared rules (`eoc`) on its own entities, and keeps the
runner and the rules testable on their own (`MemoryWorld`, `TestWorld`). No integration crate is
needed: the dependency only goes one way, and putting the glue in the sim means a dialog's flag
writes, tag changes and rolls are plain sim operations that raise the same story events as
the Osiris calls do.

The front end drives one `bg3_sim::Game` (the story VM plus the `Sim`): `tick(dt)` every frame
(requested dialogs start, timers fire, queued events reach the story), player input through
`dialog_advance` / `dialog_choose`, and `take_notifications()` for what to show (dialog lines,
choices, rolls). Everything stays headless and deterministic (seeded dice), so the trace harness
and the game run the same code.
