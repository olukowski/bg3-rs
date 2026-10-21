# Architecture (proposed — will evolve)

## Layers

```
            ┌──────────────────────── game / viewer (Bevy apps) ───────────────────────┐
            │  bg3-viewer      bg3-game                                                │
            └───────────────┬──────────────────────────────────────────────────────────┘
       Bevy plugins         │  bg3-render   bg3-ui   bg3-audio   bg3-anim             │
                            │                                                          │
       simulation (no render)  bg3-sim (esv)   bg3-rules (eoc)   bg3-osiris   bg3-nav  │
                            │                                                          │
       formats (no Bevy)       bg3-vfs   bg3-pak   bg3-lsf   bg3-stats   bg3-loca      │
                               bg3-gr2   bg3-vt    bg3-story                           │
                            │                                                          │
       install & extraction    bg3-install (locate, version check)  bg3-binx (tier-2)  │
            └──────────────────────────────────────────────────────────────────────────┘
       tools: bg3x (CLI: list/extract/dump/convert), scripts/
       standalone (no BG3 deps, publishable): e.g. bevy_xaml_ui (later)
```

## Principles

- **Formats crates are pure Rust, no Bevy**, fuzzable, tested with self-authored fixtures + `--ignored`
  install tests.
- **Simulation is headless.** Rules/AI/story run without a renderer so they can be tested against
  reference traces quickly and deterministically (seeded RNG, fixed tick).
- **Presentation is Bevy plugins** that read simulation state.
- Mirror the original's `eoc` / `esv` / `ecl` split where it helps traceability. Whether to keep a real
  client/server boundary (needed for multiplayer parity) is an open ADR.
- **VFS** mirrors the game's pak load order and override rules so mods could work like the original.

## Crate sketch

| Crate | Responsibility |
|-------|----------------|
| `bg3-install` | Find the install (`BG3_APP` / Steam default), read version, list paks |
| `bg3-pak` | LSPK v18 reader (file table, compression, multi-part) |
| `bg3-vfs` | Unified read-only view over all paks with load order/overrides |
| `bg3-lsf` | LSF (binary) / LSX (XML) / LSJ resource trees |
| `bg3-loca` | Localization `.loca` → handle → string |
| `bg3-stats` | Stats text files, inheritance, functor/condition expressions |
| `bg3-story` | Compiled Osiris story loader |
| `bg3-osiris` | Osiris VM (rule database, goals, events/queries/calls) |
| `bg3-gr2` | Granny GR2 meshes/skeletons/animations |
| `bg3-vt` | Virtual textures (Granite tile sets) |
| `bg3-rules` | 5e rules as implemented by BG3 (`eoc`): rolls, boosts, statuses, spells |
| `bg3-sim` | Server-side sim (`esv`): turns, combat, AI, Osiris bindings |
| `bg3-binx` | Tier-2 extractors that read data out of the user's binary by symbol |
| `bg3x` | CLI over all of the above |
