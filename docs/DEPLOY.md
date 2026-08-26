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

Alchemy prints the plan and asks before it changes anything. In a
terminal you answer it. A run with no terminal, which is every CI
deploy, approves automatically, because there is nobody there to ask and
the alternative is a deploy that stops at the prompt. Tearing a
deployment down always asks, terminal or not.

The worker deploys with observability on and a CPU ceiling of five
minutes, and with no placement pin. Observability is what makes a slow
read diagnosable: the worker's own events say whether a read answered
from the edge, from the bucket, or not at all, and without it those
events go nowhere. The CPU ceiling covers the one request whose cost
scales with the object, a multipart completion measuring the NAR its
parts assembled. Placement stays unpinned because the hot path is an
edge-cache hit, which touches no backend for a placement to be near: the
Cache API answers at the colo the worker runs in, so pinning the isolate
next to R2 puts the client's round trip to that colo on every cached
read.

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
| `CACHET_DEPLOY_UI_ORIGIN` | no | Where browser login lands. Unset lands on this deployment's own console; set it to override, or to the empty string for a deployment with no UI. |
| `CACHET_DEPLOY_FONT_CSS` | no | A stylesheet the console loads for licensed faces. Unset ships the free ones. |
| `CACHET_DEPLOY_GC_GRACE_MS` | no | Grace override; default 14 days. Set 0 for throwaway test deployments. |
| `CACHET_SIGNING_KEY` | yes | The `<host>-1:<base64>` secret from bootstrap. |
| `CACHET_OAUTH_CLIENT_SECRET` | yes | The OAuth App's client secret. |
| `CACHET_DEPLOY_STATS_TOKEN` | no | A Cloudflare API token scoped to Account Analytics:Read. Without it the deployment counts but cannot report. Setting it requires `CLOUDFLARE_ACCOUNT_ID` in the deploy environment too, which the worker then carries: the token authorizes the query and the account id says which account to run it against. |

The deploy also reads `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`, unprefixed by choice: they are wrangler and
alchemy's standard variable names, so every Cloudflare tool in the
shell reads the same pair rather than a cachet-specific alias. The
account id is bound into the worker as well, because reading the
deployment's own counters is Cloudflare's SQL API and that API is
addressed per account.

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

The workflow names each of those in its own `env` block rather than
inheriting the environment, so a value added to the table above reaches a
deployment only once it is added there too. A secret the environment does
not hold renders as the empty string, which the config reads as absent:
that is why an optional value can be listed unconditionally, and why a
missing one is a deployment quietly running without it rather than a
failure.

## Upgrading a deployment

Upgrades are manual and local. Fetch the tags, check out the one you
want, run `nix develop`, then `just deploy <name>`: the run builds the
worker bundle with the pinned toolchain and converges the stack in
place. The env file, the signing key, and the bucket live outside the
clone, so they carry over untouched; the served public config's key
prefix after the deploy confirms the identity did not move. Rolling
back is the same command against the previous tag, as the rollback
section below describes.

## Statistics

Every read, every write, and every push's presence probe writes one data
point to the deployment's Analytics Engine dataset, `cachet_<name>`. The
worker can only write to it, because that is all the platform allows a
worker to do, so reading is Cloudflare's SQL API with an account token.

The columns are the same for every point, so one query shape answers
most questions:

| Column | Meaning |
| --- | --- |
| `index1` | `read`, `write`, or `probe`. |
| `blob1` | What it was: `narinfo`, `nar`, `part`, `begin`, `complete`, `abort`, `probe`. |
| `blob2` | How it went: `edge_hit`, `bucket_hit`, `miss`, `stored`, or the HTTP status of a refusal. |
| `blob3` | Who asked: `ci`, `laptop`, `browser`, `anonymous`. |
| `blob4` | `owner/repo` of the workflow run, empty for anyone else. |
| `blob5` | That run's ref, empty for anyone else. |
| `blob6` | Reserved and always empty. |
| `double1` | How many things the point counts. |
| `double2` | Bytes: what a read served, what a write uploaded, and for a probe how many of the paths it asked about the cache already held. |

An admin reads them through the deployment rather than through
Cloudflare. `GET /api/self/events` takes `subject` (`reads`, `writes`,
`probes`), `by` (a dimension or a time bucket, `outcome` when unstated),
`window` (`day`, `week`, `month`), and up to three filters: `kind`,
`outcome`, and `actor`, each naming one value from that column's
vocabulary.

```
curl -sS --netrc-file ~/.netrc \
  "https://<host>/api/self/events?subject=reads&by=actor&window=week"
```

