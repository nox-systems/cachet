#!/usr/bin/env bash
# The deploy-manifest bijection: cachet-deploy's roster and release
# constants against the committed deploy-manifest.json, which is what a
# deployer reads. This script regenerates the document and fails the build
# on any drift, because the contract a deployer renders and the code must
# be one fact (CLAUDE.md §0).
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

regenerated="$(mktemp -t cachet-deploy-manifest.XXXXXX)"
trap 'rm -f "$regenerated"' EXIT

cargo run --quiet -p cachet-deploy --bin deploy-manifest >"$regenerated"

if ! cmp -s deploy-manifest.json "$regenerated"; then
  echo "deploy-manifest.json drifts from the deployment grammar."
  echo "Regenerate it: nix develop --command just deploy-manifest"
  diff -u deploy-manifest.json "$regenerated" | head -50 || true
  exit 1
fi

echo "deploy-manifest.json is in step with the code."
