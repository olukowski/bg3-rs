#!/usr/bin/env bash
# Local references and captures are game-derived content: keep everything in .cache/.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
mkdir -p .cache/shots

for name in laezel imp avatar-deepgnome-cleric; do
    reference=".cache/reference/${name}.png"
    if [[ ! -f "$reference" ]]; then
        echo "Skipping ${name}: missing ${reference}"
        continue
    fi
    case "$name" in
        laezel) args=(--play --combat-test --frame Lae --frame-mode body) ;;
        imp) args=(--play --combat-test --frame Imp --frame-mode body) ;;
        # Play uses the avatar's template name (Gnomes_Male_Deep_Player), rather than Tav.
        avatar-deepgnome-cleric) args=(--play --frame Gnomes_Male_Deep_Player --frame-mode body) ;;
    esac
    ours=".cache/shots/ours-${name}.png"
    comparison=".cache/shots/cmp-${name}.png"
    log=".cache/shots/viewer-${name}.log"
    echo "Capturing ${name} (viewer log: ${log})"
    cargo run --release -p bg3-viewer -- "${args[@]}" \
        --shot "$ours" --shot-after 10 > "$log" 2>&1
    echo "Comparing ${name}: coarse central-region sRGB colour check (0-255; diff = ours - ref)"
    cargo run --release -p bg3x -- shotcmp "$reference" "$ours" -o "$comparison"
    echo "Wrote ${ours} and ${comparison}"
done
