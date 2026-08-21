# ADR 0006: The upload protocol's fixed constants

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../testing/workerd.md](../testing/workerd.md)

## Context

A cache's write path moves megabytes to gigabytes of NARs under
Cloudflare's request limits. The workable shape is split uploads, and
the previous deployment's constants were chosen against those limits
and pinned by tests: 94,371,800 bytes as the largest single request
body the platform accepts with headroom, 64 MiB as the part size under
the multipart cap of 1000 parts, giving a 64 GiB ceiling per object.
Those numbers are observed platform facts, so the rewrite keeps them verbatim and does not rediscover the ceiling.

The larger decision is protocol shape: how a client declares a split
upload, how the worker accounts for parts, and what happens to
abandoned uploads in a store where every orphan is permanent until GC
learns about it.

## Decision

1. Single-request uploads carry the body directly with a hard cap of
   94,371,800 bytes (`UPLOAD_SINGLE_MAX_BYTES`); above that the client
   must use the multipart protocol.
2. The multipart protocol is a quartet: `POST ?uploads` declaring the
   total in `x-cachet-upload-bytes`, `PUT ?uploadId&partNumber` per
   part of exactly 67,108,864 bytes (the last part smaller), `POST
   ?uploadId` completing with the accumulated part descriptors, and
   `DELETE ?uploadId` aborting. Part numbers are one-based; the worker
   enforces the declared totals and rejects a completion whose parts
   disagree.
3. An open upload is an `uploads/{uploadId}` record in the bucket with
   its declared totals and parts so far; the GC sweep reaps records
   older than the grace window, so interrupted uploads cannot orphan
   permanently.
4. Upload ids are bearer capability strings: knowing one authorizes
   parts for that upload. They enter the world as server-minted random
   ids and die at completion, abort, or reaping.

## Consequences

Client and worker share one plan module, so part arithmetic is proven
once (golden vectors, kani laws) and never re-derived per language.
The quartet's replay behavior (a retried complete with a recorded
answer) falls out of the record document. Storage hygiene degrades
gracefully: the worst a crashed client leaves is a grace-windowed
record.

## Alternatives considered

Streaming whole-object uploads with no cap: dies at the platform's
body-size limit for exactly the NARs a monorepo produces most;
rejected. R2's native multipart surface exposed directly: forces R2
credential handling into the worker's request path and gives the
client no place to declare intent early; the record-as-document design
instead keeps auth, admission, and plan checking in the request path;
rejected. Client-chosen part sizes: one more failure matrix for no
benefit, since the cap is a platform fact; rejected.
