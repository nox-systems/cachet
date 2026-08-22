#!/usr/bin/env bash
# The integration lane (docs/testing/integration.md): against a live
# deployment, proves the served truth, the write path over the runner's
# real GitHub OIDC, and the full circle back: substitution with the
# deployment's signature verified. Required environment:
#
#   CACHET_INTEGRATION_URL          the deployment's base URL
#   CACHET_INTEGRATION_AUDIENCE     the OIDC audience (default: cachet)
#   ACTIONS_ID_TOKEN_REQUEST_URL    the runner's OIDC plumbing
#   ACTIONS_ID_TOKEN_REQUEST_TOKEN
#
# The lane fails the job loudly with named fixes when any of that is
# absent: a lane that cannot run is a defect, and silent skips lie
# (CLAUDE.md §0).
set -euo pipefail

fail() {
  echo "integration: FAIL $1" >&2
  exit 1
}

[ -n "${CACHET_INTEGRATION_URL:-}" ] ||
  fail "CACHET_INTEGRATION_URL is unset: point it at the deployment under test (deploy.yml does this for the staging job)"
URL="${CACHET_INTEGRATION_URL%/}"
AUDIENCE="${CACHET_INTEGRATION_AUDIENCE:-cachet}"
command -v jq >/dev/null 2>&1 || fail "jq is missing (it is in the dev shell)"
command -v nix-store >/dev/null 2>&1 || fail "nix is missing (it is in the dev shell)"

say() { echo "integration: $1"; }

# --- the served truth ---

say "fetching /nix-cache-info"
info="$(curl -fsSL "${URL}/nix-cache-info")" || fail "answered no /nix-cache-info"
grep -q "^StoreDir: /nix/store$" <<<"${info}" || fail "/nix-cache-info names no store dir"

say "fetching /api/public/config"
config="$(curl -fsSL "${URL}/api/public/config")" || fail "answered no public config"
host="$(jq -r '.host' <<<"${config}")"
public_key="$(jq -r '.publicKey' <<<"${config}")"
orgs="$(jq -r '.orgs | join(",")' <<<"${config}")"
[ -n "${host}" ] && [ "${host}" != "null" ] || fail "the public config names no host"
say "deployment answers for host ${host}, orgs ${orgs}, key ${public_key%%:*}"
[[ ${public_key} == "${host}-"*":"* ]] ||
  fail "the served public key ${public_key} is not ${host}-<n>:<base64>: deployment identity mismatch"

# --- the read guard over the wire ---

say "an anonymous narinfo read must refuse"
code="$(curl -s -o /dev/null -w '%{http_code}' "${URL}/0000000000000000000000000000000a.narinfo")"
[ "${code}" = "401" ] || fail "anonymous narinfo read answered ${code}, expected 401"

# --- the OIDC write credential ---

[ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] && [ -n "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ] ||
  fail "no OIDC token request variables. Add 'permissions: { contents: read, id-token: write }' to the job."
token="$(curl -fsSL \
  -H "Authorization: Bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}" \
  "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=${AUDIENCE}" | jq -r '.value')" ||
  fail "the OIDC token request failed"
[ -n "${token}" ] && [ "${token}" != "null" ] || fail "the OIDC token answer carried no token"
say "minted the job's OIDC token"

code="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${token}" \
  "${URL}/0000000000000000000000000000000a.narinfo")"
[ "${code}" = "404" ] || fail "the OIDC read probe answered ${code}, expected 404 (authenticated and absent)"

# --- the push ---

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
run_id="${GITHUB_RUN_ID:-local}-$$"
echo "cachet integration lane payload ${run_id}" >"${work}/payload"
say "adding the lane payload to the local store"
store_path="$(nix-store --add-fixed sha256 "${work}/payload" | tail -1)"
say "lane path: ${store_path}"

# The push pipeline reads its configuration from the environment exactly
# as the action's post step resolved it. GITHUB_REF is pinned OFF the
# default branch on purpose: this lane must never renew the lease.
say "pushing through the freshly built cachet binary"
RUNNER_TEMP="${work}" \
  GITHUB_REF="refs/heads/integration-lane" \
  CACHET_CACHE_URL="${URL}" \
  CACHET_AUDIENCE="${AUDIENCE}" \
  CACHET_PROJECT="$(printf '%s' "${GITHUB_REPOSITORY:-lane-org-lane-repo}" | tr '/' '-')" \
  CACHET_ROOTS="${store_path}" \
  CACHET_UPSTREAM_URL="${URL}" \
  CACHET_PUSH=true \
  cargo run -q -p cachet-cli --bin cachet -- push |
  tee "${work}/push.log"
grep -q 'cachet: uploaded [1-9]' "${work}/push.log" ||
  fail "the push uploaded nothing: $(cat "${work}/push.log")"

# --- the circle closes: substitute back with signature verification ---

say "deleting the local copy to force a real substitution"
nix-store --delete "${store_path}" >/dev/null 2>&1 ||
  fail "could not delete the local path: ${store_path}"

say "substituting ${store_path} with only the deployment's key trusted"
host_only="$(printf '%s' "${URL}" | sed -e 's#^https\?://##' -e 's#/.*$##')"
printf 'machine %s login cachet password %s\n' "${host_only}" "${token}" >"${work}/netrc"
chmod 600 "${work}/netrc"
NIX_CONFIG="
substituters = ${URL}
trusted-public-keys = ${public_key}
netrc-file = ${work}/netrc
" nix copy --from "${URL}" "${store_path}"
[ -e "${store_path}" ] || fail "the substitution produced no local path"
say "substitution verified the deployment's signature: ok"

say "the round trip holds"
