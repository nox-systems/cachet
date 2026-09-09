# The golden lane

The golden lane compares emitted bytes against committed expected bytes and
fails on drift, because every wire contract in cachet is a byte-level
fact: a narinfo that nix cannot verify, a signature a client cannot check,
or an error body a caller cannot match is a bug no type system finds.

The fixtures fall into two kinds. Generated vectors come from real nix or
real zstd tooling and commit alongside the code that consumes them. Locked
shapes are authored here and describe emissions: the `/nix-cache-info`
body and the bound constants are the first two; problem+json error bodies
and document shapes join with the modules that emit them. The deployment
grammar's two documents are locked here as well: the plan a deploy tool
consumes, for the fullest and the leanest deployment, and the manifest
template that `deploy-manifest.json` is generated from, along with the
refusal wordings an operator reads out of a CI log.

Snapshots commit with the code that produces them. An intentional wire
change updates the snapshot in the same commit; the lane runs with
INSTA_UPDATE=no so CI can never quieten drift (CLAUDE.md §9).

Run it: `just golden`.
