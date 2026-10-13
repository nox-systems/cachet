#!/usr/bin/env bash
# The composite's first step (action/action.yml): provision the job's read
# credential BEFORE nix is installed and its daemon starts. The daemon
# reads netrc-file when it starts and not again, so a credential written
# afterwards is a credential the daemon never sees: cache reads answer
# 401 and every build recompiles from source.
#
# It lives in a file rather than inline in action.yml so the workerd lane
# runs exactly what the action runs (docs/testing/workerd.md).
#
# Inputs, all from the environment: CACHET_CACHE_URL, CACHET_AUDIENCE,
# GITHUB_ENV, the runner's ACTIONS_ID_TOKEN_REQUEST_URL and
# ACTIONS_ID_TOKEN_REQUEST_TOKEN, and CACHET_NETRC_FILE, which only the
# lane sets, because it cannot write /etc/nix.
set -euo pipefail

: "${CACHET_CACHE_URL:?}" "${CACHET_AUDIENCE:?}" "${GITHUB_ENV:?}"
netrc="${CACHET_NETRC_FILE:-/etc/nix/netrc}"
base="${CACHET_CACHE_URL%/}"

# The OIDC token proves the runner is the org's CI. GitHub provisions no
# token plumbing for forked pull requests, by design, so that arbitrary
# code cannot impersonate the repo. The right answer there is a plain
# build: skip every cache-related write so the job continues uncached
# instead of dying before nix exists.
if [ -z "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ]; then
  echo "cachet: no OIDC token grant in this job (a forked pull request, or missing id-token: write)."
  echo "  nix installs and builds WITHOUT the ${CACHET_CACHE_URL} cache; everything else runs normally."
  echo "  Forks that want caching: deploy cachet and point cache-url at it."
  echo "CACHET_EXTRA_CONF=" >>"${GITHUB_ENV}"
  exit 0
fi
# A token fetch that dies on the network is the same class as a fork:
# setup could not complete, the build continues uncached. --retry covers
# the blips (connection resets, DNS, 5xx), and what outlives them
# degrades as loud log lines with the job green.
if ! token="$(curl -fsSL --retry 4 --retry-all-errors --retry-delay 2 \
  -H "Authorization: Bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}" \
  "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=${CACHET_AUDIENCE}" | jq -r '.value')" ||
  [ -z "${token}" ] || [ "${token}" = "null" ]; then
  echo "cachet: the OIDC token fetch failed on the network after retries."
  echo "  nix installs and builds WITHOUT the ${CACHET_CACHE_URL} cache; everything else runs normally."
  echo "CACHET_EXTRA_CONF=" >>"${GITHUB_ENV}"
  exit 0
fi

# Trade the OIDC token, at once, for a credential the deployment issues
# (ADR 0022). The daemon presents whatever the netrc holds on every
# substitution for the rest of the job, and GitHub's OIDC token expires
# within minutes, so writing the token itself lost the cache partway
# through every long job. The issued credential lasts six hours, the
# longest GitHub lets a job run.
#
# why not --retry-all-errors here: a deployment from before the exchange
# accepted OIDC tokens answers 401, and retrying a refusal only delays the
# fallback below. Plain --retry still repeats timeouts, 429, and 5xx.
answer="$(curl -sS --retry 4 --retry-connrefused --retry-delay 2 --max-time 30 \
  -X POST -H "Authorization: Bearer ${token}" -w '\n%{http_code}' \
  "${base}/api/login/exchange")" || answer=$'\n000'
code="${answer##*$'\n'}"
issued="${answer%$'\n'*}"
credential=""
if [ "${code}" = "200" ]; then
  credential="$(jq -r '.token // empty' <<<"${issued}" 2>/dev/null || true)"
fi
if [[ ${credential} =~ ^cachet_[A-Za-z0-9_-]{43}$ ]]; then
  echo "::add-mask::${credential}"
  password="${credential}"
  until="$(jq -r '.expiresAtMs / 1000 | floor | todate' <<<"${issued}")"
  echo "cachet: exchanged the job's OIDC token for a read credential the deployment issued, accepted until ${until}"
  # The post step revokes it, so a copy taken during the job stops
  # reading when the job ends (action/revoke).
  {
    echo "CACHET_ISSUED_NETRC=${netrc}"
    echo "CACHET_REVOKE_URL=${base}/api/login/revoke"
  } >>"${GITHUB_ENV}"
else
  # why the OIDC token as a fallback: it is what every version of the
  # action wrote before this one, so a deployment that has not been
  # upgraded still serves the first minutes of the job.
  password="${token}"
  echo "cachet: ${CACHET_CACHE_URL} did not issue a read credential (HTTP ${code}), so the netrc carries the job's OIDC token, and the cache stops answering this job when that token expires in a few minutes."
fi

# The netrc machine is the cache URL's host name. why parameter expansion:
# the sed form this replaced used `\?`, which macOS's sed does not support,
# so on a macOS runner the machine came out as `https:` and no read ever
# authenticated. why the port goes: curl matches a netrc machine against
# the host name alone, so a line naming host:port is never used.
host="${base#*://}"
host="${host%%/*}"
host="${host%:*}"

# The credential file: real, outside the nix store, 0600 before it holds
# anything. The daemon is pointed at it by the netrc-file line below,
# which it reads when it starts, so the file has to exist first.
sudo mkdir -p "$(dirname "${netrc}")"
sudo install -m 600 /dev/null "${netrc}"
printf 'machine %s login cachet password %s\n' "${host}" "${password}" |
  sudo tee "${netrc}" >/dev/null

# The deployment names its own signing key: fetch the public half from
# the worker rather than asking consumers to paste it. This doubles as
# the connectivity check: a cache that cannot answer its public config
# fails the job here, before the build starts. The retry only asks that
# "cannot answer" outlive a blip.
public_key="$(curl -fsSL --retry 4 --retry-all-errors --retry-delay 2 "${base}/api/public/config" | jq -r '.publicKey')"
# why: nix treats the key settings as restricted, so an untrusted
# caller's extra-trusted-public-keys line is parsed and dropped; the
# invoking user must itself be trusted for the key to count.
{
  echo "CACHET_EXTRA_CONF<<EOF_CACHET_CONF"
  echo "netrc-file = ${netrc}"
  echo "extra-substituters = ${base}"
  echo "extra-trusted-public-keys = ${public_key}"
  echo "trusted-users = root $(whoami)"
  echo "EOF_CACHET_CONF"
} >>"${GITHUB_ENV}"
echo "cachet: read credential provisioned before the daemon starts"
