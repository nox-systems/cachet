#!/usr/bin/env bash
# Derive the kani lane's crate list from the proof harnesses that exist
# (CLAUDE.md §9): every crate under crates/ whose sources carry `cfg(kani)`
# is a row, discovered by grep, never hand-listed, so a proof joins the lane
# by existing and no list can drift. The crate directory name is the package
# name. Fails loudly when the list is empty: a silent empty list would read
# as a green lane that runs nothing.
set -euo pipefail
export LC_ALL=C
cd "$(dirname "$0")/.."

if hits="$(find crates -name '*.rs' -type f -exec grep -l 'cfg(kani)' {} +)"; then
  :
else
  status=$?
  if [ "$status" -ne 1 ]; then
    echo "[kani-list] the harness grep failed (status $status)" >&2
    exit "$status"
  fi
  hits=""
fi

if [ -z "$hits" ]; then
  echo "[kani-list] found no cfg(kani) harnesses under crates/" >&2
  exit 1
fi

printf '%s\n' "$hits" | cut -d/ -f2 | sort -u
