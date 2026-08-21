#!/usr/bin/env bash
# The docs/testing/lanes.toml table, the lane docs in docs/testing/, and the
# lane jobs in .github/workflows/ci.yml are a bijection (CLAUDE.md §8). This
# script fails the build on drift in any direction: a row without a doc, a
# doc without a row, a lane without a job, or a job that runs nothing. The
# contract it pins: lane id equals job id equals doc basename.
set -euo pipefail
export LC_ALL=C

ROOT="${1:-.}"
cd "$ROOT"
TABLE="docs/testing/lanes.toml"
CI=".github/workflows/ci.yml"

lanes="$(grep -oE '^\[lane\.[a-z0-9-]+\]$' "$TABLE" | sed -E 's/^\[lane\.([a-z0-9-]+)\]$/\1/' | sort)"
docs="$(find docs/testing -maxdepth 1 -name '*.md' -exec basename {} .md \; | sort)"

rowless="$(comm -13 <(printf '%s\n' "$lanes") <(printf '%s\n' "$docs"))"
if [ -n "$rowless" ]; then
  echo "lane docs with no table row (add the [lane.<id>] row or remove the doc):"
  printf '%s\n' "$rowless"
  exit 1
fi

docless="$(comm -23 <(printf '%s\n' "$lanes") <(printf '%s\n' "$docs"))"
if [ -n "$docless" ]; then
  echo "table rows with no lane doc (write docs/testing/<id>.md in the same commit):"
  printf '%s\n' "$docless"
  exit 1
fi

# Lanes run as jobs in ci.yml; the integration lane runs after staging
# deploys in deploy.yml, because its subject exists only there. A lane's
# home moves; it never leaves the bijection.
JOBFILES=("$CI")
[ -f .github/workflows/deploy.yml ] && JOBFILES+=(.github/workflows/deploy.yml)

jobs="$(awk '
  /^jobs:[[:space:]]*$/ { injobs = 1; next }
  injobs && /^[^ ]/ { injobs = 0 }
  injobs && /^  [a-z0-9-]+:$/ {
    name = $1
    sub(/:$/, "", name)
    print name
  }
' "${JOBFILES[@]}" | sort -u)"

jobless="$(comm -23 <(printf '%s\n' "$lanes") <(printf '%s\n' "$jobs"))"
if [ -n "$jobless" ]; then
  echo "lanes with no CI job (the lane lands when its job runs; add the job or drop the row):"
  printf '%s\n' "$jobless"
  exit 1
fi

for lane in $lanes; do
  lane_found=0
  for jobfile in "${JOBFILES[@]}"; do
    if awk -v lane="$lane" '
      $0 == "  " lane ":" { inlane = 1; next }
      inlane && /^  [a-z0-9-]+:$/ { inlane = 0 }
      inlane && $0 ~ "just +" lane "([[:space:]]|$)" { found = 1 }
      END { exit !found }
    ' "$jobfile"; then
      lane_found=1
      break
    fi
  done
  if [ "$lane_found" -ne 1 ]; then
    echo "lane $lane has no job anywhere that runs 'just $lane'"
    exit 1
  fi
done
