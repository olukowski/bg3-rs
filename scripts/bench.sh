#!/usr/bin/env bash
# Viewer benchmark matrix on the Nautiloid (docs/performance.md, "Measuring").
#
#   scripts/bench.sh [VIEW…]            views: combat wide pod deck (default: combat wide pod)
#   SWITCHES="BG3_NO_FOG=1 BG3_NO_SUN_SHADOWS=1" scripts/bench.sh combat
#       runs the default plus one run per switch, alternating, ROUNDS times (default 2)
#   SECS=10 ROUNDS=2 V=target/release/bg3-viewer
#
# Prints one line per run: view, switch, median and mean frame time.
set -euo pipefail
V=${V:-target/release/bg3-viewer}
SECS=${SECS:-10}
ROUNDS=${ROUNDS:-2}
VIEWS=("$@")
[ ${#VIEWS[@]} -eq 0 ] && VIEWS=(combat wide pod)
SW=("-")
for s in ${SWITCHES:-}; do SW+=("$s"); done

view_args() {
    case $1 in
    combat) echo "--play --combat-test --no-follow" ;;
    wide) echo "--play --combat-test --no-follow --camera=-62,13,-392 --look-at=-44,3,-406" ;;
    pod) echo "--play" ;;
    deck) echo "--camera=-41.13,3.21,-399.93 --look-at=-38.81,3.0,-397.5" ;;
    *) echo "unknown view $1" >&2; exit 1 ;;
    esac
}

for view in "${VIEWS[@]}"; do
    for ((r = 0; r < ROUNDS; r++)); do
        for s in "${SW[@]}"; do
            envs=(BG3_BENCH_SCRIPT=1)
            [ "$view" = deck ] && envs+=(BG3_NO_ANIM=1)
            [ "$s" != - ] && envs+=("$s")
            # shellcheck disable=SC2046
            line=$(env "${envs[@]}" "$V" $(view_args "$view") --bench "$SECS" 2>&1 | grep 'bench:' || echo "bench: failed")
            med=$(sed -n 's/.*median \([0-9.]*\).*/\1/p' <<<"$line")
            mean=$(sed -n 's/.*mean \([0-9.]*\) ms.*/\1/p' <<<"$line")
            printf '%-7s %-28s median %6s  mean %6s\n' "$view" "$s" "$med" "$mean"
        done
    done
done
