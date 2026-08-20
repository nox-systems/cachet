#!/usr/bin/env bash
# The CLAUDE.md manifest is a bijection with the repo's non-ADR markdown
# (CLAUDE.md §8). This script fails the build on drift in either direction:
# a doc with no @-import, or an @-import with no doc. It also fails on a dead
# import: a backticked or fenced @-line which the tool ignores.
set -euo pipefail
export LC_ALL=C

ROOT="${1:-.}"
cd "$ROOT"

# The expected set: the root files plus every Markdown file under docs/
# except the on-demand ADR layer. A missing docs tree dies on find's exit
# code instead of passing quietly.
expected="$({
  echo README.md
  echo PROSE.md
  echo SECURITY.md
  find docs -type f -name '*.md' ! -path 'docs/adr/*'
} | sort)"

# The manifest: bare @-imports outside code fences.
manifest="$(awk '
  /^[[:space:]]*```/ { fence = !fence; next }
  fence { next }
  /^[[:space:]]*@[A-Za-z0-9_.\/-]+\.md[[:space:]]*$/ {
    line = $0
    gsub(/[[:space:]]/, "", line)
    print substr(line, 2)
  }
' CLAUDE.md | sort)"

duplicates="$(printf '%s\n' "$manifest" | uniq -d)"
if [ -n "$duplicates" ]; then
  echo "duplicate @-imports in CLAUDE.md:"
  printf '%s\n' "$duplicates"
  exit 1
fi

missing_doc="$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$manifest"))"
if [ -n "$missing_doc" ]; then
  echo "docs missing from the manifest (add an @-import line):"
  printf '%s\n' "$missing_doc"
  exit 1
fi

missing_import="$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$manifest"))"
if [ -n "$missing_import" ]; then
  echo "@-imports with no file on disk (drop the line or write the doc):"
  printf '%s\n' "$missing_import"
  exit 1
fi

# A line that mentions a @-path but is not a bare import loads nothing.
# The word "@-import" in prose is exempt; anything else with the shape fails.
dead="$(awk '
  /^[[:space:]]*```/ { fence = !fence; next }
  {
    if ($0 ~ /@`?-import/) next
    if ($0 ~ /@[A-Za-z0-9_.\/-]+\.md/ && $0 !~ /^[[:space:]]*@[A-Za-z0-9_.\/-]+\.md[[:space:]]*$/) {
      printf "%d: %s\n", FNR, $0
    }
  }
' CLAUDE.md)"
if [ -n "$dead" ]; then
  echo "dead @-imports (backticked, fenced, or inline; they load nothing):"
  printf '%s\n' "$dead"
  exit 1
fi
