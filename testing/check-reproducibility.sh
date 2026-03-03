#!/bin/sh
# Check whether builds of pimsync are reproducible.
#
# Builds the project twice into separate directories and compares the resulting
# binaries byte-for-byte. Exits 0 if identical, 1 otherwise.
#
# Usage:
#   ./check-reproducibility.sh
#   ./check-reproducibility.sh --no-default-features

set -eu

CARGO_FLAGS="--release $*"

dir1=$(mktemp -d)
dir2=$(mktemp -d)
trap 'rm -rf "$dir1" "$dir2"' EXIT

printf "Building (1/2) into %s ...\n" "$dir1"
cargo build $CARGO_FLAGS --target-dir "$dir1" 2>&1

printf "Building (2/2) into %s ...\n" "$dir2"
cargo build $CARGO_FLAGS --target-dir "$dir2" 2>&1

bin1="$dir1/release/pimsync"
bin2="$dir2/release/pimsync"

hash1=$(sha256sum < "$bin1" | cut -d' ' -f1)
hash2=$(sha256sum < "$bin2" | cut -d' ' -f1)

printf "\nBuild 1: %s  (%s bytes)\n" "$hash1" "$(wc -c < "$bin1")"
printf "Build 2: %s  (%s bytes)\n" "$hash2" "$(wc -c < "$bin2")"

if [ "$hash1" = "$hash2" ]; then
  printf "\nBuilds are reproducible.\n"
  exit 0
else
  printf "\nBuilds differ!\n"
  diffoscope "$bin1" "$bin2" || true
  exit 1
fi
