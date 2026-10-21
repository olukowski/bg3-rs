# ADR 0001 — Provenance and extraction policy

Status: accepted · 2026-10-21

## Context

We want 1:1 behaviour with BG3, as an open-source project that can be published. Two forces:

- Much of the game is **data** (paks: stats, story, dialogs, meshes, textures, UI XAML, …) that can
  be read from the user's install at runtime.
- Much of the **behaviour** lives only in the binary (rules evaluation, AI scoring, turn order,
  physics glue, constants compiled into code). Data alone can't give us 1:1.

We also want the project to respect Larian's work: it should be useless without the original game,
and it should not redistribute their code or content.

## Decision

Sources of truth, in order of preference:

| Tier | Source | How | Committed? |
|------|--------|-----|------------|
| 1 | Game data files | Parsed at runtime from the user's paks | Parsers yes, data never |
| 2 | Data embedded in the binary (tables, constants, enum/name lists, layouts) | Deterministic extractor tool in this repo, run against the user's binary at build/first run, output to `.cache/` | Extractor yes, output never |
| 3 | Runtime behaviour of the original | lldb / Frida / log capture of the user's running game → reference traces for tests | Capture tooling yes, traces never |
| 4 | Static RE (REA, decompilation, symbol index) | For **locating and understanding** logic | Prose notes + symbol names yes; pseudocode/asm no |

Rules:

- Behaviour found via tier 4 is **reimplemented** in idiomatic Rust, not transliterated from
  decompiler output. Each port cites the original symbol and game version.
- If something is a *table* rather than *logic*, prefer moving it to tier 2 over hand-copying values.
  Small, unavoidable constants (e.g. a single threshold) may be written inline with a citation.
- Verification of behaviour should, where feasible, be against tier 3 traces or real save files.

## Consequences

- The game cannot run without a BG3 install, and the repo can be public.
- Tests split into install-free (CI) and install-required (`--ignored`) suites.
- Build/run pipeline needs a game-version check; extracted caches are keyed by game version.
- Tier 2 extraction is tied to the binary layout of a specific build; we pin a reference build
  (4.1.1.7631656, macOS) and treat other builds as best-effort.

## Open questions

- License of our own code (GPL-3.0 like nv-rs vs MIT/Apache-2.0 like the Bevy ecosystem). Standalone
  reusable crates (e.g. a XAML→Bevy UI layer) likely want MIT/Apache regardless.
- Windows support: the Windows `bg3.exe` is not the RE reference; data formats are shared, so this
  mostly affects tier 2/3 tooling.