Grouping by a column answers a list, largest first. Grouping by `hour`
or `day` answers a series instead: one row per bucket, oldest first,
with each row's `dimension` the bucket's first instant in epoch seconds.
The series is filled, so a bucket nothing happened in is a zero rather
than a missing row, and a chart drawn from it never runs a straight line
through an empty hour. A bucket finer than its window can hold is
refused, because hourly rows over a month is 720 of them against an
answer capped at 100, and a truncated series is a chart that starts
partway through its own window without saying so. So `hour` goes with
`window=day`, and `day` with `week` or `month`.

Filters are what make a question about one caller class answerable.
Laptop reads split by outcome, which is a question the console's laptop
screen is entirely made of:

```
curl -sS --netrc-file ~/.netrc \
  "https://<host>/api/self/events?subject=reads&by=outcome&window=week&actor=laptop"
```

`repository`, `reference`, and the lease name can be grouped by and not
filtered on. A `GROUP BY` names a column, where a filter names a value,
and those three hold values a pusher chose rather than values from a
closed set.

The caller chooses a question; it never sends one. The worker composes
the SQL from those choices, every part of it a literal or a value an enum
produced, because the credential behind the route is a Cloudflare API
token and a caller who could compose SQL would be composing it with that
token's authority. A choice the deployment does not offer answers 400
`malformed_query` rather than falling back to a different question, and
that includes a filter naming a value nothing writes: narrowing to
nothing is louder than quietly answering the unfiltered question. The
route requires an admin: an org member outside `CACHET_ADMINS` answers
403, an anonymous request 401.

Querying Cloudflare directly is the operator's path, and needs their own
token rather than the deployment's. The hit rate, split by what was
being read:

```sql
SELECT blob1 AS kind, blob2 AS outcome, SUM(_sample_interval * double1) AS reads
FROM cachet_production
WHERE index1 = 'read' AND timestamp > NOW() - INTERVAL '1' DAY
GROUP BY kind, outcome
```

Who the cache is actually for, which is the question a hit rate alone
does not answer:

```sql
SELECT blob3 AS actor, SUM(_sample_interval * double1) AS reads,
       SUM(_sample_interval * double2) AS bytes
FROM cachet_production
WHERE index1 = 'read' AND timestamp > NOW() - INTERVAL '7' DAY
GROUP BY actor
```

Which repositories push, and whether their pushes are landing:

```sql
SELECT blob4 AS repository, blob2 AS outcome, SUM(_sample_interval * double1) AS writes
FROM cachet_production
WHERE index1 = 'write' AND blob4 != '' AND timestamp > NOW() - INTERVAL '7' DAY
GROUP BY repository, outcome
ORDER BY writes DESC
```

How much work the cache saves a push, from the probe that prices it:
`double1` is what the push asked about and `double2` is what the cache
already held, so their ratio is the write-side hit rate:

```sql
SELECT SUM(_sample_interval * double2) / SUM(_sample_interval * double1) AS already_held
FROM cachet_production
WHERE index1 = 'probe' AND timestamp > NOW() - INTERVAL '1' DAY
```

Any of these takes a `timestamp` bucket in the `GROUP BY` to become a
trend rather than a total.

The collector's own numbers are not here: `/api/self/gc-runs` and
`/api/self/stats` already serve them from the run reports, which is a
better source because it is the run's own record rather than a sample.

A dataset is pure configuration, so it costs nothing until something
writes to it and nothing is torn down with the deployment. A worker
older than the binding counts nothing and serves exactly as well;
nothing here is ever worth failing a request over.

## The console

Every deployment serves a browser console at `https://<host>/console`,
and its root redirects there. Sign in with GitHub through the same OAuth
App the CLI uses; an admin sees the collection reports, the counters, and
the access configuration, and an org member outside `CACHET_ADMINS` sees
the access screen alone.

It ships as static files uploaded beside the worker, so `just deploy`
builds it and nothing else needs configuring. The asset layer answers a
request that names one of those files and never invents an answer for one
that does not: a request matching no file falls through to the worker,
which routes it the way it always has. That is what keeps a cache miss a
cache miss (ADR 0014), and the workerd lane asserts it on every run.

A session authenticates the console and nothing else. It reads the
counters and the reports, and it answers 401 on the paths that serve
cache bytes, so a cookie copied out of a browser cannot substitute from
the cache (ADR 0016).

The console ships free faces. A deployment that holds a licence for
others points `CACHET_DEPLOY_FONT_CSS` at a stylesheet serving them, and
the console loads that one stylesheet on top; unset, which is the
default, it uses what it ships.

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
