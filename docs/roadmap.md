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
- [x] `bg3-lsf` (LSF/LSX/LSJ), `bg3-loca`
- [x] `bg3-vfs` (case-insensitive, last-mount-wins; original precedence rule still TODO — see docs/research/vfs-load-order.md)
- [ ] Format write-ups in `docs/formats/`

## M2 — See a level
- [x] `bg3-gr2`: all 285,055 GR2 files parse (BitKnit2, reflection, meshes, skeletons); animations not decoded yet
- [x] `bg3-world`: resource banks, root templates, level objects (all Nautiloid scenery resolves to GR2 or effect)
- [x] Terrain: `.patch` heightfields, holes, layer weights, brushes (`docs/research/terrain.md`)
- [x] Lights: placed + combined lights via template chains, level sun/sky/exposure (`docs/research/lighting.md`); units approximate
- [ ] Level templates, sub-level composition (which levels make up `WLD_Main_A`), fog volumes, light probes
- [x] `bg3-viewer`: fly-cam through the Nautiloid with real LOD0 meshes (placeholder per-material colours), terrain and level lights
- [ ] Materials: DDS textures, virtual textures (Granite), shader-graph parameters
- [x] Handedness: BG3 is left-handed Y-up; viewer mirrors Z (docs/research/coordinate-space.md). Still worth confirming with an in-game screenshot.

## M3 — Rules core (headless)
- [x] `bg3-stats` parsing + `using` resolution (all 157 files)
- [ ] Functor/condition expression parsing
- [ ] Rolls, boosts, statuses, action resources — validated against traces from the original
- [ ] Turn-based combat loop for a scripted encounter

## M4 — Story
- [x] `bg3-story`: compiled story loader (formats 1.13–1.15, byte-exact round-trip of all 6 stories) + `bg3x story`
- [x] `bg3-osiris` VM (libOsiris semantics, host trait, boots GustavDev with a mock host)
- [ ] Osiris runtime reference trace + replay test (`docs/research/osiris-vm.md`)
- [ ] Run Nautiloid goals headlessly (needs engine call/query/event bindings)
- [ ] Dialog trees (`.lsf` dialogs) with flags/checks

## M5 — First playable slice
- [ ] Characters, skinning, animation; movement & navcloud
- [ ] Minimal UI; dialogue; the Nautiloid end to end

## Later / side projects
- XAML (Noesis) → Bevy UI as a standalone, BG3-independent crate.
- Timelines (cinematic dialogue), Wwise audio, virtual textures at full fidelity.
- Save game compatibility; mods via VFS overrides; multiplayer.
