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
  for no vsync (`AutoNoVsync`). **macOS** still paces a window to the display (120 Hz on the M5
  Pro: an empty view benches at exactly 8.33 ms), so Mac frame times are refresh-quantised.
  Compare medians and treat differences below one refresh interval as noise.
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

## Switches

- `BG3_BOX_SHADOW_CASCADES=1`: box light shadows on Bevy's four camera cascades, with casters
  culled to the boxes (`cull_box_shadow_casters`), instead of one map per box. Sharper near the
  camera, 10–25 % slower; the look before this change.
- `BG3_NO_BOX_SHADOWS=1`, `BG3_NO_BOX_LIGHTS=1`, `BG3_NO_FOG=1`, `BG3_NO_BLOOM=1`,
  `BG3_NO_SSS=1`, `BG3_STANDARD_LEVEL=1`, `BG3_STANDARD_CHARACTERS=1`, `BG3_BEVY_POST=1`,
  `BG3_NO_ANIM=1`: A/B switches for attributing costs (they change the image).

## Open questions

- The game's shadow resolution for box lights (`ls::LocalLightShadowStage`: a shadow tile map
  and a shared depth target; tile size per light and quality setting not read). The viewer uses
  Bevy's directional map size (2048) over the whole box.
- Whether the game caches static shadow tiles and only re-renders them for moving casters.
  Bevy re-renders every shadow map every frame.
