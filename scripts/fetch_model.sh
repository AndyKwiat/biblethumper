#!/usr/bin/env bash
# Download the sentence-embedding model (all-MiniLM-L6-v2, Apache-2.0, 22M params, 384 dims)
# into assets/model-f32/, then convert it to f16 (half the size) in assets/model/.
# Idempotent: skips files that already exist.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main"
mkdir -p assets/model-f32 assets/model
for f in config.json tokenizer.json model.safetensors; do
  if [ ! -s "assets/model-f32/$f" ]; then
    echo "downloading $f"
    curl -fsSL -o "assets/model-f32/$f.tmp" "$REPO/$f"
    mv "assets/model-f32/$f.tmp" "assets/model-f32/$f"
  fi
done
cp assets/model-f32/config.json assets/model-f32/tokenizer.json assets/model/
if [ ! -s assets/model/model.safetensors ]; then
  cargo run -q --release -p bt-server --bin bt -- quantize assets/model-f32/model.safetensors assets/model/model.safetensors
fi
ls -la assets/model
