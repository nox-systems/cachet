#!/usr/bin/env bash
# The first-run walkthrough: gather one deployment's identity, verify what
# can be verified, generate the signing keypair, and write infra/.env.<name>
# with everything `just deploy <name>` needs. The GitHub and Cloudflare
# console steps cannot be scripted, so the script prints them as checklists
# at the points where they matter.
#
# The deployment NAME is housekeeping: it names the env file, the alchemy
# stage, and the cachet-<name> resources. The HOST is the protocol identity
# that signs narinfos; renaming the deployment never re-signs anything.
#
# Safe to rerun: it refuses to overwrite an existing file without --force,
# and when the host answers it cross-checks your answers against the live
# deployment's public config, so a rerun against a lost config doubles as
# the recovery path.
set -euo pipefail
cd "$(dirname "$0")/.."

fail() {
  echo "cachet: $1" >&2
  exit 1
}

preflight() {
  local missing=()
  command -v cargo >/dev/null 2>&1 || missing+=("cargo (run this inside nix develop)")
  command -v jq >/dev/null 2>&1 || missing+=("jq (run this inside nix develop)")
  command -v curl >/dev/null 2>&1 || missing+=("curl")
  if [ "${#missing[@]}" -gt 0 ]; then
    printf 'cachet: missing: %s\n' "${missing[@]}" >&2
    exit 1
  fi
}

# prompt <question> [default]: answers on stdout, so assign with $(prompt ...).
prompt() {
  local question="$1" default="${2:-}" suffix="" answer
  [ -n "${default}" ] && suffix=" [${default}]"
  printf '%s%s: ' "${question}" "${suffix}" >&2
  read -r answer
  printf '%s' "${answer:-${default}}"
}

preflight

DEPLOYMENT_NAME="$(prompt "Deployment name (lowercase + dashes; one per deployment)" "production")"
[[ ${DEPLOYMENT_NAME} =~ ^[a-z][a-z0-9-]{1,31}$ ]] ||
  fail "deployment name must match ^[a-z][a-z0-9-]{1,31}\$: resources are named cachet-${DEPLOYMENT_NAME}*"

env_file="infra/.env.${DEPLOYMENT_NAME}"
if [ -e "${env_file}" ] && [ "${1:-}" != "--force" ]; then
  fail "${env_file} already exists; rerun with --force to rebuild it."
fi

CACHET_DEPLOY_HOST="$(prompt "Cache host (the public domain, e.g. cache.example.com)")"
[ -n "${CACHET_DEPLOY_HOST}" ] || fail "the host cannot be empty"

# If the token is in the environment, prove it before anything writes:
# a bootstrap that accepts a dead token wastes the rest of the prompts.
if [ -n "${CLOUDFLARE_API_TOKEN:-}" ]; then
  status="$(curl -fsS -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
    https://api.cloudflare.com/client/v4/user/tokens/verify | jq -r '.result.status' 2>/dev/null)" ||
    fail "CLOUDFLARE_API_TOKEN could not reach Cloudflare; check the network and the token."
  [ "${status}" = "active" ] ||
    fail "CLOUDFLARE_API_TOKEN is ${status}, not active; mint a fresh one (docs/DEPLOY.md lists the scopes)."
else
  echo "cachet: no CLOUDFLARE_API_TOKEN in the environment; the deploy step needs it (session-scoped, never stored)." >&2
fi

# When the host already answers, its public config is the ground truth:
# on a rerun after losing the file, this is what recovers the non-secret
# values, and on a first run it silently proves nothing and moves on.
served_config="$(curl -fsS --max-time 10 "https://${CACHET_DEPLOY_HOST}/api/public/config" 2>/dev/null || true)"

cat >&2 <<CHECKLIST
cachet: bootstrap needs one GitHub OAuth App. In the GitHub org you will
serve, open Settings > Developer settings > OAuth Apps > New OAuth App:

    Application name:               cachet
    Homepage URL:                   https://${CACHET_DEPLOY_HOST}
    Authorization callback URL:     https://${CACHET_DEPLOY_HOST}/_auth/callback
    Enable Device Flow:             on

Then copy the app Client ID, generate a Client Secret, and have both ready
for the next prompts.
CHECKLIST

CACHET_DEPLOY_ORGS="$(prompt "GitHub org slug(s) to serve, comma-separated")"
CACHET_DEPLOY_ADMINS="$(prompt "Admin GitHub login(s), comma-separated")"
CACHET_DEPLOY_OAUTH_CLIENT_ID="$(prompt "OAuth App client id")"
GITHUB_OAUTH_CLIENT_SECRET="$(prompt "OAuth App client secret")"
CACHET_DEPLOY_AUDIENCE="$(prompt "OIDC audience" "cachet")"
CACHET_DEPLOY_DEFAULT_BRANCH_REF="$(prompt "Default branch ref" "refs/heads/main")"

for pair in "org slug(s):${CACHET_DEPLOY_ORGS}" "admin login(s):${CACHET_DEPLOY_ADMINS}" \
  "OAuth client id:${CACHET_DEPLOY_OAUTH_CLIENT_ID}" "OAuth client secret:${GITHUB_OAUTH_CLIENT_SECRET}"; do
  [ -n "${pair#*:}" ] || fail "the ${pair%%:*} cannot be empty"
done

