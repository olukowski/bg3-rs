# Viewer performance

How to measure the viewer, what costs what on the Nautiloid, and the switches that trade
fidelity or cost for A/B checks.

## Measuring

- `F3` toggles an on-screen overlay; `--fps` starts with it shown and also logs it every
  second (`fps: …`). Per second it reports:
  - frame rate, mean and worst frame time;
  - CPU: the main world's update, the render world's extract-to-prepare (this includes waiting
    for the swap chain, i.e. for the GPU) and the render graph (encoding and submission);
  - GPU time per pass (Bevy's `RenderDiagnosticsPlugin`, on with `--fps`), where the backend has
    timestamp queries: Vulkan and DX12, not Metal;
  - shadow maps: directional lights with shadows (the sun and the box light carriers, and how
    many carriers are in use), point and spot lights with shadows;
  - churn: render pipelines created and shader, mesh, image and material assets changed in the
    last second. After loading these must stay at 0. Steady growth means per-frame
    re-specialisation or re-uploads, which cost far more on Vulkan (each pipeline compiles to the
    driver's ISA) than on Metal.
- `--bench <seconds>` waits until loading has settled (≥ 5 s and no pipeline created for 1 s),
  measures that long, prints `bench: N frames …: mean, median, p95, p99, max` and exits. It asks
  for no vsync (`AutoNoVsync`), keeps rendering when the window is unfocused (Bevy would
  throttle it to 60 Hz) and opens a fixed 1600×900 window (equal minimum and maximum size, which
  a tiling window manager such as Hyprland floats instead of resizing); the result line prints
  the size. Before this, benchmark windows took whatever size the tiling layout gave them and
  views differed by up to 70 % between runs (GPU-bound frames scale with pixels). **macOS** still paces a window to the display (120 Hz on the M5
  Pro: an empty view benches at exactly 8.33 ms), so Mac frame times are refresh-quantised.
  Compare medians and treat differences below one refresh interval as noise.
- `BG3_PERF_PASSES=1` (with `--fps`) logs every render diagnostic each second (nested passes,
  CPU and GPU) and, every 120 render frames, the multidrawable batch sets per view of the
  shadow, opaque, alpha-mask and prepass phases.
- `cargo build --release -p bg3-viewer --features trace` adds Bevy's `trace_chrome`: a run writes
  `trace-<timestamp>.json` to the working directory (per-system spans on every thread; open in
  ui.perfetto.dev). Run it from `.cache/` (a 10 s run writes ~700 MB).
- Other viewers running on the machine (other agents' screenshots) change the numbers by tens of
  percent: check `ps` before benchmarking, and alternate the runs being compared.

Benchmark commands (Nautiloid, fixed cameras):

```sh
V=target/release/bg3-viewer
# combat set-up, camera behind the party
$V --play --combat-test --no-follow --fps --bench 10
# wide view over the deck
$V --play --combat-test --no-follow --camera=-62,13,-392 --look-at=-44,3,-406 --fps --bench 10
# new game, start pod
$V --play --fps --bench 10
```

Visual checks that don't depend on timing: `--play` shots differ from run to run with the frame
rate (the fight runs and characters move and animate while the shot waits; the framed `imp`
shot of `scripts/compare-references.sh` moves by ±4 luma). Static level views without `--play`
and with `BG3_NO_ANIM=1`, e.g. `--camera=-41.13,3.21,-399.93 --look-at=-38.81,3.0,-397.5`, and
`scripts/probe-calibration.sh` repeat to about ±1 luma (auto-exposure timing).

## Findings (2026-10-22, M5 Pro, macOS)

Before: after the light audit the Nautiloid ran at a third of its earlier frame rate on the Mac
and as a slideshow on Linux (RTX 5060 Ti, Vulkan); `BG3_NO_BOX_SHADOWS=1` restored it on both.
Combat view, before the fix (means; churn counters were 0 after loading):

| Configuration | Frame | Main update | Render extract–prepare | Render graph |
|------|------|------|------|------|
| Default | 177.7 ms (5.6 fps) | 13 ms | 117 ms | 60 ms |
| `BG3_NO_BOX_SHADOWS=1` | 33.7 ms | 5 ms | 23 ms | 12 ms |
| `BG3_NO_FOG=1` | 192 ms | | | |
| `BG3_NO_BLOOM=1` | 195 ms | | | |

The box light shadows were > 80 % of the frame, all of it CPU work in the render world. Nine
black "carrier" directional lights each had Bevy's four cascades fitted to the camera frustum.
That made 36 shadow views a frame, each culling, specialising, queueing and drawing the whole
view. The fix (`viewer/src/lights.rs`, see `docs/research/lighting.md`, "In the viewer"):

1. **One map per box, as the game does.** Each carrier has one cascade, an orthographic
   projection fitted to its boxes (`fit_box_shadow_cascades`). Shadow culling then draws only
   what lies in a box or towards its light.
2. **Only boxes in view.** Carriers go to the boxes that intersect the camera frustum (a box
   outside it has no visible pixels), so 1–5 carriers are in use on the Nautiloid instead of 9.
3. **No churn.** Carriers stay shadow-enabled (an unused one renders an empty map), so the
   directional shadow array never changes size as the camera moves and boxes enter or leave the
   view. Disabling unused carriers saved 2–3 ms on the Mac (measured with four cascades each). It would also re-allocate the
   array whenever the in-use count changes, which a following camera does often.

After (means on a quiet machine; medians quantised to the 120 Hz refresh):

| View | Before | After | `BG3_NO_BOX_SHADOWS=1` | `BG3_BOX_SHADOW_CASCADES=1` |
|------|------|------|------|------|
| Combat set-up | 177.7–179.5 ms | 32.4–33.3 ms | 32.3–33.7 ms | 40.3–41.2 ms |
| Wide deck view | 195.6–196.5 ms | 52.4–52.6 ms | 69.9 ms | 57.0 ms |
| Start pod (`--play`) | 74.5 ms | 41.2–41.6 ms | 53.9 ms | 44.9 ms |

The render world's CPU time fell with it. Combat: extract–prepare went from 117 to 20–24 ms and
the graph from 60 to 12–14 ms. Wide: 108–130 to 40 ms and 67 to 12 ms. The main update varies
with the fight (2.5–9 ms) and is no longer a factor. The wide and start-pod views run faster
with box shadows than with `BG3_NO_BOX_SHADOWS=1`, repeatably; not investigated (the unshadowed
10000 lx box lights flood those views).
With box shadows on, the frame now costs the same as without them. In a profile (`sample`) of
the combat view after the fix, the CPU threads mostly wait: the frame is GPU-bound. GPU
attribution on the Mac is coarse (switches below one refresh interval don't show). Fog, bloom,
SSS and the post chain each cost less than one 8 ms step. The character shader
(`BG3_STANDARD_CHARACTERS=1`) and the level shader (`BG3_STANDARD_LEVEL=1`) are the largest
remaining items. Per-pass GPU times on Linux: run with `--fps`.

Visual check, ours after − before, luma (0–255): `scripts/compare-references.sh` and
`scripts/probe-calibration.sh` (see `docs/research/lighting.md`) and five static deck views
with `BG3_NO_ANIM=1`. One map per box: −0.66 to +0.17. `BG3_BOX_SHADOW_CASCADES=1`: 0.00 to −0.08
(identical by construction). A one-map box's texels are coarser near the camera than Bevy's
first cascade, so shadow edges there are slightly softer.

## Findings (2026-10-22, RTX 5060 Ti, Linux, Vulkan)

Combat view 25.3 ms (39.5 fps), wide view 23.5 ms, start pod 17.7 ms. The GPU is ~62 % busy in
the combat view (`nvidia-smi dmon -s u`): the frame is bound by the **render thread's CPU**.
Bevy's per-pass GPU timestamps are unusable here (the main opaque pass reports 3 vertices; most
passes are missing), so attribute GPU costs with A/B switches and GPU utilisation instead.

A `--features trace` profile (Bevy's `trace_chrome`, see "Measuring") of one combat frame on the
render thread (22 ms; the main app waits ~14 ms of each frame for it):

| Render-world work | ms |
|------|------|
| `write_binned_instance_buffers<Shadow>` (one task, every shadow view) | 6.0 |
| `prepare_preprocess_bind_groups` (one task) | 4.6 |
| `render_system`: shadow pass encoding, `finish` of command buffers 2.5, `queue_submit` 3.8 | 8.7 |
| everything else | ~3 |

All three scale with **multidrawable batch sets × views**. `BG3_PERF_PASSES=1` logs the binned
phases: the main view had 54 opaque + 6 alpha-mask batch sets, the 13 shadow views 1224 (sun
cascades 429/305/183/128, the two box carriers in use 121/58). A batch set is keyed by pipeline,
material bind group and mesh slabs. `StandardMaterial` declares a prepass shader, so Bevy treats
every `ExtendedMaterial<StandardMaterial, _>` as reading its material in the shadow pass and
never takes the depth-only path that would drop the bind group from the key. Our extensions
weren't bindless, so every material was its own batch set in every shadow view it is cast into:
in the largest sun cascade ~250 sets were character materials (one per body part) and ~280
level materials. The sun's far cascades cover most of the level (`ShadowFarPlane` = 100 m).

**Fix: bindless level and character materials.** Both extensions are now bindless (the
uniform as a struct-level `data` array, maps in Bevy's binding arrays, the shared box-light,
decal and surface buffers as storage-buffer binding arrays). One index table layout
(`range(100..148)` at binding 160) serves both, so the shared shader modules find their buffers
through `shaders/material_slot.wesl` whichever material they shade; terrain keeps the plain
bindings. The largest shadow view went from 534 to 23 batch sets. `BG3_NO_BINDLESS=1` turns
binding arrays off. Images: 2–28 of 1.95 M pixels differ by more than 1 % in the Lae'zel, imp,
deck and wide shots (float rounding).

| View (same session, alternating) | `BG3_NO_BINDLESS=1` | Bindless | GPU busy |
|------|------|------|------|
| Combat set-up | 24.5–25.1 ms | 15.0–15.1 ms | 85 % → 98 % |
| Wide deck view | 31.8–32.5 ms | 24.9–25.9 ms | 96 % → 99 % |
| Start pod (`--play`) | 24.8–26.4 ms | 22.5–23.1 ms | 96 % → 99 % |

All three views are now GPU-bound. Render graph CPU time fell from 9–10 to 3–4 ms. Wide and
start pod were 23.5 and 17.7 ms earlier in the day with the binary from before the change
(30.9 and 26.4 ms in this session): the window had another size (benchmarks were not yet at a
fixed size), not the change. Since this change
`--fps` reports GPU times for the main passes (why they were missing before is not known). Combat
view: main opaque pass 5.4 ms, shadow passes
4.1, volumetric fog 3.0, bin unpacking and uniform allocation 1.6.

**Box light grid.** The game's light loop visited all 52 box lights of the Nautiloid for every
pixel of every game-shaded material (`BG3_NO_BOX_LIGHTS=1` halved the main opaque pass). The
boxes are now listed per 4 m world cell (`lights::box_light_grid`, the decal grid's layout,
`decals::xz_grid`) and the loop visits only the boxes of the pixel's cell; boxes larger than
512 m (one ~10 km box) don't extend the grid, and pixels outside it visit every box. Images
match (≤ 17 pixels of 1.44–1.95 M differ by more than 1 %, as between two runs of one binary).
At 1600×900, two alternating runs each:

| View | Without grid | With grid |
|------|------|------|
| Combat set-up | 14.3–14.7 ms | 11.4–11.6 ms |
| Static deck (`BG3_NO_ANIM=1`) | 19.3–19.7 ms | 15.4–17.4 ms |
| Wide deck view (`BG3_NO_ANIM=1`) | 20.1–21.2 ms | 18.4 ms |
| Start pod (`--play`) | 17.9–18.6 ms | 16.5–16.7 ms |

The main opaque pass goes from 5.8–6.2 to 2.9–3.2 ms in the combat view and from 10.3–10.9 to
6.3–7.9 ms in the deck view. Remaining GPU time (combat): shadow passes 4.0 ms, opaque 3.0,
volumetric fog 2.3; the wide view spends 7.1–7.2 ms in volumetric fog.

Bevy 0.20 caveat: an `ExtendedMaterial`'s layout makes the base bindless whenever the base
supports it, even when the extension can't be (here: no buffer binding arrays), which gives an
invalid layout. Vulkan with descriptor indexing and Metal with tier 2 argument buffers have both,
and `level_materials::check_binding_arrays` stops with a message otherwise.

Before the fix:

A/B, combat view (means; GPU = busy %):

| Configuration | Frame | GPU |
|------|------|------|
| Default | 25.3 ms | 62 % |
| `BG3_NO_BOX_SHADOWS=1` | 18.3 ms | 76 % |
| Sun with 1 cascade (instead of 4) | 13.4 ms | 74 % |
| `BG3_NO_SUN_SHADOWS=1` | 8.2 ms | 95 % |
| both off | 5.9 ms | 97 % |

With the sun's shadows off the frame is GPU-bound, and each remaining feature costs 0.5–1.8 ms
(`BG3_NO_SUN_SHADOWS=1` plus): `NO_FOG` −1.6, `STANDARD_LEVEL` −1.9, `STANDARD_CHARACTERS`
−1.4, `NO_DECALS` −1.2, `STANDARD_TERRAIN` −1.2, `NO_SSS`/`NO_SURFACES`/`NO_BLOOM` −0.8. Wide
view 23.5 → 13.4 ms, start pod 17.7 → 8.6 ms without sun shadows (both GPU-bound then). Level
objects always draw LOD0. The main world takes ~4–5 ms (transform propagation 1.4, character
animation 1.3–1.6, skin extraction 0.9) and runs in parallel with the render world.

## Switches

- `BG3_BOX_SHADOW_CASCADES=1`: box light shadows on Bevy's four camera cascades, with casters
  culled to the boxes (`cull_box_shadow_casters`), instead of one map per box. Sharper near the
  camera, 10–25 % slower; the look before this change.
- `BG3_NO_BINDLESS=1`: no texture or buffer binding arrays, so every material is its own bind
  group and batch set (the state before bindless level and character materials).
- `BG3_NO_SUN_SHADOWS=1`, `BG3_NO_BOX_SHADOWS=1`, `BG3_NO_BOX_LIGHTS=1`, `BG3_NO_FOG=1`, `BG3_NO_BLOOM=1`,
  `BG3_NO_SSS=1`, `BG3_STANDARD_LEVEL=1`, `BG3_STANDARD_CHARACTERS=1`, `BG3_BEVY_POST=1`,
  `BG3_NO_ANIM=1`: A/B switches for attributing costs (they change the image).

## Open questions

- The game's shadow resolution for box lights (`ls::LocalLightShadowStage`: a shadow tile map
  and a shared depth target; tile size per light and quality setting not read). The viewer uses
  Bevy's directional map size (2048) over the whole box.
- Whether the game caches static shadow tiles and only re-renders them for moving casters.
  Bevy re-renders every shadow map every frame.
