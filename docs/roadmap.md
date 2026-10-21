# Roadmap

Long-term goal: a full, 1:1 playable BG3 on Bevy. Near-term goal: prove each layer on a small slice.
First playable target: **the Nautiloid prologue** (self-contained, introduces most core systems).

## M0 — Foundations ← *current*
- [x] Workspace docs, AGENTS.md, provenance ADR, install survey
- [x] Cargo workspace skeleton
- [x] CI (fmt, clippy, install-free tests)
- [x] `scripts/symbol-index.sh` → demangled symbol index in `.cache/symbols/`
- [x] `bg3-install`: locate install, read version, enumerate paks

## M1 — Read everything
- [x] `bg3-pak` + `bg3x list/extract/cat/verify` (verify against all paks in the install)
- [ ] `bg3-lsf` (LSF/LSX), `bg3-loca`
- [x] `bg3-vfs` (case-insensitive, last-mount-wins; original precedence rule still TODO — see docs/research/vfs-load-order.md)
- [ ] Format write-ups in `docs/formats/`

## M2 — See a level
- [ ] `bg3-gr2` static meshes
- [ ] Level loading (level templates, terrain, static objects)
- [ ] `bg3-viewer`: fly-cam through a Nautiloid level with basic materials

## M3 — Rules core (headless)
- [ ] `bg3-stats` incl. functor/condition expressions
- [ ] Rolls, boosts, statuses, action resources — validated against traces from the original
- [ ] Turn-based combat loop for a scripted encounter

## M4 — Story
- [ ] `bg3-story` + `bg3-osiris` VM, run Nautiloid goals headlessly
- [ ] Dialog trees (`.lsf` dialogs) with flags/checks

## M5 — First playable slice
- [ ] Characters, skinning, animation; movement & navcloud
- [ ] Minimal UI; dialogue; the Nautiloid end to end

## Later / side projects
- XAML (Noesis) → Bevy UI as a standalone, BG3-independent crate.
- Timelines (cinematic dialogue), Wwise audio, virtual textures at full fidelity.
- Save game compatibility; mods via VFS overrides; multiplayer.
