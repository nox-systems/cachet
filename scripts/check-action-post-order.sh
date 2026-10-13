#!/usr/bin/env bash
# The composite action's post phases run in the reverse order of their
# main phases, so the order of its steps decides which post runs last.
# The revoke action's post must run after the push's post, because the
# push can still substitute through the daemon and the daemon presents
# the credential being revoked. It must also revoke when a later step
# fails, and GitHub runs a post phase only for a step whose main phase
# ran. Both hold when the revoke action is the composite's first step.
# This script fails the actionlint gate when that order, or either post
# phase's condition, drifts.
set -euo pipefail
export LC_ALL=C

ROOT="${1:-.}"
cd "$ROOT"
COMPOSITE="action/action.yml"

fail() {
  echo "check-action-post-order: $1" >&2
  exit 1
}

first_step="$(grep -n -m1 '^    - ' "$COMPOSITE" | cut -d: -f1)"
revoke="$(grep -n '^    - uses: nox-systems/cachet/action/revoke@' "$COMPOSITE" | cut -d: -f1)"
push="$(grep -n '^    - uses: nox-systems/cachet/action/post@' "$COMPOSITE" | cut -d: -f1)"

[ -n "$revoke" ] || fail "$COMPOSITE does not use action/revoke"
[ -n "$push" ] || fail "$COMPOSITE does not use action/post"
[ "$(wc -l <<<"$revoke")" -eq 1 ] || fail "$COMPOSITE uses action/revoke more than once"
[ "$(wc -l <<<"$push")" -eq 1 ] || fail "$COMPOSITE uses action/post more than once"
[ "$revoke" = "$first_step" ] ||
  fail "action/revoke is not the first step of $COMPOSITE (line $revoke, first step on line $first_step), so a failure before it would leave the credential unrevoked"
[ "$revoke" -lt "$push" ] ||
  fail "action/revoke (line $revoke) comes after action/post (line $push), so the credential would be revoked before the push's post phase runs"

grep -qx '  post-if: always()' action/revoke/action.yml ||
  fail "action/revoke/action.yml does not declare post-if: always(), so a failed job would keep its credential"
grep -qx '  post-if: success()' action/post/action.yml ||
  fail "action/post/action.yml does not declare post-if: success(), so a failed build could populate the cache"

echo "check-action-post-order: action/revoke runs first and its post runs last"
