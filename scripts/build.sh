#!/usr/bin/env bash
# Build a radify deploy folder in dist/:
#   dist/bin/app                 static musl binary (UI + data compiled in)
#   dist/assets/model/*          MiniLM weights (f16), tokenizer, config
#   dist/assets/embeddings.bin   precomputed verse + section vectors
# Rebuilds embeddings.bin when data/ or the model changed (fingerprint check).
set -euo pipefail
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env" 2>/dev/null || true
TARGET=x86_64-unknown-linux-musl

[ -s assets/model/model.safetensors ] || scripts/fetch_model.sh

cargo build --release -p bt-server --target "$TARGET"
BIN=target/$TARGET/release

# (Re)build the index with the same binary we ship; skip if it is current.
if ! "$BIN/bt" check-index >/dev/null 2>&1; then
  "$BIN/bt" build-index
fi

rm -rf dist
mkdir -p dist/bin dist/assets/model
cp "$BIN/app" dist/bin/app
cp assets/model/config.json assets/model/tokenizer.json assets/model/model.safetensors dist/assets/model/
cp assets/embeddings.bin dist/assets/
chmod -R a+rX dist
du -sh dist
