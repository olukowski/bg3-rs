#!/usr/bin/env bash
# Compare scene radiance with the game's own light probe captures: meter each probe's specular
# capture faces like the exposure pass (bg3x example `light_survey`, METER_PROBE) and the viewer
# from the probe's position along the same six directions (uniform metering, metered target
# before clamping). Prints EV100 per face (+X −X +Y −Y +Z −Z, BG3 space): game, ours, ours − game.
# Game-derived output: keep it in .cache/. See docs/research/lighting.md ("Calibrating against
# the game's captures").
#
# Usage: scripts/probe-calibration.sh [LEVEL] PROBE_NAME...
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
level=TUT_Avernus_C
if [[ $# -gt 0 && $1 != LP_* && $1 != LightProbe* && $1 != GUS_* ]]; then
    level=$1
    shift
fi
# A point inside the ship, so the lighting trigger with the probes' captures is in effect.
at=${PROBE_AT:--46.6,3.5,-402.8}
mkdir -p .cache/shots
cargo build --release -q -p bg3-viewer
cargo build --release -q -p bg3x --example light_survey

for name in "$@"; do
    line=$(METER_PROBE=$name cargo run --release -q -p bg3x --example light_survey -- \
        "$level" "$at" 1 | grep "metered EV100" | head -1)
    if [[ -z $line ]]; then
        echo "$name: no capture for the lighting at $at"
        continue
    fi
    pos=$(sed -E 's/.* at \[([^]]*)\].*/\1/; s/ //g' <<< "$line")
    game=$(sed -E 's/.*\): //' <<< "$line")
    IFS=, read -r x y z <<< "$pos"
    ours=()
    for d in "1,0,0" "-1,0,0" "0,1,0.01" "0,-1,0.01" "0,0,1" "0,0,-1"; do
        IFS=, read -r dx dy dz <<< "$d"
        look=$(awk -v x="$x" -v y="$y" -v z="$z" -v a="$dx" -v b="$dy" -v c="$dz" \
            'BEGIN { printf "%f,%f,%f", x + 10 * a, y + 10 * b, z + 10 * c }')
        ev=$(BG3_METERING=0 BG3_POST_DEBUG=1 target/release/bg3-viewer "$level" \
            --camera="$x,$y,$z" --look-at="$look" --shot .cache/shots/probe-calibration.png \
            --shot-after 4 2>&1 | grep "post:" | tail -1 | sed -E 's/.*target ([-0-9.]+)\).*/\1/')
        ours+=("$ev")
    done
    diff=$(awk -v g="$game" -v o="${ours[*]}" 'BEGIN {
        n = split(g, gs, " "); split(o, os, " "); s = 0
        for (i = 1; i <= n; i++) { d = os[i] - gs[i]; s += d; printf "%+.2f ", d }
        printf "(mean %+.2f)", s / n }')
    echo "$name"
    echo "  game  $game"
    echo "  ours  ${ours[*]}"
    echo "  diff  $diff"
done
