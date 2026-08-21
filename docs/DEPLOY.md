# Deploying cachet

cachet deploys into your own Cloudflare account. One deployment serves one
or more GitHub orgs; stages (`staging`, `production`) are fully separate
sets of resources that share nothing. The deploy itself is one command
(`just deploy <stage>`); everything below is what surrounds it.

## Prerequisites

Bring these before the first deploy:

1. A Cloudflare account and an API token with Workers Scripts:Edit,
   Workers KV Storage:Edit, R2:Edit, Zone:Read, and Zone:DNS:Edit on the
   zone that will serve the cache. The account id comes from the
   dashboard's right-hand sidebar.
2. A zone in that account for the cache's custom domain (for example
   `cache.example.com`). Production attaches the domain at deploy time;
   skip the zone only if you will run production on workers.dev, in which
   case set `CACHET_DEPLOY_DOMAIN` to something public you own anyway:
   the host name doubles as the signing key's name and appears in every
   narinfo signature, so choose it once.
3. A GitHub org whose Actions runners will write to the cache and whose
   members will read from it.
4. A GitHub OAuth App in that org: homepage `https://<host>`, callback
   `https://<host>/_auth/callback`, Device Flow enabled. The callback URL
   is fixed by the host, so it can be created before anything deploys.
5. The list of GitHub logins that get admin rights (the GC report API).

## The first run

Run `nix develop`, then `just bootstrap`. It asks for the values above,
generates the signing keypair (`cachet keygen`), and writes
`infra/.env.production` at mode 0600. That file holds the signing secret
and the OAuth client secret; it is gitignored, and it is the canonical
local store for them. The printed public key is what laptops will trust.

Deploy with the Cloudflare credentials in the environment:

```
CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... just deploy production
```

The recipe builds the wasm bundle, installs the infra dependencies from
the lockfile, sources `infra/.env.production`, and runs the alchemy
stack: R2 bucket, KV namespace, the worker with its bindings, the
garbage collector's cron (`0 5 * * *`), and the custom domain. It is
idempotent: rerunning converges, it never duplicates.

## The configuration contract

`just deploy` reads `infra/.env.<stage>` (or exported equivalents).
These variables name the deployment:

| Variable | Required | Meaning |
| --- | --- | --- |
| `CACHET_DEPLOY_HOST` | yes | The cache's host name; also the signing key's name prefix. |
| `CACHET_DEPLOY_ORGS` | yes | Comma-joined GitHub org slugs; nobody outside them authenticates. |
| `CACHET_DEPLOY_OAUTH_CLIENT_ID` | yes | The OAuth App's client id. |
| `CACHET_DEPLOY_ADMINS` | yes | Comma-joined GitHub logins allowed on `/api/self/*`. |
| `CACHET_DEPLOY_AUDIENCE` | no | OIDC audience; default `cachet`. |
| `CACHET_DEPLOY_DEFAULT_BRANCH_REF` | no | The ref allowed to renew leases; default `refs/heads/main`. |
| `CACHET_DEPLOY_DOMAIN` | no | Custom domain; production defaults to the host, other stages to none. |
| `CACHET_DEPLOY_UI_ORIGIN` | no | Browser login's redirect target; unset answers 204 instead. |
| `CACHET_DEPLOY_GC_GRACE_MS` | no | Grace override; staging defaults to 0, elsewhere 14 days. |
| `CACHET_SIGNING_KEY` | yes | The `<host>-1:<base64>` secret from bootstrap. |
| `GITHUB_OAUTH_CLIENT_SECRET` | yes | The OAuth App's client secret. |

## Staging

`just deploy staging` deploys the same stack as `cachet-staging-*` with
the GC grace window zeroed, so a seeded object is sweepable on the very
next collector tick. Set `CACHET_DEPLOY_DOMAIN` to a staging hostname in
your zone (for example `cache-staging.example.com`): without it staging
lives on an account-specific workers.dev URL that nothing else can
predict, and the integration lane that follows staging deploys needs to
know where to point. Use staging to rehearse changes:
deploy staging, exercise it, then `just deploy production`. Tear a stage
down with `just destroy <stage>`; alchemy asks before deleting.

## CI deploys

The `deploy` workflow runs staging automatically after a green `ci` on
main. Production is manual: Actions > deploy > Run workflow, choosing
`production`, and the run waits for the `production` environment's
reviewer approval. Configure the two GitHub environments with the same
variables the local file carries: `CACHET_DEPLOY_*` as environment
variables, `CACHET_SIGNING_KEY`, `GITHUB_OAUTH_CLIENT_SECRET`,
`CLOUDFLARE_API_TOKEN`, and `CLOUDFLARE_ACCOUNT_ID` as environment
secrets.

## Verifying a deployment

`curl https://<host>/api/public/config` answers the orgs, the host, the
OAuth client id, and the public key; the key must match what bootstrap
printed. Then on a laptop: install the CLI with the one-line installer
in the README, run `cachet login --cache-url https://<host>`,
`cachet setup`, and `cachet doctor`; every probe should print `ok`. The
collector fires on the cron, and its runs appear under
`/api/self/gc-runs` for an admin.

## Rollback

Redeploy the previous commit: `git checkout <previous> && just deploy
<stage>` converges the stack to that state. The collector's grace window
protects cache content through it; leases and reports persist in the
bucket, which is stage-scoped, so a rollback never strands state.

## Key rotation

Rotate the signing key by running `cachet keygen --name <host>-2` (the
suffix increments), replacing `CACHET_SIGNING_KEY` in the env file and in
the CI secrets, and redeploying. Deployments with clients configured
before the rotation must add the new public key: `cachet setup`
refreshes the trusted key list from the deployment's public config on
re-run.
