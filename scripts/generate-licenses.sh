#!/bin/sh
# Regenerates THIRD_PARTY_LICENSES.txt from the Rust core's dependency tree (needs `cargo install cargo-about --features cli`).
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)/core"
cargo about generate --workspace --fail -o ../THIRD_PARTY_LICENSES.txt about.hbs
echo "Wrote THIRD_PARTY_LICENSES.txt"
