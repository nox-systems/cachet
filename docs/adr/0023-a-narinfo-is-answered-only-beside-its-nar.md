# ADR 0023: A narinfo is answered only beside its NAR

- **Status:** Accepted; point 3's listing amended by
  [ADR 0024](0024-a-small-probe-heads-each-narinfo.md), under which a probe
  of at most 128 hashes heads each narinfo
- **Date:** 2026-10-15
- **Context doc:** [0013-edge-keys-outlive-sweeps.md](0013-edge-keys-outlive-sweeps.md);
  [0005-gc-on-the-cron.md](0005-gc-on-the-cron.md);
  [0012-measure-the-nar-when-it-lands.md](0012-measure-the-nar-when-it-lands.md);
  [../testing/workerd.md](../testing/workerd.md)

## Context

On 15 October 2026 a CI job in nox-systems/actual took 30.5 minutes for
a change to one crate, where the same change takes about 5 on a warm
cache. The deployment answered 200 for the narinfos of three vendored
crate sources (crypto-common 0.1.7, slab 0.4.12, and potential_utf 0.1.6)
and 404 for the NAR files they named. Nix then refused to substitute every
build output that refers to those sources and built the dependency graph
from source.

The order of the two answers decides what nix does. A narinfo that
answers 404 tells nix this cache does not have the path. When no
substituter has it, a path that refers to it counts as an incomplete
closure, and nix builds the missing path and then substitutes the path
that referred to it. A narinfo that answers 200 followed by a NAR that
answers 404 is a failed download instead. Nix's derivation goal retries
substitution after building the inputs only when every failure was an
incomplete closure, so a failed download leaves the derivation to
`--fallback`, which builds it from source along with everything that
depends on it.

The deployment could answer a narinfo whose NAR it no longer held in each
of the following cases:

1. The collector deletes a path's narinfo and then its NAR from the
   bucket. ADR 0013 keys an object's edge entry without the generation
   and gives it thirty days, so a colo that cached the narinfo kept
   answering 200 after the sweep. The NAR has an entry in that colo only
   if a client downloaded the NAR there and the cache has not evicted it,
   and a NAR over 512 MiB never has one, so the NAR read went to the
   bucket and answered 404. No lease reaches a build-only path such as a
   vendored crate source, and the push's probe reports it as held, so no
   push writes it again: the collector deletes it fourteen days after its
   first push, however often CI uses it.
2. The collector deletes narinfos first and their NARs after them, in the
   same tick or in the next one when the budget runs out between the two.
   A push in between finds the narinfo absent and writes the NAR and the
   narinfo again, and the sweep then deletes the NAR it had planned to
   delete. The narinfo stays in the bucket without its NAR.
3. Two store paths with the same contents serialize to the same NAR. The
   plan kept a NAR that a marked narinfo names and deleted one that only
   an unmarked narinfo inside its grace window names, because the plan
   never reads that narinfo.
4. HEAD and a ranged GET answered a narinfo from the bucket without
   looking for its NAR.
5. A NAR write with bytes that are not the key's replaced the stored NAR
   before its measurement refused them. The refusal then deleted the
   object and left its facts document, so the narinfo route kept signing
   narinfos for a NAR that was gone. A multipart completion did the same.

No push repaired any of these. The probe lists the narinfos the bucket
holds and answers presence from that list, so a narinfo stored without its
NAR reads as held, and every push skips it until the collector deletes
it, up to fourteen days later.

## Decision

1. The narinfo route answers a narinfo only when the deployment can serve
   the NAR it names. An entry in this colo's edge cache qualifies when the
   colo also holds an entry for the NAR or the bucket holds the NAR. An
   entry that does not is deleted from this colo, and the read goes on to
   the bucket. A narinfo the bucket holds qualifies when the bucket holds
   its NAR, which one more head of the bucket checks.
2. A narinfo the bucket holds without its NAR is deleted by the read that
   finds it, and the read answers 404. A read whose edge copy already led
   it to head that NAR reuses that answer only for a narinfo stored before
   the request began. A narinfo stored since then can belong to a push
   that stored the NAR after the head, so its NAR is headed again. The 404
   is not cached as a miss, so a push that repairs the path inside the
   thirty-second miss window serves at once. HEAD and ranged GETs of a
   narinfo follow the same rule.
3. The push's probe keeps answering presence from the narinfo listing.
   Because a read deletes a narinfo it finds without its NAR, the job that
   had to build the path finds it absent at its post step and uploads it.
4. The collector keeps a NAR whose facts document was written at or after
   the grace boundary. Every write that stores or re-sends a NAR writes
   its facts document, and a cachet push writes each path's NAR
   immediately before the narinfo that names it, so a fresh facts
   document means a narinfo naming the NAR was stored, or is about to be,
   since the boundary. The plan applies the rule with the inventory it
   has already listed. The sweep applies it again with a listing of
   `meta/nar/` taken before its first NAR batch and taken again once the
   listing is a minute old, against the cutoff the plan recorded in the
   cursor. The report's `narsDeleted` counts only the NARs deleted.
5. A NAR the plan or the sweep keeps by that rule joins a held list at
   `meta/gc-held-nars`, which each run reads when it plans. The plan
   decides every held NAR by the rule it applies to a candidate's URL:
   it deletes one that nothing has written since the grace boundary and
   no marked narinfo names, and holds the rest again. A NAR whose claiming
   push never stored its narinfo is named by no narinfo, so without the
   list it would stay forever.
