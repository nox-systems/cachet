# Deploying cachet

cachet deploys into your own Cloudflare account. One deployment serves one
or more GitHub orgs. A deployment has a name (`production`, `staging`,
`prod-acme`, any short lowercase string): the name is housekeeping only —
it names the env file, the alchemy stage, and the deployment's resources
(`cachet-<name>`, or the name as-is when it already starts with the
prefix) — while the host name is the protocol identity that signs
narinfos. Deployments in one account share nothing but the account, and
a name identifies one deployment per account: two stacks named
`production` in the same account are one stack, converging. The deploy
itself is one command (`just deploy <name>`); everything below is what
surrounds it.

## Prerequisites

Bring these before the first deploy:

1. A Cloudflare account and a custom API token (My Profile > API
   Tokens): on the account, Workers Scripts:Edit, Workers KV
   Storage:Edit, Workers R2 Storage:Edit, and Secrets Store:Edit (the
   deploy's state store lives there); on the zone that will serve the
   cache, Zone:Read and Zone:DNS:Edit. The account id comes from the
   dashboard's right-hand sidebar. If the account has never run a
   Worker, pick its workers.dev subdomain once in the dashboard first:
   the deploy's state store answers on it.
2. A zone in that account for the cache's custom domain (for example
   `cache.example.com`): the deploy attaches the host name as the
   domain, and `CACHET_DEPLOY_DOMAIN` only overrides it. The host name
   doubles as the signing key's name and appears in every narinfo
   signature, so choose it once.
3. A GitHub org whose Actions runners will write to the cache and whose
   members will read from it.
4. A GitHub OAuth App in that org: homepage `https://<host>`, callback
   `https://<host>/_auth/callback`, Device Flow enabled. The callback URL
   is fixed by the host, so it can be created before anything deploys.
5. The list of GitHub logins that get admin rights (the GC report API).

## The first run

Run `nix develop`, then `just bootstrap`. It asks for the deployment
name (default `production`), then the values above, verifying what it
can on the way: a `CLOUDFLARE_API_TOKEN` in the environment is probed
against Cloudflare before anything writes, and when the host already
answers, the served public config cross-checks your answers so a rerun
after losing the file recovers the non-secret values. It generates the
signing keypair (`cachet keygen`) and writes `infra/.env.<name>` at mode
0600. That file holds the signing secret and the OAuth client secret; it
is gitignored, and it is the canonical local store for them. The printed
public key is what laptops will trust.

Deploy with the Cloudflare credentials in the environment:

```
CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... just deploy production
```

The recipe builds the wasm bundle, installs the infra dependencies from
the lockfile, sources `infra/.env.production`, and runs the alchemy
stack: R2 bucket, KV namespace, the worker with its bindings, the
garbage collector's cron (`0 5 * * *`), and the custom domain. It is
idempotent: rerunning converges, it never duplicates.

The worker deploys with observability on, Smart Placement, and a CPU
ceiling of five minutes. Observability is what makes a slow read
diagnosable: the worker's own events say whether a read answered from
the edge, from the bucket, or not at all, and without it those events go
nowhere. Smart Placement moves the isolate next to R2 and KV, which is
what almost every request waits on. The CPU ceiling covers the one
request whose cost scales with the object, a multipart completion
measuring the NAR its parts assembled.

## The configuration contract

`just deploy <name>` reads `infra/.env.<name>` (or exported
equivalents). These variables define the deployment:

| Variable | Required | Meaning |
| --- | --- | --- |
| `CACHET_DEPLOY_HOST` | yes | The cache's host name; also the signing key's name prefix. |
| `CACHET_DEPLOY_ORGS` | yes | Comma-joined GitHub org slugs; nobody outside them authenticates. |
| `CACHET_DEPLOY_OAUTH_CLIENT_ID` | yes | The OAuth App's client id. |
| `CACHET_DEPLOY_ADMINS` | yes | Comma-joined GitHub logins allowed on `/api/self/*`. |
| `CACHET_DEPLOY_AUDIENCE` | no | OIDC audience; default `cachet`. |
| `CACHET_DEPLOY_DEFAULT_BRANCH_REF` | no | The ref allowed to renew leases; default `refs/heads/main`. |
| `CACHET_DEPLOY_DOMAIN` | no | Custom domain override; defaults to the host. |
| `CACHET_DEPLOY_UI_ORIGIN` | no | Browser login's redirect target; unset answers 204 instead. |
| `CACHET_DEPLOY_GC_GRACE_MS` | no | Grace override; default 14 days. Set 0 for throwaway test deployments. |
| `CACHET_SIGNING_KEY` | yes | The `<host>-1:<base64>` secret from bootstrap. |
| `CACHET_OAUTH_CLIENT_SECRET` | yes | The OAuth App's client secret. |

