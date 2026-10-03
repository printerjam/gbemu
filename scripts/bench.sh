#!/usr/bin/env bash
# Headless speed table: scripts/bench.sh [frames]   (default 3000 frames per ROM)
set -euo pipefail
cd "$(dirname "$0")/.."
frames=${1:-3000}
cargo build --release -q -p gb-runner
home=$(cd "$(git rev-parse --git-common-dir)/.." && pwd)
roms=(roms/dmg-acid2/dmg-acid2.gb "$home/homebrew/tobu.gb" "$home/homebrew/geometrix.gbc")
printf '%-44s %10s %12s\n' ROM frames/s xRealTime
for r in "${roms[@]}"; do
  [ -f "$r" ] || { printf '%-44s %s\n' "$(basename "$r")" "(missing)"; continue; }
  target/release/gbtest bench "$r" --frames "$frames" | sed -E 's|^(.*/)?([^/]+): [0-9]+ frames in [0-9.]+s = ([0-9]+) frames/s = ([0-9]+)x.*|\2 \3 \4|' \
    | awk '{printf "%-44s %10s %11sx\n",$1,$2,$3}'
done
