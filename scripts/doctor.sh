#!/usr/bin/env bash
# Diff our CPU trace for blargg cpu_instrs individual ROM <n> (1..11) against the
# Gameboy Doctor truth log, printing the first mismatch with context.
#   usage: scripts/doctor.sh <n> [context-lines]
# Env: DOCTOR_DIR (default: <main checkout>/out/doctor), ROMS (default: roms), GBTEST (default: target/release/gbtest)
set -euo pipefail
n=${1:?usage: doctor.sh <n 1..11> [context]}
ctx=${2:-5}
root=$(git rev-parse --show-toplevel)
cd "$root"
dir=${DOCTOR_DIR:-$(cd "$(git rev-parse --git-common-dir)/.." && pwd)/out/doctor}
roms=${ROMS:-roms}
bin=${GBTEST:-target/release/gbtest}

truth="$dir/truth/$n.log"
if [ ! -f "$truth" ]; then
  echo "fetching Gameboy Doctor truth logs into $dir" >&2
  mkdir -p "$dir/truth"
  [ -d "$dir/repo" ] || git clone -q --depth 1 https://github.com/robert/gameboy-doctor "$dir/repo"
  for z in "$dir"/repo/truth/zipped/cpu_instrs/*.zip; do unzip -oq "$z" -d "$dir/truth"; done
fi

rom=$(printf '%s/blargg/cpu_instrs/individual/%02d-*.gb' "$roms" "$n")
rom=$(ls $rom)
[ -x "$bin" ] || cargo build --release -p gb-runner >&2

total=$(wc -l < "$truth")
ours=$(mktemp)
trap 'rm -f "$ours"' EXIT
"$bin" trace "$rom" --doctor --steps "$total" > "$ours"

got=$(wc -l < "$ours")
# first differing line (1-based); empty if the common prefix is identical
first=$(cmp "$ours" "$truth" 2>/dev/null | sed -n 's/.*line \([0-9]*\).*/\1/p' || true)
if [ -z "$first" ]; then
  if [ "$got" -eq "$total" ]; then
    echo "ROM $n: MATCH ($total lines)"
    exit 0
  fi
  first=$((got + 1))
fi
echo "ROM $n: first divergence at line $first of $total (ours emitted $got lines)"
from=$((first > ctx ? first - ctx : 1))
to=$((first + ctx))
echo "--- context (line numbers: ours | truth)"
paste -d'\n' <(sed -n "${from},${to}p" "$ours" | nl -v"$from" -ba -s' ours  ' ) \
              <(sed -n "${from},${to}p" "$truth" | nl -v"$from" -ba -s' truth ' )
exit 1