The deploy also reads `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`, unprefixed by choice: they are wrangler and
alchemy's standard variable names, so every Cloudflare tool in the
shell reads the same pair rather than a cachet-specific alias.

## Rehearsing with a second deployment

One account can hold any number of deployments, one per name. To
rehearse a change, bootstrap a second name with its own host (for
example name `staging`, host `cache-staging.example.com`) and deploy it:
`just deploy staging` builds `cachet-staging-*` while `cachet-production-*`
never enters the plan. Each deployment wants its own host and its own
OAuth App, because the callback URL is fixed by the host. For a
throwaway deployment, set `CACHET_DEPLOY_GC_GRACE_MS=0` in its env file
so the collector sweeps anything you seed on the very next tick; real
deployments keep the 14-day default. Tear a deployment down with
`just destroy <name>`; alchemy asks before deleting.

## This repository's own deploys

The `deploy` workflow in this repository runs staging automatically
after a green `ci` on main, with the grace window pinned to zero in the
workflow itself so the integration lane can sweep what it seeds.
Production is manual: Actions > deploy > Run workflow, choosing
`production`, and the run waits for the `production` environment's
reviewer approval. Each deployment's GitHub environment carries the same
values the local file carries, all of them as environment secrets: the
`CACHET_DEPLOY_*` set, `CACHET_SIGNING_KEY`,
`CACHET_OAUTH_CLIENT_SECRET`, `CLOUDFLARE_API_TOKEN`, and
`CLOUDFLARE_ACCOUNT_ID`.

## Upgrading a deployment

Upgrades are manual and local. Fetch the tags, check out the one you
want, run `nix develop`, then `just deploy <name>`: the run builds the
worker bundle with the pinned toolchain and converges the stack in
place. The env file, the signing key, and the bucket live outside the
clone, so they carry over untouched; the served public config's key
prefix after the deploy confirms the identity did not move. Rolling
back is the same command against the previous tag, as the rollback
section below describes.

## Verifying a deployment

`curl https://<host>/api/public/config` answers the orgs, the host, the
OAuth client id, and the public key; the key must match what bootstrap
printed. Then on a laptop: install the CLI with the one-line installer
in the README, run `cachet login --cache-url https://<host>`,
`cachet setup`, and `cachet doctor`; every probe should print `ok`. The
collector fires on the cron, and its runs appear under
`/api/self/gc-runs` for an admin.

## Losing the env file

The deployment keeps running without the clone; deleting the repository
directory destroys only code. Recovering is a rerun: clone at the tag
you deployed, run `nix develop`, run `just bootstrap` again, and the
script cross-checks your answers against the live deployment's public
config, which answers the host, the orgs, the OAuth client id, and the
key prefix. The two secrets are what the file alone holds. The OAuth
client secret regenerates in the OAuth App's settings. The signing key
is write-only once bound, so keep a second copy at bootstrap time: a
password manager entry, or the GitHub environment you deploy from. If
every copy is gone, the way forward is the key rotation below, and
narinfos signed before the rotation read as cache misses until their
paths are pushed again.

## Rollback

Redeploy the previous commit: `git checkout <previous> && just deploy
<name>` converges the stack to that state. The collector's grace window
protects cache content through it; leases and reports persist in the
bucket, which is deployment-scoped, so a rollback never strands state.

## Key rotation

Rotate the signing key by running `cachet keygen --name <host>-2` (the
suffix increments), replacing `CACHET_SIGNING_KEY` in the env file and in
the CI secrets, and redeploying. Deployments with clients configured
before the rotation must add the new public key: `cachet setup`
refreshes the trusted key list from the deployment's public config on
re-run.
