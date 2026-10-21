#!/usr/bin/env bash
# Build a demangled symbol index from the user's BG3 binaries (tier-2 extraction, see ADR 0001).
# Output goes to .cache/symbols/ (gitignored) — never commit it.
set -euo pipefail

DEFAULT_APP="$HOME/Library/Application Support/Steam/steamapps/common/Baldurs Gate 3/Baldur's Gate 3.app"
APP="${BG3_APP:-$DEFAULT_APP}"
OUT="$(cd "$(dirname "$0")/.." && pwd)/.cache/symbols"
ARCH="${ARCH:-arm64}"

[[ -d "$APP" ]] || { echo "BG3 not found at: $APP (set BG3_APP)" >&2; exit 1; }
mkdir -p "$OUT"

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
echo "$VERSION" > "$OUT/VERSION"

index() { # <binary> <name>
  # address<TAB>type<TAB>demangled-name
  nm -U -arch "$ARCH" "$1" \
    | awk '{ addr=$1; type=$2; $1=""; $2=""; sub(/^  /, ""); print addr "\t" type "\t" $0 }' \
    | c++filt > "$OUT/$2.tsv"
  echo "$2: $(wc -l < "$OUT/$2.tsv") symbols"
}

index "$APP/Contents/MacOS/Baldur's Gate 3" bg3
index "$APP/Contents/Frameworks/libOsiris.dylib" osiris
echo "Indexed BG3 $VERSION ($ARCH) into $OUT"
