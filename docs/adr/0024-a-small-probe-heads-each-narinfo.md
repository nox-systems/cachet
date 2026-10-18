# ADR 0024: A small probe heads each narinfo, and a large one lists by first character

- **Status:** Accepted
- **Date:** 2026-10-18
- **Context doc:** [0023-a-narinfo-is-answered-only-beside-its-nar.md](0023-a-narinfo-is-answered-only-beside-its-nar.md);
  [../testing/workerd.md](../testing/workerd.md);
  [../testing/unit.md](../testing/unit.md)

## Context

The action's post step took 11 to 15 seconds in nox-systems/actual's CI
even when the job named no roots and uploaded nothing. A golden-lane job
on 18 October 2026 added two store paths that the cache already held, and
its post step printed `cachet: 0 new to cachet, 2 already cached` 14.9
seconds after it started, with nothing printed before. Reading the store
with `nix path-info --all` costs about 0.3 seconds on the same runners,
and the lease renewal that follows the tally on the default branch costs
about 0.4.

The rest was the presence probe. Three `POST /api/probe` requests for two
absent hashes, sent with a laptop credential from Toronto, took 12.0, 10.4,
and 13.2 seconds, while an authorized HEAD of a random absent narinfo took
0.11 to 0.36 seconds and the handshake took 0.06 to 0.13. The probe listed
every narinfo in the bucket, one page of a thousand after another, for
every question, whatever its size. Production held 17,398 narinfos at its
last collector run, so each probe read 18 pages, at about two thirds of a
second a page. The cost grew with the cache and had nothing to do with the
push.

A second cost showed only with roots. The push client evaluated each root
with nix before the probe, one flake evaluation apiece, and only the lease
renewal reads the result. A branch other than the default never renews
the lease, so a pull request paid those evaluations for nothing.

## Decision

1. A probe that names at most `PROBE_HEADS_MAX` hashes (128) heads each
   narinfo key it names, and a hash is held when its head answers. The
   worker keeps six heads waiting at once, because a Worker invocation
   can have six connections waiting for headers, R2's reads among them,
   and a seventh queues.
2. A larger probe lists the narinfos under each first character its hashes
   use, at most 32 listings, six at a time, each paging on its own. Every
   narinfo key begins with its hash, so an asked hash is held exactly when
   its key appears under its own first character.
3. The answer's meaning does not change. A hash is held when the bucket
   holds its narinfo key, as ADR 0023's third decision states for the
   listing, and the answer still comes from the bucket and from nothing
   kept between requests. `cachet_core::probe::plan` makes the choice, so
   the keys and prefixes a probe reads are a pure function the unit lane
   tests.
4. The push client evaluates roots only on the default branch.
5. The probe's `probe.bulk` event counts the heads and the list pages it
   spent. The workerd lane holds a small probe to one head per hash and no
   page, and a large one to one page per first character it asks about.
   The unit lane holds a push whose new paths are all held to one store
   read, one mint, and one probe off the default branch.

## Consequences

A post step with nothing new to upload costs one nix process to read the
store, one OIDC mint, one probe, and on the default branch the lease
renewal and one evaluation per root. A probe of two hashes is two heads in
one round, so its cost no longer grows with the bucket. The production
numbers after the release that carries this are recorded in the pull
request that ships it.

A probe of more than 128 hashes still grows with the bucket, because its
listings read every narinfo under the characters it uses. A cold runner
that substituted thousands of paths from the cache asks about all of them,
and those hashes start with nearly every character, so its listings read
nearly the whole bucket. They now run six at a time instead of one after
another.

A head is an R2 Class B operation and a list page is a Class A operation,
so a probe of a few hashes costs less than the 18 list pages it replaces.
A large probe makes at least one listing per character it uses, up to 32,
which is more requests than one listing on a bucket of a few thousand
narinfos.

A probe makes at most 128 heads, or 32 listings plus their later pages,
which stays far under the Workers Paid limit of 10,000 subrequests that
DEPLOY.md already expects a CI deployment to run on.

## Alternatives considered

**Head every hash, whatever the question's size.** A cold runner's probe
names thousands of hashes, 5,255 in one of actual's jobs, and six heads at
a time would take longer than the listing. A probe may also name up to
16,384 hashes, past the 10,000 subrequests one request may make.

**Probe from the client, one narinfo HEAD per path.** This is the shape
the bulk probe replaced, and ADR 0023 rejected it again for repairs. Each
request pays a credential check of its own, and a read answers a miss
from the colo's edge cache for thirty seconds, so a path a sibling job
pushed a moment ago reads as absent and uploads again. The probe answers
from the bucket and caches nothing.

**Keep a presence index or a memo of the last listing.** Rejected when the
bulk probe landed: an index needs a second writer on every narinfo write
and every sweep, and a memo drifts away from the pushes a sibling job made
a moment ago, which is the dedup a warm push relies on.

**Skip the probe for paths this cache's own key signed.** A path the job
substituted from the cache carries the deployment's signature, but that
says the cache held it when the job fetched it, not that it holds it now,
and the client would have to learn the deployment's key name.

**Choose the bound from the collector's inventory count.** The crossover
between heads and listings moves with the bucket's size, and the latest
run report holds that size. Reading it costs one more bucket read per
probe and answers with a count up to an hour old, and a fixed bound
already keeps the small probe's cost flat.
