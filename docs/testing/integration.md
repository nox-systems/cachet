# The integration lane

The integration lane proves the deployment end to end, against the real
thing: a live staging (or production) worker, GitHub's real OIDC issuer,
and nix's own signature verification. The local lanes cannot see three
facts: that the deployed worker actually answers on its domain, that
GitHub issues tokens the claim policy accepts, and that what the
server signs verifies for a real nix client. This lane closes them.

The script is scripts/integration.sh, run as `just integration`. It is
wiring, not machinery: curl, jq, nix, and the freshly built cachet
binary. Its subject is environment, by design
(CACHET_INTEGRATION_URL plus an audience and the GitHub Actions OIDC
variables), and it fails loudly with named fixes when any of it is
missing: a lane that cannot run is a defect, never a skip
(CLAUDE.md §0).

The assertions, in order:

1. `/nix-cache-info` serves, and `/api/public/config` names a host
   whose served public key begins `<host>-n:`.
2. The root redirects to `/console`, the console's shell serves without a
   credential, and the script follows the shell to one of the scripts the
   build emitted: a deploy that uploaded a stale or empty directory fails
   here rather than in somebody's browser.
3. An anonymous narinfo read answers 401.
4. The runner's real OIDC token (audience from the deployment) reads
   the impossible-probe narinfo as 404: authenticated, absent.
5. The composite's main step snapshots the store, a one-line file
   enters the local store with `nix-store --add-fixed`, the freshly
   built `cachet push` sends exactly it, and the log counts at least
   one uploaded object.
6. The local copy is deleted, and `nix copy --from` the deployment
   with only the deployment's served public key trusted returns the
   path: the round trip, with nix itself verifying the signature.
7. An absent narinfo answers 404 and not HTML. The deployment binds an
   asset directory for the console, and this is the row that proves the
   asset layer never answers a path the protocol owns: a cache miss
   answered with the console's shell would be 200 text/html to every nix
   client that asked whether this cache holds a path (ADR 0014).

The lane's configuration keeps it from damaging the deployment it
tests. It forces `GITHUB_REF` off the default branch, so its push can
never renew a lease; the server-side `forbidden_ref` guard would
refuse a renewal anyway. The only store paths the lane writes are its
own run-tagged payloads, which the staging collector sweeps with a
zeroed grace window (docs/DEPLOY.md).

In CI the lane runs in `.github/workflows/deploy.yml` as the
`integration` job, `needs: staging`, after each green main deploys to
staging. Run it by hand the same way the job does:

```
CACHET_INTEGRATION_URL=https://<host> just integration
```

Run it: `just integration`.
