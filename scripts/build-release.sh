#!/usr/bin/env bash
# Quick local test build: cargo build --release, then just the binary at
# build/ai-video-studio-v<version> (nothing else - no icons, no desktop entry).
# Not to be confused with Cargo's own target/release/ - that's its build cache, this is ours.
# For an actual distributable package use scripts/package-studio.sh instead, which
# builds dist/ai-video-studio-linux-<arch>/.
# Usage: scripts/build-release.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo build -p studio --release

version="$(grep -m1 '^version' studio/Cargo.toml | sed -E 's/version\s*=\s*"([^"]+)"/\1/')"
out="$root/build"
rm -rf "$out"
mkdir -p "$out"

bin="$out/ai-video-studio-v$version"
cp target/release/ai-video-studio "$bin"
echo "Built $bin"
