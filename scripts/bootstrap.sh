#!/usr/bin/env bash
# The first-run walkthrough: gather the deployment's identity, generate the
# signing keypair, and write infra/.env.production with everything a deploy
# needs. The GitHub and Cloudflare console steps cannot be scripted, so the
# script prints them as checklists at the points where they matter. Safe to
# rerun: it refuses to overwrite an existing file without --force.
set -euo pipefail
cd "$(dirname "$0")/.."

env_file="infra/.env.production"
if [ -e "${env_file}" ] && [ "${1:-}" != "--force" ]; then
  echo "cachet: ${env_file} already exists; rerun with --force to rebuild it." >&2
  exit 1
fi

preflight() {
  local missing=()
  command -v cargo >/dev/null 2>&1 || missing+=("cargo (run this inside nix develop)")
  command -v bun >/dev/null 2>&1 || missing+=("bun (run this inside nix develop)")
  if [ "${#missing[@]}" -gt 0 ]; then
    printf 'cachet: missing: %s\n' "${missing[@]}" >&2
    exit 1
  fi
}

# prompt <question> [default]: answers on stdout, so assign with $(prompt ...).
prompt() {
  local question="$1" default="${2:-}" suffix=""
  [ -n "${default}" ] && suffix=" [${default}]"
  printf '%s%s: ' "${question}" "${suffix}" >&2
  read -r answer
  printf '%s' "${answer:-${default}}"
}

preflight

CACHET_DEPLOY_HOST="$(prompt "Cache host (the public domain, e.g. cache.example.com)")"

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

echo "cachet: generating the signing keypair..."
keydir="$(mktemp -d)"
trap 'rm -rf "${keydir}"' EXIT
cargo run -q -p cachet-cli -- keygen --name "${CACHET_DEPLOY_HOST}-1" --out-dir "${keydir}" >/dev/null
signing_key="$(tr -d '\n' <"${keydir}/cachet-key.secret")"
public_key="$(tr -d '\n' <"${keydir}/cachet-key.public")"
rm -rf "${keydir}"
trap - EXIT

cat >"${env_file}.tmp" <<ENV
# cachet deployment configuration, written by just bootstrap.
# CACHET_SIGNING_KEY never leaves this file and a secret store; keep the
# file at 0600 and never commit it.
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
    2. Deploy: CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... just deploy production
       The token needs Workers Scripts:Edit, Workers KV Storage:Edit,
       R2:Edit, Zone:Read, and Zone:DNS:Edit on the zone above.
    3. If GitHub let you set the callback URL only roughly, make sure it
       reads exactly https://${CACHET_DEPLOY_HOST}/_auth/callback.
    4. On a laptop: install the cachet binary from the release page, then
       cachet login --cache-url https://${CACHET_DEPLOY_HOST}
       cachet setup
       cachet doctor
    5. In CI: uses: nox-systems/cachet/action@v1 with cache-url:
       https://${CACHET_DEPLOY_HOST} and permissions id-token: write.

SUMMARY
