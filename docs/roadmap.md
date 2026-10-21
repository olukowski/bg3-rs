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
- [x] `bg3-gr2`: all 285,055 GR2 files parse (BitKnit2, reflection, meshes, skeletons) and all 228,541 animations in Models/English_Animations decode
- [x] `bg3-world`: resource banks, root templates, level objects (all Nautiloid scenery resolves to GR2 or effect)
- [x] Terrain: `.patch` heightfields, holes, layer weights, brushes (`docs/research/terrain.md`)
- [x] Lights: placed + combined lights via template chains, level sun/sky/exposure (`docs/research/lighting.md`); units approximate
- [ ] Level templates, sub-level composition (which levels make up `WLD_Main_A`), fog volumes, light probes
- [x] `bg3-viewer`: fly-cam through the Nautiloid with real LOD0 meshes, textured materials, terrain and level lights
- [x] Materials, first approximation: `bg3-material` (shader-graph parameters, instance overrides, slots, BC re-packing); DDS + virtual textures → `StandardMaterial` (docs/research/materials.md)
- [ ] Materials at full fidelity: custom shaders (detail maps, SSS, translucency, glow animation, glass dithering, vertex-colour masks)
- [x] Handedness: BG3 is left-handed Y-up; viewer mirrors Z (docs/research/coordinate-space.md). Still worth confirming with an in-game screenshot.

## M3 — Rules core (headless)
- [x] `bg3-stats` parsing + `using` resolution (all 157 files)
- [x] Functor/condition/boost/formula parsing, Thoth (`.khn`) parser + interpreter, condition evaluation (`bg3-rules`, docs/formats/stats-expressions.md)
- [ ] Validate roll streams and roll outcomes against traces of the original
- [ ] Rolls, boosts, statuses, action resources — validated against traces from the original
- [ ] Turn-based combat loop for a scripted encounter

## M4 — Story
- [x] `bg3-story`: compiled story loader (formats 1.13–1.15, byte-exact round-trip of all 6 stories) + `bg3x story`
- [x] `bg3-osiris` VM (libOsiris semantics, host trait, boots GustavDev with a mock host)
- [x] Osiris runtime reference trace + replay test (`docs/research/osiris-vm.md`)
- [x] Engine API inventory of the opening (`docs/research/osiris-engine-api.md`)
- [x] `bg3-sim`: flags, tags, statuses (minimal), timers, entities; the opening's story runs on
  sim answers with every graded answer equal to the game's (`bg3x sim-replay`)
- [ ] Run Nautiloid goals headlessly without a trace (character creation, loot, HP/death,
  perception still come from the trace)
- [x] Dialog trees (`.lsf` dialogs) with flags/checks: `bg3-dialog` runner, `bg3x dialog`, viewer `--dialog` (`docs/formats/dialog.md`)
- [x] Integration step 1: `bg3-sim` is the `DialogWorld` and the `RulesContext` (character
  sheets from stats, dialog checks through `bg3-rules`); the story starts dialogs on the sim and
  all six dialogs of the recorded opening run on it event for event (`dialog_e2e`,
  `bg3x sim-replay`, `bg3x dialog --walk --sim`); `bg3_sim::Game` packages VM + sim for a front end
- [x] Integration step 2: the viewer drives a live `bg3_sim::Game` (`bg3-viewer --play`,
  headless `bg3x play`): new game started like the trace (character creation skipped, placeholder
  avatar), characters/items follow on-stage state and story teleports, the avatar's position goes
  back to the sim and trigger volumes raise `EnteredTrigger`/`LeftTrigger`, clicking a character
  raises `DialogStartRequested`, story dialogs show in a panel with rolls (`live_game` tests)
- [ ] Live game gaps: item use (`UseStarted`, e.g. the Brinepool), character creation, combat/HP,
  perception, path-found scripted moves, the ~60 story functions `bg3x play` lists as missing

## M5 — First playable slice
- [x] Characters: visuals, skinning, idle animation, weapons in hand (docs/research/characters.md)
- [x] Navigation: AI grid + nav cloud parsing, path finding (`bg3-nav`), viewer `--nav` overlay and click-to-move at template run speed (docs/formats/navcloud.md)
- [ ] Navigation fidelity: bidirectional search, game smoothing passes, height-cost tunables, legacy grid versions, layers/portals/climbing, doors; compare with traced paths
- [ ] Character shaders, armour equipment visuals, subsets/sheathing, death poses; locomotion start/stop blending
- [ ] Minimal UI; dialogue; the Nautiloid end to end

## Later / side projects
- XAML (Noesis) → Bevy UI as a standalone, BG3-independent crate.
- Timelines (cinematic dialogue), Wwise audio, virtual textures at full fidelity.
- Save game compatibility; mods via VFS overrides; multiplayer.
