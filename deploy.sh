#!/usr/bin/env bash
# Build and deploy to https://biblethumper.akwiat.com via radify (HTTPS upload).
set -euo pipefail
cd "$(dirname "$0")"
scripts/build.sh
radify deploy biblethumper dist
radify health biblethumper
