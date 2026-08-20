#!/usr/bin/env bash
# The OpenAPI bijection: cachet-api's route descriptors against the
# committed docs/openapi.yaml, which is what the worker serves. This script
# regenerates the document and fails the build on any drift, because the
# served bytes and the code must be one fact (CLAUDE.md §0).
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

regenerated="$(mktemp -t cachet-openapi.XXXXXX)"
trap 'rm -f "$regenerated"' EXIT

cargo run --quiet -p cachet-api --features yaml-export --bin openapi >"$regenerated"

if ! cmp -s docs/openapi.yaml "$regenerated"; then
  echo "docs/openapi.yaml drifts from the route descriptors."
  echo "Regenerate it: nix develop --command just openapi"
  diff -u docs/openapi.yaml "$regenerated" | head -50 || true
  exit 1
fi

echo "openapi.yaml is in step with the code."
