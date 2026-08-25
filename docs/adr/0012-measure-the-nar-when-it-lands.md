# ADR 0012: A NAR is measured by the write that stores it

- **Status:** Accepted
- **Date:** 2026-08-25
- **Context doc:** [../security/threat-model.md](../security/threat-model.md);
  refines the mechanism of [ADR 0001](0001-server-side-signing.md) (its
  verify-then-sign decision stands unchanged)

## Context

ADR 0001 settled that the worker signs a narinfo only after the stored
NAR verifies byte-for-byte. The first implementation put that
verification on the narinfo request: the worker read the object back out
of the bucket, hashed the compressed bytes, decoded the zstd frame,
hashed the decompressed bytes, and only then signed.

The cost is proportional to the object, and it lands on the request that
carries a document of a few hundred bytes. Uploading the rust toolchain
meant a 132 MB re-read, a 409 MB decode, and 541 MB of hashing, in
wasm, after those same bytes had already streamed through the worker
minutes earlier. Measured against CI: a push step of 974 store paths took
21 minutes while the job it was caching took 47 seconds. The client
retries a failed request three times, so a worker that ran out of CPU
paid the whole cost again on each attempt.

The bytes were in the worker's hands once already. Nothing about
verify-then-sign requires measuring them a second time.

## Decision

1. The write that stores a NAR measures it. `PUT /nar/<key>` streams the
   body into the bucket through the same hashers and decoder the narinfo
   request used to run, so the bytes are measured while they are already
   moving. No request reads an object back to learn what it holds, except
   the one case in point 4.

2. The measurement lands beside the object as a facts document at
   `meta/nar/<key>`, holding the decompressed hash and size and the
   compressed hash and size. `meta/` is already a reserved prefix, so the
   document is unreachable from any request and never enters a sweep's
   candidate set. The collector deletes it in the same call as the NAR it
   describes.

3. `PUT /<hash>.narinfo` reads that document and feeds it to the same
   `VerifiedNar::verify` as before. The typestate is unchanged: the
   signing step still accepts only the verifier's output, and the
   verifier still accepts only measured values. The facts document is
   written only after a NAR is measured in full, so its presence is what
   proves the NAR was stored and measured, and its absence refuses the
   narinfo with `narinfo_nar_missing`.

4. A multipart upload measures on completion instead. Its parts arrive
   out of order and in parallel, so no request ever holds the assembled
   bytes in sequence until the completion assembles them; that request
   reads the object once and writes the facts. The cost stays where an
   object large enough to need multipart already is, and it stops being
   something every path pays.

5. A NAR write declares its decompressed size in `x-cachet-nar-bytes`.
   The decoder needs a ceiling before it reads a byte, and the narinfo
   that carries `NarSize` has not arrived yet. The declaration is bounded
   twice: by `NAR_DECOMPRESSED_BYTES_MAX`, and by how far the uploaded
   bytes may expand (`NAR_EXPANSION_RATIO_MAX`), so a compression bomb
   can only spend CPU in proportion to bytes it actually sent.

6. Content addressing is checked by the write. A NAR key names the hash
   of the bytes it holds, so a body measuring as anything else is refused
   and the object deleted. `VerifiedNar::verify` still checks the same
   thing, because the typestate is what makes the order structural rather
   than conventional.

## Consequences

The narinfo request costs a small bucket read whatever the path's size,
and a push no longer pays for its own bytes twice. The verification is
the same computation over the same bytes, moved to the moment they are
stored, which closes the window in which the object could change between
being stored and being measured.

Two new shapes exist: a facts document per NAR, and a header on every NAR
write. A client that omits the header is refused with `length_required`,
which makes this a breaking protocol change for older clients. Both are
covered in the workerd lane, and the facts document's key derives from
the NAR key rather than a grammar of its own, so the two cannot drift.

A NAR stored without its facts document (a worker that died between the
two writes) can never be signed. That is deliberate: the client retries,
and the collector reaps an orphan nothing references.

## Alternatives considered

**Keep the re-read and raise the CPU limit.** Rejected: the limit was
raised anyway, and it does not make the work smaller. The re-read is a
full extra pass over every byte in the cache on the way in, and no
ceiling turns that into a cost worth paying.

**Verify asynchronously and sign later.** Rejected: it moves the cost off
the request without removing it, and it makes "the upload succeeded" stop
meaning "the path is readable". A narinfo left pending by an evicted
worker would need a reconciliation pass in the collector to ever become
readable, which is new machinery guarding against a failure the
synchronous path does not have.

**Trust the client's declared hashes.** Rejected: it is the defense in
ADR 0001, and the threat model's poisoned-narinfo scenario is exactly a
client that lies about them.

**Record the measurement in the object's own R2 custom metadata.**
Rejected: metadata is supplied before the body streams, so values derived
from the body cannot go there. Writing declared values instead would mean
the object lands carrying claims nothing has checked yet, and a worker
that died before the check left a poisoned object a later narinfo would
trust. A separate document written only after the measurement passes
cannot exist in that state.
