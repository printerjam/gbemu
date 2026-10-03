#!/usr/bin/env bash
# Run every test-ROM suite and compare against scoreboard-baseline.json.
#   scripts/scoreboard.sh            exit 1 if any baseline-passing test now fails (or vanished)
#   scripts/scoreboard.sh --update   rewrite the baseline from the current results
#   scripts/scoreboard.sh --markdown print gbtest's markdown suite table (no comparison)
# Extra arguments after the mode flag are passed to gbtest (e.g. --long).
set -euo pipefail
cd "$(dirname "$0")/.."

update=0
if [ "${1:-}" = "--update" ]; then update=1; shift; fi

cargo build --release -q -p gb-runner
# Every runnable suite; the *-cgb suites and gbmicrotest-manual are list-only (see `gbtest --list`).
suites=()
for s in blargg blargg-extra mooneye mooneye-mbc acid2 mbc3 gbmicrotest gambatte age same-suite mealybug \
  scribbltests turtle-tests bully strikethrough little-things mooneye-wilbertpol mooneye-extra; do
  suites+=(--suite "$s")
done
if [ "${1:-}" = "--markdown" ]; then
  shift
  exec target/release/gbtest --roms roms "${suites[@]}" --markdown "$@"
fi
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
target/release/gbtest --roms roms "${suites[@]}" --json "$@" > "$tmp"

python3 - "$tmp" scoreboard-baseline.json "$update" <<'PY'
import json, sys

results_path, baseline_path, update = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
data = json.load(open(results_path))
current = {f"{t['suite']}::{t['name']}": t["pass"] for t in data["tests"]}
reasons = {f"{t['suite']}::{t['name']}": t["reason"] for t in data["tests"]}
total, passing = len(current), sum(current.values())

if update:
    with open(baseline_path, "w") as f:
        json.dump(dict(sorted(current.items())), f, indent=1)
        f.write("\n")
    print(f"baseline updated: {passing}/{total} passing")
    sys.exit(0)

try:
    baseline = json.load(open(baseline_path))
except FileNotFoundError:
    sys.exit(f"{baseline_path} missing; run scripts/scoreboard.sh --update")

new_pass = sorted(k for k, v in current.items() if v and not baseline.get(k, False))
regressed = sorted(k for k, v in baseline.items() if v and not current.get(k, False))
print(f"scoreboard: {passing}/{total} passing (baseline {sum(baseline.values())}/{len(baseline)})")
for k in new_pass:
    print(f"  newly passing: {k}")
for k in regressed:
    why = "missing from results" if k not in current else reasons[k]
    print(f"  REGRESSION:    {k}  [{why}]")
if regressed:
    sys.exit(1)
if new_pass:
    print("run scripts/scoreboard.sh --update to record the improvements")
PY
