#!/usr/bin/env bash
# Fetch the free test-ROM collection (c-sp/game-boy-test-roms) into ./roms.
set -euo pipefail
cd "$(dirname "$0")/.."
ver=v7.0
[ -d roms/blargg ] && { echo "roms/ already present"; exit 0; }
mkdir -p roms
curl -sSL -o roms/t.zip "https://github.com/c-sp/game-boy-test-roms/releases/download/$ver/game-boy-test-roms-$ver.zip"
(cd roms && unzip -q t.zip && rm t.zip)
echo "fetched test roms $ver into roms/"
