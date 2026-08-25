#!/usr/bin/env bash
# The shipped-artifact hygiene gate (CLAUDE.md §5). The worker bundle must
# never carry ring, aws-lc, the C zstd library, or tokio symbols: deny.toml
# bans them from the graph, and this script catches anything that reaches
# the artifact anyway.
# It also scans the bundle for secret-shaped strings, because a leaked
# constant here ships to an edge runtime. Run after `just wasm`; the build
# output is the input, so the script fails with instructions when it is
# missing.
set -euo pipefail
export LC_ALL=C
cd "$(dirname "$0")/.."

BUILD="crates/cachet-worker/build"
if [ ! -d "$BUILD" ]; then
  echo "[wasm-hygiene] $BUILD not found; run 'just wasm' first" >&2
  exit 2
fi

wasm="$(find "$BUILD" -name '*.wasm' -print -quit)"
if [ -z "$wasm" ]; then
  echo "[wasm-hygiene] no .wasm module under $BUILD" >&2
  exit 2
fi

# why: bare "ring" matches std's string::String, so the patterns require a
# crate-qualified shape: the banned names appear as `::ring::`, `aws_lc_`,
# `zstd_sys`, the C library's own `ZSTD_` exports,
# or `::tokio::` when those runtimes leak into the module.
banned="$(wasm-tools print "$wasm" | grep -E ':(ring|tokio)::|aws_lc|aws-lc|zstd_sys|ZSTD_' || true)"
if [ -n "$banned" ]; then
  echo "[wasm-hygiene] banned runtime symbols in $wasm:" >&2
  printf '%s\n' "$banned" | head -20 >&2
  exit 1
fi

# Secrets never ship: scanning for token-shaped strings catches embedded
# credentials and accidental forks of bearer material into the artifact.
secrets="$(grep -rEl 'ghp_[A-Za-z0-9]{20,}|gho_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY' "$BUILD" || true)"
if [ -n "$secrets" ]; then
  echo "[wasm-hygiene] secret-shaped strings in the bundle:" >&2
  printf '%s\n' "$secrets" >&2
  exit 1
fi