6. A NAR write never replaces a NAR the bucket holds with its facts. A
   single write to such a key is measured as it arrives and stored
   nowhere. Bytes that measure as the key are the stored NAR's own, so the
   write rewrites the facts document, the claim the collector reads, and
   bytes that do not are refused. A multipart upload to such a key
   assembles under a scratch key below `meta/scratch/`, its completion
   measures the scratch object and deletes it, and the stored NAR is left
   as it was. A write refused while storing a NAR its key did not hold
   deletes the object and its facts document together.

## Consequences

A narinfo read costs one more Cache API lookup, for its NAR, and one
bucket head whenever this colo holds no entry for that NAR. A colo holds
none when the NAR is over 512 MiB, when no client downloaded the NAR
there, and once the cache evicted it, which happens to a large NAR long
before its narinfo's thirty days run out. Each such read waits for one
more round trip to the bucket's region: tens of milliseconds near the
bucket, and 150 to 300 ms from a colo in Asia-Pacific. Nix queries
narinfos 25 at a time by default, so 3,000 narinfo queries from a far
colo whose cache holds none of their NARs take about 20 to 35 seconds
longer. A cold read costs a get and then a head, because the narinfo
names the NAR to check. The heads cost little: 15 million a month come
to about $1.80. Misses cost what they did.

Edge entries keep their thirty days. A narinfo entry whose NAR is gone
costs one bucket head the first time a colo reads it, and the colo then
drops it. The entries production already holds need no purge.

A read can delete a narinfo, and only after the bucket answered during
that request that the NAR it names is absent. The deletion turns a
narinfo no client can substitute from the bucket into an absent one,
which the next push uploads. A push that stores the narinfo again
between the read's head and its delete loses it, and the path reads as
absent until the push after it. R2's binding has no conditional delete,
so that window of one round trip stays.

The collector can still leave a narinfo without its NAR. A push that
writes a planned NAR after the sweep's facts listing and before the
batch that deletes it loses the NAR, a window of at most a minute and
one delete call. The read path finds such a narinfo and deletes it the
first time anyone asks for it.

A client that keeps nix's own narinfo cache, as a laptop or a
self-hosted runner does in `~/.cache/nix`, answers a narinfo from that
cache for `narinfo-cache-positive-ttl`, thirty days by default, without
asking the deployment. Such a client can still fetch a NAR the collector
swept and fail the substitution as before. GitHub-hosted jobs keep no
such cache, so CI on them is covered.

A colo that holds a NAR's entry answers the narinfo even after the bucket
lost the NAR, because that colo serves the NAR. A download of that NAR
that breaks partway cannot resume, because the range path reads the
bucket alone, and the substitution fails as any failed download does.
That was already the case for a NAR the collector deleted.

The held list keeps at most 65,536 NARs. One past that is forgotten and
leaks its bytes, as every held NAR did before the list existed. A
scratch object whose completion's worker died between assembling it and
deleting it stays under `meta/`, which the collector never sweeps.

Build-only paths still leave the cache fourteen days after their first
push. Each one now costs the next job a build of that path alone, where
it used to cost a build of everything above it.

## Alternatives considered

**Cache a positive answer for each NAR the read checked.** An entry under
the generation's key space, for a few minutes, would spare the bucket
head on a narinfo whose NAR this colo does not hold. Rejected for now:
the sweep would have to bump the generation before its NAR phase as well
as after it, and the entry would answer for a NAR the sweep deleted until
the generation reached every isolate. The head costs a round trip and
answers for the bucket as it is.

**Shorten the narinfo's edge lifetime, and delete NARs a tick after their
narinfos.** A deleted narinfo would stop answering within the lifetime,
and its NAR would outlive it. Rejected: every colo would read every
narinfo from the bucket again once per lifetime, which is the cold cache
ADR 0013 removed, and it does nothing for a narinfo the bucket itself
holds without its NAR.

**Purge the edge entry when the collector deletes a narinfo.** Rejected:
the Cache API's delete reaches only the colo that runs it (ADR 0013), and
a global purge needs a zone API token in the worker, for entries keyed
under a synthetic origin.

**Make the probe check NARs.** The probe would answer held only for a
narinfo whose NAR the bucket holds. Rejected: the bucket listing does not
say which NAR a narinfo names, so the probe would read every held
narinfo, one bucket get per path in a request that asks about thousands,
or depend on a NAR key stored beside each narinfo, which no narinfo
stored so far carries.

**Check each held path from the push client.** The client would read the
narinfo of every path the probe reports held and upload the ones that
answer 404. Rejected: it brings back one request per path, the protocol
shape the bulk probe replaced, for a repair the read path already makes
for every path a job had to build.

**Rewrite the facts document from the narinfo route.** Storing a narinfo
would then claim its NAR whatever the client sent before it. Rejected for
now: a rewrite that lands after the collector deleted the NAR leaves a
facts document with no NAR beside it, and the narinfo route accepts that
document as proof the NAR is stored. The NAR write that rewrites it
checks the NAR is still there afterwards, and removes the document when
it is not.

**Collect every orphan NAR.** The collector would delete each NAR no
narinfo names. Rejected: knowing that means reading the URL of every
narinfo in the bucket each run, where the held list names the NARs a
claim kept, which are the only ones this decision can orphan.