if [ -n "${served_config}" ]; then
  served_orgs="$(printf '%s' "${served_config}" | jq -r '.orgs | join(",") | ascii_downcase')"
  served_client_id="$(printf '%s' "${served_config}" | jq -r '.oauthClientId')"
  answered_orgs="$(printf '%s' "${CACHET_DEPLOY_ORGS}" | tr -d ' ' | tr '[:upper:]' '[:lower:]')"
  if [ "${served_orgs}" != "${answered_orgs}" ] || [ "${served_client_id}" != "${CACHET_DEPLOY_OAUTH_CLIENT_ID}" ]; then
    cat >&2 <<MISMATCH
cachet: the live deployment at ${CACHET_DEPLOY_HOST} disagrees with your
answers (orgs: served "${served_orgs}" vs answered "${answered_orgs}";
client id: served "${served_client_id}" vs answered "${CACHET_DEPLOY_OAUTH_CLIENT_ID}").
Continuing writes a divergent deployment; run with --force only if the
config change is the point of this rerun.
MISMATCH
    [ "${1:-}" = "--force" ] || fail "answered values disagree with the live deployment"
  fi
fi

echo "cachet: generating the signing keypair..."
keydir="$(mktemp -d)"
trap 'rm -rf "${keydir}"' EXIT
cargo run -q -p cachet-cli -- keygen --name "${CACHET_DEPLOY_HOST}-1" --out-dir "${keydir}" >/dev/null
signing_key="$(tr -d '\n' <"${keydir}/cachet-key.secret")"
public_key="$(tr -d '\n' <"${keydir}/cachet-key.public")"
rm -rf "${keydir}"
trap - EXIT

# The nix secret-key wire form is <name>:<base64 of 64 bytes> (secret+public
# halves concatenated); the body of a well-formed key is always 88 chars.
[[ ${signing_key} =~ ^"${CACHET_DEPLOY_HOST}"-1:[A-Za-z0-9+/=]{88}$ ]] ||
  fail "keygen produced a malformed secret: ${signing_key%%:*}:***"

# A rerun regenerates the pair: if the live deployment serves a DIFFERENT
# public key, writing this file swaps the signing key, and every client's
# trusted-key list must follow (docs/DEPLOY.md's rotation section).
if [ -n "${served_config}" ]; then
  served_key="$(printf '%s' "${served_config}" | jq -r '.publicKey')"
  if [ "${served_key}" != "${public_key}" ]; then
    echo "cachet: the live deployment serves key ${served_key%%:*}:…, this rerun generated a new pair." >&2
    echo "cachet: deploying this file is a KEY ROTATION: clients must rerun cachet setup (docs/DEPLOY.md)." >&2
  fi
fi

cat >"${env_file}.tmp" <<ENV
# cachet deployment "${DEPLOYMENT_NAME}" configuration, written by just bootstrap.
# CACHET_SIGNING_KEY never leaves this file and a secret store; keep the
# file at 0600 and never commit it. A second copy in a password manager or
# the CI environment is the recovery answer if this machine dies.
CACHET_DEPLOY_HOST=${CACHET_DEPLOY_HOST}
CACHET_DEPLOY_ORGS=${CACHET_DEPLOY_ORGS}
CACHET_DEPLOY_ADMINS=${CACHET_DEPLOY_ADMINS}
CACHET_DEPLOY_OAUTH_CLIENT_ID=${CACHET_DEPLOY_OAUTH_CLIENT_ID}
CACHET_DEPLOY_AUDIENCE=${CACHET_DEPLOY_AUDIENCE}
CACHET_DEPLOY_DEFAULT_BRANCH_REF=${CACHET_DEPLOY_DEFAULT_BRANCH_REF}
GITHUB_OAUTH_CLIENT_SECRET=${GITHUB_OAUTH_CLIENT_SECRET}
CACHET_SIGNING_KEY=${signing_key}
# Optional: the future UI's origin for the browser login redirect.
#CACHET_DEPLOY_UI_ORIGIN=https://ui.example.com
ENV
chmod 600 "${env_file}.tmp"
mv "${env_file}.tmp" "${env_file}"

cat <<SUMMARY
cachet: wrote ${env_file} (0600).
cachet: the deployment's public key is:

    ${public_key}

cachet: next steps, in order:

    1. Point the zone running ${CACHET_DEPLOY_HOST} at this Cloudflare
       account: the custom domain attaches at deploy time and the zone
       must already exist there.
    2. Keep a second copy of the secrets in ${env_file}: a password
       manager entry, or the CI environment if you deploy from Actions.
       This machine losing the file otherwise means a key rotation
       (docs/DEPLOY.md).
    3. Deploy: CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... just deploy ${DEPLOYMENT_NAME}
       The token needs Workers Scripts:Edit, Workers KV Storage:Edit,
       R2:Edit, Zone:Read, and Zone:DNS:Edit on the zone above.
    4. If GitHub let you set the callback URL only roughly, make sure it
       reads exactly https://${CACHET_DEPLOY_HOST}/_auth/callback.
    5. On a laptop: install the cachet binary from the release page, then
       cachet login --cache-url https://${CACHET_DEPLOY_HOST}
       cachet setup
       cachet doctor
    6. In CI: uses: nox-systems/cachet/action@v1 with cache-url:
       https://${CACHET_DEPLOY_HOST} and permissions id-token: write.

SUMMARY
