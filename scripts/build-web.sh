#!/usr/bin/env bash
# Build the browser version into web/pkg/ (wasm + JS glue).
#
# One-time setup (Homebrew's rustc has no wasm32 std; rustup lives alongside it in ~/.cargo/bin and is only
# used by this script, so the rest of the repo keeps building with Homebrew's cargo):
#   curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
#       --default-toolchain stable -t wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129 --locked   # must match the wasm-bindgen crate version
#
# usage: scripts/build-web.sh [--serve [PORT]]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Locally rustup lives in ~/.cargo/bin, which is deliberately not on PATH (Homebrew's cargo is the default);
# in CI rustup is already on PATH. Either way the build runs through `rustup run stable`, whose toolchain
# has the wasm32 target.
if ! command -v rustup >/dev/null && [ -x "$HOME/.cargo/bin/rustup" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v rustup >/dev/null; then
  echo "rustup not found; see the setup notes at the top of $0" >&2
  exit 1
fi
if ! command -v wasm-bindgen >/dev/null; then
  if [ -x "$HOME/.cargo/bin/wasm-bindgen" ]; then export PATH="$HOME/.cargo/bin:$PATH"; else
    echo "wasm-bindgen not found; see the setup notes at the top of $0" >&2
    exit 1
  fi
fi

want=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
have=$(wasm-bindgen --version | awk '{print $2}')
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen CLI $have does not match crate $want: cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi

rustup run stable cargo build --profile web --target wasm32-unknown-unknown -p gb-wasm
mkdir -p web/pkg
wasm-bindgen --target web --no-typescript --out-dir web/pkg --out-name gb_wasm \
  target/wasm32-unknown-unknown/web/gb_wasm.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 -o web/pkg/gb_wasm_bg.wasm web/pkg/gb_wasm_bg.wasm
fi
ls -l web/pkg

if [ "${1:-}" = "--serve" ]; then
  port=${2:-8000}
  echo "serving web/ on http://localhost:$port"
  exec python3 -m http.server "$port" --directory web
fi
