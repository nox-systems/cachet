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
2. An anonymous narinfo read answers 401.
3. The runner's real OIDC token (audience from the deployment) reads
   the impossible-probe narinfo as 404: authenticated, absent.
4. A one-line file enters the local store with
   `nix-store --add-fixed`, the freshly built `cachet push` sends it,
   and the log counts at least one uploaded object.
5. The local copy is deleted, and `nix copy --from` the deployment
   with only the deployment's served public key trusted returns the
   path: the round trip, with nix itself verifying the signature.

Two safety properties are load-bearing, not incidental. The lane
forces `GITHUB_REF` off the default branch so it can never renew a
lease — the server-side `forbidden_ref` guard is the second wall
behind that choice. And the only store paths it writes are its own
run-tagged payloads, swept by the staging GC whose grace is zeroed
(docs/DEPLOY.md).

In CI the lane runs in `.github/workflows/deploy.yml` as the
`integration` job, `needs: staging`: an ear on every green main's
staging deploy. run it by hand the same way the job does:

```
CACHET_INTEGRATION_URL=https://<host> just integration
```

Run it: `just integration`.
